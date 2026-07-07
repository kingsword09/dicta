    const send = (message) => {
      if (window.ipc?.postMessage) {
        window.ipc.postMessage(JSON.stringify(message));
      }
    };

    const providers = document.getElementById('providers');
    const current = document.getElementById('current');
    const status = document.getElementById('status');
    const liveChip = document.getElementById('live-chip');
    const startStop = document.getElementById('start-stop');
    const restart = document.getElementById('restart');
    const shell = document.querySelector('.shell');
    const providersFrame = providers.closest('.providers-frame');

    if (window.__dictaNativeGlass && shell) shell.classList.add('native-glass');

    let switchingProvider = null;

    const setText = (node, value) => {
      node.textContent = value || '';
      node.title = value || '';
    };

    const providerSwitchable = (provider) => (provider.live || provider.ptt) && provider.local_config_ok;

    const providerReason = (provider) => {
      if (!provider.live && !provider.ptt) return `${provider.name} does not support realtime mode`;
      if (!provider.local_config_ok) return provider.local_config_error || `${provider.name} needs configuration`;
      return `Switch to ${provider.name}`;
    };

    const unavailableBadge = (provider) => {
      if (!provider.live && !provider.ptt) return 'No Realtime';
      if (!provider.local_config_ok) return 'Not Ready';
      return 'Unavailable';
    };

    const providerBadge = (provider, liveRunning) => {
      if (provider.selected && liveRunning) return ['Active', 'ready'];
      if (provider.selected) return ['Selected', 'warn'];
      return [provider.ptt ? 'PTT' : 'Ready', 'ready'];
    };

    const updateProviderScrollState = () => {
      if (!providersFrame) return;
      const scrollable = providers.scrollHeight > providers.clientHeight + 1;
      const canScrollUp = providers.scrollTop > 1;
      const canScrollDown = providers.scrollTop + providers.clientHeight < providers.scrollHeight - 1;
      providersFrame.classList.toggle('scrollable', scrollable);
      providersFrame.classList.toggle('can-scroll-up', scrollable && canScrollUp);
      providersFrame.classList.toggle('can-scroll-down', scrollable && canScrollDown);
    };

    providers.addEventListener('scroll', updateProviderScrollState, { passive: true });
    window.addEventListener('resize', updateProviderScrollState);

    const sendProviderSwitch = (provider) => {
      if (!providerSwitchable(provider) || switchingProvider) return;
      switchingProvider = provider.name;
      setText(status, `Switching to ${provider.name}`);
      renderProviders(window.__dictaState || { providers: [] });
      send({ action: 'set_provider', provider: provider.name });
    };

    const sendPanelAction = (action, pendingStatus) => {
      if (pendingStatus) setText(status, pendingStatus);
      send({ action });
    };

    const renderProviders = (state) => {
      providers.replaceChildren();
      if (!state.providers.length) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = 'No realtime providers found';
        providers.append(empty);
        return;
      }

      for (const provider of state.providers) {
        const switchable = providerSwitchable(provider);
        const pending = switchingProvider === provider.name;
        const row = document.createElement('button');
        row.type = 'button';
        row.className = `provider${provider.selected ? ' selected' : ''}${pending ? ' pending' : ''}${switchable ? '' : ' unavailable'}`;
        row.disabled = !switchable || (switchingProvider !== null && !pending);
        row.setAttribute('aria-pressed', provider.selected ? 'true' : 'false');
        row.setAttribute('aria-label', providerReason(provider));
        row.title = providerReason(provider);
        row.addEventListener('click', () => sendProviderSwitch(provider));

        const mark = document.createElement('span');
        mark.className = 'mark';
        mark.setAttribute('aria-hidden', 'true');

        const main = document.createElement('span');
        main.className = 'provider-main';

        const name = document.createElement('span');
        name.className = 'provider-name';
        setText(name, provider.name);

        const meta = document.createElement('span');
        meta.className = 'provider-meta';
        setText(meta, `${provider.kind} / ${provider.model}`);

        main.append(name, meta);
        row.append(mark, main);

        const [badgeText, badgeKind] = switchable
          ? providerBadge(provider, state.live_running)
          : [unavailableBadge(provider), 'bad'];
        const badge = document.createElement('span');
        badge.className = `badge ${badgeKind}`;
        setText(badge, badgeText);
        row.append(badge);

        if (!switchable) {
          const errorMsg = document.createElement('span');
          errorMsg.className = 'provider-error';
          setText(errorMsg, providerReason(provider));
          main.append(errorMsg);
        }

        providers.append(row);
      }
      requestAnimationFrame(updateProviderScrollState);
    };

    window.__dictaUpdate = (state) => {
      window.__dictaState = state;
      switchingProvider = null;
      setText(current, state.current ? state.current : 'No provider');
      setText(status, state.hotkey ? `${state.status} / ${state.hotkey}` : state.status);
      liveChip.className = state.live_running ? 'chip live' : 'chip';
      if (state.ptt_recording) {
        liveChip.textContent = 'PTT';
      } else if (state.live_running && state.worker_mode === 'ptt') {
        liveChip.textContent = 'Ready';
      } else {
        liveChip.textContent = state.live_running ? 'Live' : 'Idle';
      }

      renderProviders(state);

      const playIcon = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round" class="btn-icon"><polygon points="6 4 20 12 6 20 6 4"></polygon></svg>';
      const stopIcon = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round" class="btn-icon"><rect x="5" y="5" width="14" height="14" rx="1.5" ry="1.5"></rect></svg>';
      const restartIcon = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round" class="btn-icon"><path d="M21 3v6h-6"></path><path d="M20.5 14.5a8.5 8.5 0 1 1-2.2-8.1L21 9"></path></svg>';

      const pttSelected = state.selected_ptt || state.worker_mode === 'ptt';
      const recording = Boolean(state.ptt_recording);
      const canStart = state.selected_ready && (!state.live_running || (state.worker_mode === 'ptt' && !recording));
      const canStop = state.live_running && (state.worker_mode !== 'ptt' || recording);
      const stopActive = state.live_running && (!pttSelected || recording);
      startStop.innerHTML = stopActive
        ? (stopIcon + `<span>${pttSelected ? 'Stop PTT' : 'Stop Live'}</span>`)
        : (playIcon + `<span>${pttSelected ? 'Start PTT' : 'Start Live'}</span>`);
      startStop.className = stopActive ? 'control danger' : 'control primary';
      startStop.disabled = switchingProvider !== null || (stopActive ? !canStop : !canStart);
      startStop.onclick = () => sendPanelAction(
        stopActive ? 'stop_live' : 'start_live',
        stopActive ? (pttSelected ? 'Stopping PTT' : 'Stopping live') : (pttSelected ? 'Starting PTT' : 'Starting live')
      );

      restart.style.display = state.live_running ? 'inline-flex' : 'none';
      restart.innerHTML = restartIcon + '<span>Restart</span>';
      restart.disabled = switchingProvider !== null || !state.selected_ready;
      restart.onclick = () => sendPanelAction('restart_live', 'Restarting live');
    };

    document.addEventListener('click', (event) => {
      const action = event.target?.closest('[data-action]')?.dataset?.action;
      if (action) send({ action });
    });

    document.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') send({ action: 'hide_panel' });
    });

    window.__dictaUpdate(__DICTA_INITIAL_STATE__);
