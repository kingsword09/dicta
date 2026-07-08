use anyhow::{Context, Result, bail};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
#[cfg(target_os = "macos")]
use tao::platform::macos::{WindowBuilderExtMacOS, WindowExtMacOS};
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, Rect, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use wry::{WebView, WebViewBuilder};

#[cfg(target_os = "macos")]
mod native_glass;

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
use tao::platform::unix::WindowExtUnix;
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
use wry::WebViewBuilderExtUnix;

const PANEL_WIDTH: f64 = 380.0;
const PANEL_HEIGHT: f64 = 448.0;
const LIVE_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const LIVE_WORKER_SHUTDOWN_POLL: Duration = Duration::from_millis(100);

const MENU_START: &str = "start";
const MENU_STOP: &str = "stop";
const MENU_RESTART: &str = "restart";
const MENU_REFRESH: &str = "refresh";
const MENU_QUIT: &str = "quit";

const ACTION_SET_PROVIDER: &str = "set_provider";
const ACTION_START_LIVE: &str = "start_live";
const ACTION_STOP_LIVE: &str = "stop_live";
const ACTION_RESTART_LIVE: &str = "restart_live";
const ACTION_REFRESH: &str = "refresh";
const ACTION_HIDE_PANEL: &str = "hide_panel";
const ACTION_QUIT: &str = "quit";

#[derive(Debug, Clone)]
struct AppConfig {
    dicta_bin: String,
    live_args: Vec<String>,
    ptt_args: Vec<String>,
    activation: ActivationPreference,
    hotkey: Option<ConfiguredHotkey>,
    provider_config: Option<String>,
    provider_state: Option<String>,
    autostart: bool,
    native_glass: bool,
}

#[derive(Debug, Default)]
struct TrayApp {
    config: AppConfig,
    provider_report: Option<ProviderListReport>,
    provider_actions: BTreeMap<String, String>,
    live_child: Option<Child>,
    worker_mode: Option<WorkerMode>,
    ptt_recording: bool,
    hotkey_down: bool,
    status: String,
}

#[derive(Debug, Clone)]
struct ConfiguredHotkey {
    hotkey: HotKey,
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerMode {
    Live,
    Ptt,
}

impl WorkerMode {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Ptt => "PTT",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Ptt => "PTT",
        }
    }

    fn state_name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Ptt => "ptt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationPreference {
    Auto,
    Live,
    Ptt,
}

impl ActivationPreference {
    fn ready_label(self) -> &'static str {
        match self {
            Self::Auto => "realtime-ready",
            Self::Live => "live-ready",
            Self::Ptt => "PTT-ready",
        }
    }
}

#[derive(Debug, Clone)]
enum UserEvent {
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    Panel(PanelMessage),
    Hotkey { id: u32, state: HotKeyState },
    ShutdownSignal,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProviderListReport {
    current: Option<String>,
    providers: Vec<ProviderListEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProviderListEntry {
    name: String,
    kind: String,
    #[serde(default)]
    selected: bool,
    #[serde(default)]
    live: bool,
    #[serde(default)]
    ptt: bool,
    #[serde(default)]
    local_config_ok: bool,
    local_config_error: Option<String>,
    model: String,
}

#[derive(Debug, Deserialize, Clone)]
struct PanelMessage {
    action: String,
    provider: Option<String>,
}

#[derive(Debug, Serialize)]
struct PanelState<'a> {
    status: &'a str,
    current: Option<&'a str>,
    live_running: bool,
    worker_mode: Option<&'static str>,
    ptt_recording: bool,
    selected_ptt: bool,
    hotkey: Option<&'a str>,
    selected_ready: bool,
    providers: &'a [ProviderListEntry],
}

struct Panel {
    window: Window,
    webview: WebView,
    visible: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            dicta_bin: "dicta".to_owned(),
            live_args: vec![
                "--provider".to_owned(),
                "active".to_owned(),
                "--live".to_owned(),
            ],
            ptt_args: vec![
                "--provider".to_owned(),
                "active".to_owned(),
                "--ptt".to_owned(),
            ],
            activation: ActivationPreference::Auto,
            hotkey: None,
            provider_config: None,
            provider_state: None,
            autostart: false,
            native_glass: false,
        }
    }
}

impl AppConfig {
    fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(value) = env::var("DICTA_BIN") {
            if !value.trim().is_empty() {
                config.dicta_bin = value;
            }
        }
        if let Ok(value) = env::var("DICTA_UI_LIVE_ARGS") {
            let args: Vec<String> = value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            if !args.is_empty() {
                config.live_args = args;
            }
        }
        if let Ok(value) = env::var("DICTA_UI_PTT_ARGS") {
            let args: Vec<String> = value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            if !args.is_empty() {
                config.ptt_args = args;
            }
        }
        if let Ok(value) = env::var("DICTA_UI_ACTIVATION") {
            config.activation = match value.trim().to_ascii_lowercase().as_str() {
                "live" => ActivationPreference::Live,
                "ptt" => ActivationPreference::Ptt,
                _ => ActivationPreference::Auto,
            };
        }
        if let Ok(value) = env::var("DICTA_UI_HOTKEY") {
            match parse_hotkey_config(&value) {
                Ok(hotkey) => config.hotkey = hotkey,
                Err(error) => eprintln!("dicta-tray: hotkey disabled: {error}"),
            }
        }
        config.provider_config = non_empty_env("DICTA_PROVIDER_CONFIG");
        config.provider_state = non_empty_env("DICTA_PROVIDER_STATE");
        config.autostart = env::var("DICTA_UI_AUTOSTART").is_ok_and(|value| value == "1");
        config.native_glass = env::var("DICTA_UI_NATIVE_GLASS").is_ok_and(|value| value == "1");
        config
    }

    fn dicta_command(&self) -> Command {
        let mut command = Command::new(&self.dicta_bin);
        if let Some(path) = &self.provider_config {
            command.env("DICTA_PROVIDER_CONFIG", path);
        }
        if let Some(path) = &self.provider_state {
            command.env("DICTA_PROVIDER_STATE", path);
        }
        command
    }
}

