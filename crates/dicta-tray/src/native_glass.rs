use objc2::runtime::AnyClass;
use objc2::{ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSGlassEffectView, NSGlassEffectViewStyle, NSView,
};
use objc2_web_kit::WKWebView;
use tao::platform::macos::WindowExtMacOS;
use tao::window::Window;
use wry::{WebView, WebViewExtMacOS};

const PANEL_RADIUS: f64 = 28.0;

pub fn install_panel_glass(window: &Window, webview: &WebView) -> bool {
    if !panel_glass_available() {
        return false;
    }

    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };

    let ns_window = webview.ns_window();
    let Some(content_view) = ns_window.contentView() else {
        return false;
    };

    let frame = content_view.bounds();
    let glass = NSGlassEffectView::initWithFrame(mtm.alloc(), frame);
    glass.setAutoresizingMask(fill_mask());
    glass.setCornerRadius(PANEL_RADIUS);
    glass.setStyle(NSGlassEffectViewStyle::Regular);
    let tint = NSColor::colorWithSRGBRed_green_blue_alpha(0.78, 0.91, 1.0, 0.22);
    glass.setTintColor(Some(&tint));

    let wry_webview = webview.webview();
    let wk_webview: &WKWebView = wry_webview.as_super();
    let webview_view: &NSView = wk_webview.as_super();
    webview_view.setFrame(frame);
    webview_view.setAutoresizingMask(fill_mask());

    glass.setContentView(Some(webview_view));
    ns_window.setContentView(Some(&glass));

    window.set_has_shadow(false);
    window.set_background_color(Some((0, 0, 0, 0)));
    true
}

pub fn panel_glass_available() -> bool {
    if !macos_major_at_least(26) {
        return false;
    }
    AnyClass::get(c"NSGlassEffectView").is_some()
}

fn macos_major_at_least(required_major: u32) -> bool {
    let Ok(output) = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
    else {
        return false;
    };
    let version = String::from_utf8_lossy(&output.stdout);
    let Some(major) = version.trim().split('.').next() else {
        return false;
    };
    major
        .parse::<u32>()
        .is_ok_and(|major| major >= required_major)
}

fn fill_mask() -> NSAutoresizingMaskOptions {
    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable
}