impl TrayApp {
    fn new(config: AppConfig) -> Self {
        Self {
            config,
            provider_report: None,
            provider_actions: BTreeMap::new(),
            live_child: None,
            worker_mode: None,
            ptt_recording: false,
            hotkey_down: false,
            status: "Starting".to_owned(),
        }
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        eprintln!("dicta-tray: {}", self.status);
    }

    fn refresh(&mut self) {
        match self.load_provider_report_with_live_default() {
            Ok((report, status)) => {
                self.provider_report = Some(report);
                self.status = status;
            }
            Err(error) => {
                self.provider_report = None;
                self.status = format!("Provider list failed: {error}");
                eprintln!("dicta-tray: {}", self.status);
            }
        }
    }

    fn selected_provider(&self) -> Option<&str> {
        self.provider_report
            .as_ref()
            .and_then(|report| report.current.as_deref())
    }

    fn selected_provider_entry(&self) -> Option<&ProviderListEntry> {
        let selected = self.selected_provider()?;
        self.provider_report
            .as_ref()?
            .providers
            .iter()
            .find(|provider| provider.name == selected)
    }

    fn selected_provider_ready(&self) -> bool {
        self.selected_provider_entry()
            .is_some_and(|provider| self.provider_ready_for_activation(provider))
    }

    fn selected_provider_mode(&self) -> Option<WorkerMode> {
        selected_provider_mode(self.selected_provider_entry()?, self.config.activation)
    }

    fn provider_by_name(&self, name: &str) -> Option<&ProviderListEntry> {
        self.provider_report
            .as_ref()?
            .providers
            .iter()
            .find(|provider| provider.name == name)
    }

    fn provider_switchable(&self, name: &str) -> bool {
        self.provider_by_name(name)
            .is_some_and(|provider| self.provider_ready_for_activation(provider))
    }

    fn provider_ready_for_activation(&self, provider: &ProviderListEntry) -> bool {
        provider.local_config_ok
            && selected_provider_mode(provider, self.config.activation).is_some()
    }

    fn load_provider_report(&self) -> Result<ProviderListReport> {
        let output = self
            .config
            .dicta_command()
            .args(["--json", "provider", "list"])
            .output()
            .with_context(|| format!("failed to run `{}`", self.config.dicta_bin))?;
        if !output.status.success() {
            bail!("{}", command_error(&output.stderr));
        }
        let report: ProviderListReport =
            serde_json::from_slice(&output.stdout).context("failed to parse provider list JSON")?;
        Ok(filter_provider_report_for_tray(report))
    }

    fn load_provider_report_with_live_default(&self) -> Result<(ProviderListReport, String)> {
        let report = self.load_provider_report()?;
        if report_current_switchable_for(&report, self.config.activation) {
            return Ok((report, "Ready".to_owned()));
        }

        let Some(name) =
            first_switchable_provider_for(&report, self.config.activation).map(ToOwned::to_owned)
        else {
            return Ok((
                report,
                format!(
                    "No {} providers found",
                    self.config.activation.ready_label()
                ),
            ));
        };
        self.remember_provider(&name)?;
        let report = self.load_provider_report()?;
        Ok((report, format!("Provider switched to {name}")))
    }

    fn remember_provider(&self, name: &str) -> Result<()> {
        let output = self
            .config
            .dicta_command()
            .args(["provider", "set", name])
            .output();
        match output {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => bail!("{}", command_error(&output.stderr)),
            Err(error) => Err(error).context("failed to run provider set"),
        }
    }

    fn set_provider(&mut self, name: &str) {
        if !self.provider_switchable(name) {
            let status = self
                .provider_by_name(name)
                .map(unavailable_provider_status)
                .unwrap_or_else(|| format!("Provider {name} is unavailable"));
            self.set_status(status);
            return;
        }

        self.set_status(format!("Switching provider to {name}"));
        match self.remember_provider(name) {
            Ok(()) => {
                let switched_status = format!("Provider switched to {name}");
                let was_running = self.live_child.is_some();
                if was_running {
                    self.shutdown_live_worker();
                }
                self.refresh();
                self.set_status(switched_status);
                if was_running {
                    self.start_live();
                }
            }
            Err(error) => {
                self.set_status(format!("Provider switch failed: {error}"));
            }
        }
    }

    fn start_live(&mut self) {
        if !self.selected_provider_ready() {
            let status = match self.selected_provider_entry() {
                Some(provider) => unavailable_provider_status(provider),
                None => "No live-ready provider is selected".to_owned(),
            };
            self.set_status(status);
            return;
        }
        let mode = self.selected_provider_mode().unwrap_or(WorkerMode::Live);
        if self.live_child.is_some() {
            match (self.worker_mode, mode, self.ptt_recording) {
                (Some(WorkerMode::Ptt), WorkerMode::Ptt, false) => {
                    self.start_ptt_recording();
                }
                (Some(WorkerMode::Ptt), WorkerMode::Ptt, true) => {
                    self.set_status("PTT is already recording");
                }
                _ => self.set_status("Live already running"),
            }
            return;
        }

        let provider = self
            .selected_provider()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "active provider".to_owned());
        self.set_status(format!("Starting {} with {provider}", mode.label()));

        let mut command = self.config.dicta_command();
        configure_live_worker_process(&mut command);
        let log_path = match configure_live_worker_stdio(&mut command, &provider) {
            Ok(path) => Some(path),
            Err(error) => {
                eprintln!("dicta-tray: failed to open live worker stderr log: {error}");
                command.stderr(Stdio::null());
                None
            }
        };
        match mode {
            WorkerMode::Live => {
                command.args(&self.config.live_args).stdin(Stdio::null());
            }
            WorkerMode::Ptt => {
                command.args(&self.config.ptt_args).stdin(Stdio::piped());
            }
        }

        match command.spawn() {
            Ok(child) => {
                self.live_child = Some(child);
                self.worker_mode = Some(mode);
                self.ptt_recording = false;
                if let Some(path) = log_path {
                    eprintln!("dicta-tray: live worker stderr log: {}", path.display());
                }
                match mode {
                    WorkerMode::Live => self.set_status(format!("Live running with {provider}")),
                    WorkerMode::Ptt => self.start_ptt_recording(),
                }
            }
            Err(error) => {
                self.set_status(format!("{} start failed: {error}", mode.label()));
            }
        }
    }

    fn stop_live(&mut self) {
        if self.worker_mode == Some(WorkerMode::Ptt) && self.ptt_recording {
            self.stop_ptt_recording();
            return;
        }
        self.shutdown_live_worker();
    }

    fn start_ptt_recording(&mut self) {
        match self.write_ptt_toggle() {
            Ok(()) => {
                self.ptt_recording = true;
                let provider = self.selected_provider().unwrap_or("active provider");
                self.set_status(format!("PTT recording with {provider}"));
            }
            Err(error) => {
                self.set_status(format!("PTT start failed: {error}"));
                self.shutdown_live_worker();
            }
        }
    }

    fn stop_ptt_recording(&mut self) {
        match self.write_ptt_toggle() {
            Ok(()) => {
                self.ptt_recording = false;
                let provider = self.selected_provider().unwrap_or("active provider");
                self.set_status(format!("PTT finalizing with {provider}"));
            }
            Err(error) => {
                self.set_status(format!("PTT stop failed: {error}"));
                self.shutdown_live_worker();
            }
        }
    }

    fn write_ptt_toggle(&mut self) -> std::io::Result<()> {
        let Some(child) = self.live_child.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "PTT worker is not running",
            ));
        };
        let Some(stdin) = child.stdin.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "PTT worker stdin is not available",
            ));
        };
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    fn shutdown_live_worker(&mut self) {
        let Some(mut child) = self.live_child.take() else {
            self.set_status("Live is not running");
            return;
        };
        let mode = self.worker_mode.take().unwrap_or(WorkerMode::Live);
        self.ptt_recording = false;

        self.set_status(format!("Stopping {}", mode.label()));
        if let Err(error) = request_live_worker_shutdown(&mut child) {
            eprintln!("dicta-tray: failed to request live worker shutdown: {error}");
        }
        match wait_live_worker_exit(&mut child, LIVE_WORKER_SHUTDOWN_TIMEOUT) {
            Ok(Some(status)) => {
                eprintln!("dicta-tray: live worker exited with {status}");
                self.set_status(format!("{} stopped", mode.title()));
                return;
            }
            Ok(None) => {
                eprintln!("dicta-tray: live worker did not exit after graceful shutdown request");
                if let Err(error) = force_live_worker_shutdown(&mut child) {
                    eprintln!("dicta-tray: failed to force live worker shutdown: {error}");
                }
            }
            Err(error) => {
                self.set_status(format!("Live stop wait failed: {error}"));
                return;
            }
        }

        match child.wait() {
            Ok(status) => {
                eprintln!("dicta-tray: live worker exited with {status}");
                self.set_status(format!("{} stopped", mode.title()));
            }
            Err(error) => {
                self.set_status(format!("Live stop wait failed: {error}"));
            }
        }
    }

    fn restart_live(&mut self) {
        self.set_status("Restarting live");
        if self.live_child.is_some() {
            self.shutdown_live_worker();
        } else {
            eprintln!("dicta-tray: live was not running; starting selected provider");
        }
        self.start_live();
    }

    fn reap_live(&mut self) -> bool {
        let Some(child) = self.live_child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.live_child = None;
                self.worker_mode = None;
                self.ptt_recording = false;
                self.status = format!("Live exited with {status}");
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.live_child = None;
                self.worker_mode = None;
                self.ptt_recording = false;
                self.status = format!("Live status failed: {error}");
                true
            }
        }
    }

    fn build_menu(&mut self) -> Result<Menu> {
        self.provider_actions.clear();
        let menu = Menu::new();
        let status = MenuItem::with_id(
            MenuId::new("status"),
            format!("Status: {}", self.status),
            false,
            None,
        );
        menu.append(&status)?;
        menu.append(&PredefinedMenuItem::separator())?;

        if let Some(report) = &self.provider_report {
            if report.providers.is_empty() {
                let item = MenuItem::new("No realtime providers found", false, None);
                menu.append(&item)?;
            } else {
                for (index, provider) in report.providers.iter().enumerate() {
                    let id = format!("provider:{index}");
                    self.provider_actions
                        .insert(id.clone(), provider.name.clone());
                    let label = provider_label(provider);
                    let enabled = self.provider_ready_for_activation(provider);
                    let item = CheckMenuItem::with_id(
                        MenuId::new(&id),
                        label,
                        enabled,
                        provider.selected,
                        None,
                    );
                    menu.append(&item)?;
                }
            }
        } else {
            let item = MenuItem::new("Provider list unavailable", false, None);
            menu.append(&item)?;
        }

        menu.append(&PredefinedMenuItem::separator())?;
        let start = MenuItem::with_id(
            MenuId::new(MENU_START),
            self.start_menu_label(),
            self.can_start_selected_worker(),
            None,
        );
        let stop = MenuItem::with_id(
            MenuId::new(MENU_STOP),
            self.stop_menu_label(),
            self.can_stop_selected_worker(),
            None,
        );
        let restart = MenuItem::with_id(
            MenuId::new(MENU_RESTART),
            "Restart Worker",
            self.live_child.is_some() || self.selected_provider_ready(),
            None,
        );
        let refresh = MenuItem::with_id(MenuId::new(MENU_REFRESH), "Refresh Providers", true, None);
        menu.append_items(&[&start, &stop, &restart, &refresh])?;
        menu.append(&PredefinedMenuItem::separator())?;
        let quit = MenuItem::with_id(MenuId::new(MENU_QUIT), "Quit dicta", true, None);
        menu.append(&quit)?;
        Ok(menu)
    }

    fn tray_title(&self) -> String {
        if self.live_child.is_some() {
            "dicta *".to_owned()
        } else {
            "dicta".to_owned()
        }
    }

    fn tooltip(&self) -> String {
        match self.selected_provider() {
            Some(provider) => format!("dicta: {provider} ({})", self.status),
            None => format!("dicta: no active provider ({})", self.status),
        }
    }

    fn panel_state(&self) -> PanelState<'_> {
        let providers = self
            .provider_report
            .as_ref()
            .map(|report| report.providers.as_slice())
            .unwrap_or(&[]);
        PanelState {
            status: &self.status,
            current: self.selected_provider(),
            live_running: self.live_child.is_some(),
            worker_mode: self.worker_mode.map(WorkerMode::state_name),
            ptt_recording: self.ptt_recording,
            selected_ptt: self.selected_provider_mode() == Some(WorkerMode::Ptt),
            hotkey: self
                .config
                .hotkey
                .as_ref()
                .map(|hotkey| hotkey.label.as_str()),
            selected_ready: self.selected_provider_ready(),
            providers,
        }
    }

    fn can_start_selected_worker(&self) -> bool {
        if !self.selected_provider_ready() {
            return false;
        }
        match (
            self.live_child.is_some(),
            self.worker_mode,
            self.ptt_recording,
        ) {
            (false, _, _) => true,
            (true, Some(WorkerMode::Ptt), false) => true,
            _ => false,
        }
    }

    fn can_stop_selected_worker(&self) -> bool {
        match self.worker_mode {
            Some(WorkerMode::Ptt) => self.ptt_recording,
            Some(WorkerMode::Live) => self.live_child.is_some(),
            None => false,
        }
    }

    fn start_menu_label(&self) -> &'static str {
        match self.selected_provider_mode() {
            Some(WorkerMode::Ptt) => "Start PTT",
            _ => "Start Live",
        }
    }

    fn stop_menu_label(&self) -> &'static str {
        match self.worker_mode {
            Some(WorkerMode::Ptt) => "Stop PTT",
            _ => "Stop Live",
        }
    }

    fn handle_hotkey(&mut self, id: u32, state: HotKeyState) {
        let Some(configured) = &self.config.hotkey else {
            return;
        };
        if configured.hotkey.id() != id {
            return;
        }
        match state {
            HotKeyState::Pressed => {
                if self.hotkey_down {
                    return;
                }
                self.hotkey_down = true;
                if self.selected_provider_mode() == Some(WorkerMode::Ptt) {
                    if !self.ptt_recording {
                        self.start_live();
                    }
                } else if self.live_child.is_some() {
                    self.shutdown_live_worker();
                } else {
                    self.start_live();
                }
            }
            HotKeyState::Released => {
                self.hotkey_down = false;
                if self.worker_mode == Some(WorkerMode::Ptt) && self.ptt_recording {
                    self.stop_ptt_recording();
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let config = AppConfig::from_env();
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Tray(event));
    }));

    install_shutdown_signal_handler(event_loop.create_proxy());

    let mut app = TrayApp::new(config);
    app.refresh();
    let _hotkey_registration = install_global_hotkey(&app.config, event_loop.create_proxy());
    if app.config.autostart {
        app.start_live();
    }

    let panel_proxy = event_loop.create_proxy();
    let mut tray_icon: Option<TrayIcon> = None;
    let mut panel: Option<Panel> = None;
    event_loop.run(move |event, _target, control_flow| {
        update_control_flow(&app, control_flow);
        match event {
            Event::NewEvents(StartCause::Init) => {
                match create_tray_icon(&mut app) {
                    Ok(icon) => tray_icon = Some(icon),
                    Err(error) => {
                        eprintln!("dicta-tray: failed to create tray icon: {error}");
                        *control_flow = ControlFlow::Exit;
                    }
                }
                match create_panel(_target, panel_proxy.clone(), &app) {
                    Ok(created_panel) => panel = Some(created_panel),
                    Err(error) => {
                        eprintln!("dicta-tray: failed to create panel UI: {error}");
                    }
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if app.reap_live() {
                    refresh_shell(&mut app, tray_icon.as_ref(), panel.as_ref());
                }
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                let id = event.id().0.as_str();
                if let Some(name) = app.provider_actions.get(id).cloned() {
                    app.set_provider(&name);
                } else {
                    match id {
                        MENU_START => app.start_live(),
                        MENU_STOP => app.stop_live(),
                        MENU_RESTART => app.restart_live(),
                        MENU_REFRESH => app.refresh(),
                        MENU_QUIT => {
                            app.shutdown_live_worker();
                            *control_flow = ControlFlow::Exit;
                        }
                        _ => {}
                    }
                }
                app.reap_live();
                refresh_shell(&mut app, tray_icon.as_ref(), panel.as_ref());
            }
            Event::UserEvent(UserEvent::Tray(event)) => {
                app.reap_live();
                if tray_click_opens_panel(&event) {
                    if let Some(panel) = panel.as_mut() {
                        toggle_panel(panel, &event);
                    }
                }
                refresh_shell(&mut app, tray_icon.as_ref(), panel.as_ref());
            }
            Event::UserEvent(UserEvent::Panel(message)) => {
                let exit = handle_panel_message(message, &mut app, panel.as_mut());
                app.reap_live();
                refresh_shell(&mut app, tray_icon.as_ref(), panel.as_ref());
                if exit {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::Hotkey { id, state }) => {
                app.handle_hotkey(id, state);
                app.reap_live();
                refresh_shell(&mut app, tray_icon.as_ref(), panel.as_ref());
            }
            Event::UserEvent(UserEvent::ShutdownSignal) => {
                app.shutdown_live_worker();
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                window_id, event, ..
            } => {
                if let Some(panel) = panel.as_mut()
                    && panel.window.id() == window_id
                {
                    match event {
                        WindowEvent::CloseRequested | WindowEvent::Focused(false) => {
                            hide_panel(panel);
                        }
                        _ => {}
                    }
                }
            }
            Event::LoopDestroyed => {
                app.shutdown_live_worker();
            }
            _ => {}
        }
        if !matches!(*control_flow, ControlFlow::ExitWithCode(_)) {
            update_control_flow(&app, control_flow);
        }
    });
}

fn install_shutdown_signal_handler(proxy: tao::event_loop::EventLoopProxy<UserEvent>) {
    if let Err(error) = ctrlc::set_handler(move || {
        let _ = proxy.send_event(UserEvent::ShutdownSignal);
    }) {
        eprintln!("dicta-tray: failed to install shutdown signal handler: {error}");
    }
}

struct HotkeyRegistration {
    _manager: GlobalHotKeyManager,
    _hotkey: HotKey,
}

fn install_global_hotkey(
    config: &AppConfig,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) -> Option<HotkeyRegistration> {
    let configured = config.hotkey.as_ref()?;
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            eprintln!("dicta-tray: failed to initialize global hotkey: {error}");
            return None;
        }
    };
    if let Err(error) = manager.register(configured.hotkey) {
        eprintln!(
            "dicta-tray: failed to register hotkey {}: {error}",
            configured.label
        );
        return None;
    }
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        let _ = proxy.send_event(UserEvent::Hotkey {
            id: event.id,
            state: event.state,
        });
    }));
    eprintln!("dicta-tray: registered hotkey {}", configured.label);
    Some(HotkeyRegistration {
        _manager: manager,
        _hotkey: configured.hotkey,
    })
}

fn handle_panel_message(
    message: PanelMessage,
    app: &mut TrayApp,
    panel: Option<&mut Panel>,
) -> bool {
    match message.action.as_str() {
        ACTION_SET_PROVIDER => {
            if let Some(name) = message.provider {
                app.set_provider(&name);
            }
        }
        ACTION_START_LIVE => app.start_live(),
        ACTION_STOP_LIVE => app.stop_live(),
        ACTION_RESTART_LIVE => app.restart_live(),
        ACTION_REFRESH => app.refresh(),
        ACTION_HIDE_PANEL => {
            if let Some(panel) = panel {
                hide_panel(panel);
            }
        }
        ACTION_QUIT => {
            app.shutdown_live_worker();
            return true;
        }
        _ => {}
    }
    false
}

fn update_control_flow(app: &TrayApp, control_flow: &mut ControlFlow) {
    if app.live_child.is_some() {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(2));
    } else {
        *control_flow = ControlFlow::Wait;
    }
}

fn create_tray_icon(app: &mut TrayApp) -> Result<TrayIcon> {
    let icon = tray_icon_image()?;
    let menu = app.build_menu()?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_menu_on_right_click(true)
        .with_tooltip(app.tooltip())
        .with_title(app.tray_title())
        .build()
        .context("tray icon build failed")
}

fn configure_live_worker_stdio(command: &mut Command, provider: &str) -> Result<PathBuf> {
    let path = live_worker_log_path(provider);
    let stderr = open_live_worker_log(&path)?;
    command.stderr(Stdio::from(stderr));
    Ok(path)
}

#[cfg(unix)]
fn configure_live_worker_process(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_live_worker_process(_command: &mut Command) {}

fn wait_live_worker_exit(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(LIVE_WORKER_SHUTDOWN_POLL);
    }
}

#[cfg(unix)]
fn request_live_worker_shutdown(child: &mut Child) -> std::io::Result<()> {
    signal_live_worker_process_group(child, libc::SIGINT)
}

#[cfg(not(unix))]
fn request_live_worker_shutdown(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

#[cfg(unix)]
fn force_live_worker_shutdown(child: &mut Child) -> std::io::Result<()> {
    signal_live_worker_process_group(child, libc::SIGKILL)
}

#[cfg(not(unix))]
fn force_live_worker_shutdown(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

#[cfg(unix)]
fn signal_live_worker_process_group(child: &mut Child, signal: libc::c_int) -> std::io::Result<()> {
    let pid = child.id() as libc::pid_t;
    if pid <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "live worker pid is unavailable",
        ));
    }
    let result = unsafe { libc::kill(-pid, signal) };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn open_live_worker_log(path: &PathBuf) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open live worker log {}", path.display()))
}

fn live_worker_log_path(provider: &str) -> PathBuf {
    let provider = sanitize_log_name(provider);
    std::env::temp_dir().join(format!(
        "dicta-tray-live-{provider}-{}.log",
        std::process::id()
    ))
}

fn sanitize_log_name(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "provider".to_owned()
    } else {
        sanitized
    }
}

fn update_tray_icon(icon: &TrayIcon, app: &mut TrayApp) -> Result<()> {
    icon.set_title(Some(app.tray_title()));
    icon.set_tooltip(Some(app.tooltip()))?;
    icon.set_menu(Some(Box::new(app.build_menu()?)));
    Ok(())
}

fn refresh_shell(app: &mut TrayApp, tray_icon: Option<&TrayIcon>, panel: Option<&Panel>) {
    if let Some(icon) = tray_icon
        && let Err(error) = update_tray_icon(icon, app)
    {
        eprintln!("dicta-tray: failed to update tray icon: {error}");
    }
    if let Some(panel) = panel
        && let Err(error) = update_panel(panel, app)
    {
        eprintln!("dicta-tray: failed to update panel UI: {error}");
    }
}

fn create_panel(
    target: &tao::event_loop::EventLoopWindowTarget<UserEvent>,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
    app: &TrayApp,
) -> Result<Panel> {
    let window_builder = WindowBuilder::new()
        .with_title("dicta")
        .with_visible(false)
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top(true)
        .with_focused(false)
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_inner_size(LogicalSize::new(PANEL_WIDTH, PANEL_HEIGHT));
    #[cfg(target_os = "macos")]
    let window_builder = window_builder.with_has_shadow(false);
    let window = window_builder
        .build(target)
        .context("panel window build failed")?;
    #[cfg(target_os = "macos")]
    {
        window.set_has_shadow(false);
        window.set_background_color(Some((0, 0, 0, 0)));
    }
    let native_glass_requested = app.config.native_glass && native_panel_glass_available();
    let html = render_panel_html(app, native_glass_requested)?;
    let builder = WebViewBuilder::new()
        .with_transparent(true)
        .with_html(html)
        .with_background_color((0, 0, 0, 0))
        .with_ipc_handler(move |request| {
            if let Ok(message) = serde_json::from_str::<PanelMessage>(request.body()) {
                let _ = proxy.send_event(UserEvent::Panel(message));
            }
        });
    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    ))]
    let webview = builder
        .build(&window)
        .context("panel webview build failed")?;
    #[cfg(target_os = "macos")]
    let native_glass =
        native_glass_requested && native_glass::install_panel_glass(&window, &webview);
    #[cfg(target_os = "macos")]
    if native_glass {
        eprintln!("dicta-tray: using macOS native glass panel background");
    } else if native_glass_requested {
        let _ = webview.evaluate_script("window.__dictaNativeGlass = false; document.querySelector('.shell')?.classList.remove('native-glass');");
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    let webview = {
        let vbox = window
            .default_vbox()
            .context("panel GTK vbox unavailable")?;
        builder
            .build_gtk(vbox)
            .context("panel GTK webview build failed")?
    };
    Ok(Panel {
        window,
        webview,
        visible: false,
    })
}

fn update_panel(panel: &Panel, app: &TrayApp) -> Result<()> {
    let state =
        serde_json::to_string(&app.panel_state()).context("panel state serialize failed")?;
    let script = format!("window.__dictaUpdate({state});");
    panel
        .webview
        .evaluate_script(&script)
        .context("panel state update failed")
}

fn toggle_panel(panel: &mut Panel, event: &TrayIconEvent) {
    if panel.visible {
        hide_panel(panel);
    } else {
        show_panel(panel, event);
    }
}

fn show_panel(panel: &mut Panel, event: &TrayIconEvent) {
    let position = panel_position(panel, event);
    panel.window.set_outer_position(position);
    panel.window.set_visible(true);
    panel.window.set_focus();
    panel.visible = true;
}

fn hide_panel(panel: &mut Panel) {
    panel.window.set_visible(false);
    panel.visible = false;
}

fn tray_click_opens_panel(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

fn panel_position(panel: &Panel, event: &TrayIconEvent) -> PhysicalPosition<i32> {
    let scale = panel.window.scale_factor();
    let panel_size = LogicalSize::new(PANEL_WIDTH, PANEL_HEIGHT).to_physical::<i32>(scale);
    let anchor = tray_anchor(event).unwrap_or_else(|| {
        panel
            .window
            .cursor_position()
            .map(|position| position.cast())
            .unwrap_or_else(|_| fallback_anchor(panel, panel_size))
    });
    let (min_x, min_y, max_x, max_y) = monitor_bounds(panel, panel_size);
    let x = (anchor.x - panel_size.width + 18).clamp(min_x, max_x);
    let y = (anchor.y + 10).clamp(min_y, max_y);
    PhysicalPosition::new(x, y)
}

fn tray_anchor(event: &TrayIconEvent) -> Option<PhysicalPosition<i32>> {
    match event {
        TrayIconEvent::Click { rect, position, .. }
        | TrayIconEvent::DoubleClick { rect, position, .. }
        | TrayIconEvent::Enter { rect, position, .. }
        | TrayIconEvent::Move { rect, position, .. }
        | TrayIconEvent::Leave { rect, position, .. } => Some(anchor_from_rect(*rect, *position)),
        _ => None,
    }
}

fn anchor_from_rect(rect: Rect, position: PhysicalPosition<f64>) -> PhysicalPosition<i32> {
    if rect.size.width > 0 && rect.size.height > 0 {
        PhysicalPosition::new(
            (rect.position.x + f64::from(rect.size.width)).round() as i32,
            (rect.position.y + f64::from(rect.size.height)).round() as i32,
        )
    } else {
        position.cast()
    }
}

fn fallback_anchor(panel: &Panel, panel_size: PhysicalSize<i32>) -> PhysicalPosition<i32> {
    let (min_x, min_y, max_x, _) = monitor_bounds(panel, panel_size);
    PhysicalPosition::new(max_x.max(min_x), min_y)
}

fn monitor_bounds(panel: &Panel, panel_size: PhysicalSize<i32>) -> (i32, i32, i32, i32) {
    let Some(monitor) = panel
        .window
        .current_monitor()
        .or_else(|| panel.window.available_monitors().next())
    else {
        return (
            0,
            0,
            i32::MAX - panel_size.width,
            i32::MAX - panel_size.height,
        );
    };
    let position = monitor.position();
    let size = monitor.size();
    let padding = 8;
    let min_x = position.x + padding;
    let min_y = position.y + padding;
    let max_x = position.x + size.width as i32 - panel_size.width - padding;
    let max_y = position.y + size.height as i32 - panel_size.height - padding;
    (min_x, min_y, max_x.max(min_x), max_y.max(min_y))
}

fn tray_icon_image() -> Result<Icon> {
    let size = 32;
    let mut rgba = vec![0; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let in_capsule = (11..=20).contains(&x)
                && (5..=18).contains(&y)
                && rounded_rect_contains(x, y, 11, 5, 10, 14, 5);
            let in_stem = (15..=16).contains(&x) && (19..=24).contains(&y);
            let in_base = (9..=22).contains(&x) && (25..=27).contains(&y);
            let in_left_wave = (7..=8).contains(&x) && (12..=18).contains(&y);
            let in_right_wave = (23..=24).contains(&x) && (12..=18).contains(&y);
            if in_capsule || in_stem || in_base || in_left_wave || in_right_wave {
                set_icon_pixel(&mut rgba, size, x, y, 255);
            }
        }
    }
    Icon::from_rgba(rgba, size as u32, size as u32).context("failed to build tray icon image")
}

fn rounded_rect_contains(
    x: usize,
    y: usize,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    radius: usize,
) -> bool {
    let right = left + width - 1;
    let bottom = top + height - 1;
    let cx = if x < left + radius {
        left + radius
    } else if x > right - radius {
        right - radius
    } else {
        x
    };
    let cy = if y < top + radius {
        top + radius
    } else if y > bottom - radius {
        bottom - radius
    } else {
        y
    };
    let dx = x.abs_diff(cx);
    let dy = y.abs_diff(cy);
    dx * dx + dy * dy <= radius * radius
}

fn set_icon_pixel(rgba: &mut [u8], size: usize, x: usize, y: usize, alpha: u8) {
    let index = (y * size + x) * 4;
    rgba[index] = 0;
    rgba[index + 1] = 0;
    rgba[index + 2] = 0;
    rgba[index + 3] = alpha;
}

const PANEL_HTML: &str = include_str!("../assets/panel.html");
const PANEL_CSS: &str = include_str!("../assets/panel.css");
const PANEL_JS: &str = include_str!("../assets/panel.js");

fn render_panel_html(app: &TrayApp, native_glass: bool) -> Result<String> {
    let state =
        serde_json::to_string(&app.panel_state()).context("panel state serialize failed")?;
    let state = state.replace('<', "\\u003c");
    Ok(PANEL_HTML
        .replace("__DICTA_PANEL_CSS__", PANEL_CSS)
        .replace("__DICTA_PANEL_JS__", PANEL_JS)
        .replace(
            "__DICTA_NATIVE_GLASS__",
            if native_glass { "true" } else { "false" },
        )
        .replace("__DICTA_INITIAL_STATE__", &state))
}

#[cfg(target_os = "macos")]
fn native_panel_glass_available() -> bool {
    native_glass::panel_glass_available()
}

#[cfg(not(target_os = "macos"))]
fn native_panel_glass_available() -> bool {
    false
}

fn provider_label(provider: &ProviderListEntry) -> String {
    let mut label = format!("{} ({}, {})", provider.name, provider.kind, provider.model);
    if !provider.live && !provider.ptt {
        label.push_str(" - no realtime");
    } else if !provider.local_config_ok {
        match &provider.local_config_error {
            Some(error) => label.push_str(&format!(" - {error}")),
            None => label.push_str(" - config needed"),
        }
    } else if provider.ptt {
        label.push_str(" - PTT");
    }
    label
}

fn provider_visible_in_tray(provider: &ProviderListEntry) -> bool {
    provider.live || provider.ptt
}

fn selected_provider_mode(
    provider: &ProviderListEntry,
    preference: ActivationPreference,
) -> Option<WorkerMode> {
    match preference {
        ActivationPreference::Ptt if provider.ptt => Some(WorkerMode::Ptt),
        ActivationPreference::Live if provider.live => Some(WorkerMode::Live),
        ActivationPreference::Ptt | ActivationPreference::Live => None,
        ActivationPreference::Auto if provider.ptt => Some(WorkerMode::Ptt),
        ActivationPreference::Auto if provider.live => Some(WorkerMode::Live),
        ActivationPreference::Auto => None,
    }
}

fn parse_hotkey_config(value: &str) -> Result<Option<ConfiguredHotkey>> {
    let value = value.trim();
    if value.is_empty()
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "none" | "off" | "false" | "disabled"
        )
    {
        return Ok(None);
    }
    let hotkey: HotKey = value
        .parse()
        .with_context(|| format!("invalid hotkey `{value}`"))?;
    Ok(Some(ConfiguredHotkey {
        hotkey,
        label: hotkey.to_string(),
    }))
}

fn filter_provider_report_for_tray(mut report: ProviderListReport) -> ProviderListReport {
    report.providers.retain(provider_visible_in_tray);
    report
}

#[cfg(test)]
fn report_current_switchable(report: &ProviderListReport) -> bool {
    report_current_switchable_for(report, ActivationPreference::Auto)
}

fn report_current_switchable_for(
    report: &ProviderListReport,
    preference: ActivationPreference,
) -> bool {
    let Some(current) = report.current.as_deref() else {
        return false;
    };
    report.providers.iter().any(|provider| {
        provider.name == current
            && provider.local_config_ok
            && selected_provider_mode(provider, preference).is_some()
    })
}

#[cfg(test)]
fn first_switchable_provider(report: &ProviderListReport) -> Option<&str> {
    first_switchable_provider_for(report, ActivationPreference::Auto)
}

fn first_switchable_provider_for(
    report: &ProviderListReport,
    preference: ActivationPreference,
) -> Option<&str> {
    report
        .providers
        .iter()
        .find(|provider| {
            provider.local_config_ok && selected_provider_mode(provider, preference).is_some()
        })
        .map(|provider| provider.name.as_str())
}

fn unavailable_provider_status(provider: &ProviderListEntry) -> String {
    if !provider.live && !provider.ptt {
        format!("{} does not support realtime mode", provider.name)
    } else {
        provider
            .local_config_error
            .clone()
            .unwrap_or_else(|| format!("{} is not ready", provider.name))
    }
}

fn command_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_owned();
    if text.is_empty() {
        "command exited with an error".to_owned()
    } else {
        text
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .and_then(|value| (!value.trim().is_empty()).then_some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_provider_is_switchable_only_when_live_ready() {
        let report = provider_report(Some("live-asr"));

        assert!(report_current_switchable(&report));
    }

    #[test]
    fn batch_only_current_provider_is_not_switchable() {
        let report = provider_report(Some("openai"));

        assert!(!report_current_switchable(&report));
        assert_eq!(first_switchable_provider(&report), Some("live-asr"));
    }

    #[test]
    fn missing_current_provider_falls_back_to_first_switchable_provider() {
        let report = provider_report(None);

        assert!(!report_current_switchable(&report));
        assert_eq!(first_switchable_provider(&report), Some("live-asr"));
    }

    #[test]
    fn tray_hides_batch_only_providers() {
        let report = filter_provider_report_for_tray(provider_report(Some("openai")));

        assert!(
            report
                .providers
                .iter()
                .any(|provider| provider.name == "apple")
        );
        assert!(
            report
                .providers
                .iter()
                .any(|provider| provider.name == "live-asr")
        );
        assert!(
            !report
                .providers
                .iter()
                .any(|provider| provider.name == "openai")
        );
        assert_eq!(first_switchable_provider(&report), Some("live-asr"));
    }

    #[test]
    fn auto_activation_prefers_ptt_provider() {
        let ptt = provider("ptt-asr", true, true).with_ptt();
        assert_eq!(
            selected_provider_mode(&ptt, ActivationPreference::Auto),
            Some(WorkerMode::Ptt)
        );
        assert_eq!(
            selected_provider_mode(&ptt, ActivationPreference::Live),
            Some(WorkerMode::Live)
        );
    }

    #[test]
    fn ptt_activation_skips_live_only_provider() {
        let report = provider_report(Some("live-asr"));

        assert!(!report_current_switchable_for(
            &report,
            ActivationPreference::Ptt
        ));
        assert_eq!(
            first_switchable_provider_for(&report, ActivationPreference::Ptt),
            None
        );
    }

    #[test]
    fn hotkey_config_parses_or_disables_shortcut() {
        let parsed = parse_hotkey_config("ctrl+alt+space").unwrap().unwrap();
        assert_eq!(parsed.label, "control+alt+Space");

        assert!(parse_hotkey_config("off").unwrap().is_none());
        assert!(parse_hotkey_config("").unwrap().is_none());
        assert!(parse_hotkey_config("ctrl+space+alt").is_err());
    }

    #[test]
    fn live_worker_keeps_stdout_visible_and_logs_stderr() {
        let provider = format!(
            "stdio-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let log_path = live_worker_log_path(&provider);
        let _ = std::fs::remove_file(&log_path);

        let mut command = shell_echo_command("visible-output", "hidden-error");
        let configured_log = configure_live_worker_stdio(&mut command, &provider).unwrap();
        let output = command.output().unwrap();

        assert!(output.status.success());
        assert_eq!(configured_log, log_path);
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
            "visible-output"
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            normalize_newlines(&std::fs::read_to_string(&configured_log).unwrap()),
            "hidden-error"
        );

        let _ = std::fs::remove_file(configured_log);
    }

    #[cfg(unix)]
    #[test]
    fn live_worker_shutdown_uses_sigint_process_group() {
        use std::os::unix::process::ExitStatusExt;

        let mut command = std::process::Command::new("sleep");
        command
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_live_worker_process(&mut command);
        let mut child = command.spawn().unwrap();

        request_live_worker_shutdown(&mut child).unwrap();
        let status = wait_live_worker_exit(&mut child, Duration::from_secs(5))
            .unwrap()
            .unwrap_or_else(|| {
                let _ = force_live_worker_shutdown(&mut child);
                child.wait().unwrap()
            });

        assert_eq!(status.signal(), Some(libc::SIGINT), "got {status}");
    }

    #[cfg(unix)]
    fn shell_echo_command(stdout: &str, stderr: &str) -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg("printf '%s' \"$1\"; printf '%s' \"$2\" >&2");
        command.arg("sh").arg(stdout).arg(stderr);
        command
    }

    #[cfg(windows)]
    fn shell_echo_command(_stdout: &str, _stderr: &str) -> std::process::Command {
        let mut command = std::process::Command::new("cmd");
        command
            .arg("/C")
            .arg("echo visible-output& 1>&2 echo hidden-error");
        command
    }

    fn normalize_newlines(value: &str) -> String {
        value
            .replace("\r\n", "\n")
            .trim_end_matches('\n')
            .to_owned()
    }

    fn provider_report(current: Option<&str>) -> ProviderListReport {
        ProviderListReport {
            current: current.map(ToOwned::to_owned),
            providers: vec![
                provider("apple", true, false),
                provider("live-asr", true, true),
                provider("openai", false, true),
            ],
        }
    }

    fn provider(name: &str, live: bool, local_config_ok: bool) -> ProviderListEntry {
        ProviderListEntry {
            name: name.to_owned(),
            kind: name.to_owned(),
            selected: false,
            live,
            ptt: false,
            local_config_ok,
            local_config_error: None,
            model: name.to_owned(),
        }
    }

    trait ProviderListEntryTestExt {
        fn with_ptt(self) -> Self;
    }

    impl ProviderListEntryTestExt for ProviderListEntry {
        fn with_ptt(mut self) -> Self {
            self.ptt = true;
            self
        }
    }
}
