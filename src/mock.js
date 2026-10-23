// Mock backend for checking the UI in a plain browser (loaded by common.js only when
// window.__TAURI__ is missing). Returns the sample data from docs/design/*.dc.html for every
// command and simulates the events. Query params: ?state=empty|loading|problems|compact|update,
// ?os=windows, ?rb=local|local-key|network|vestaboard|error|none (Rattleboard: connected that way,
// with a failed send, or not installed), plus ?view= and ?theme= (handled by the UI).
(() => {
  const q = new URLSearchParams(location.search);
  const scenario = q.get('state') || '';
  const now = Date.now();
  const M = 60000, H = 60 * M, D = 24 * H;
  const listeners = {};
  // Cloned, as Tauri serialises events: the UI never shares objects with the mock.
  const emit = (event, payload) => (listeners[event] || []).forEach((cb) => cb({ event, payload: structuredClone(payload) }));
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));

  // Next local Monday 09:00, the 1st of next month etc., so local times read like the design.
  const at = (offset) => now + offset;
  const win = (id, label, abs, pct, resetIn, pace, proj, status, forecast, outIn) => ({
    id, label, abs, pct, pace, proj, resetsAt: resetIn == null ? null : at(resetIn), status, forecast, outAt: outIn == null ? null : at(outIn),
  });

  const rbDefault = { mode: 'off', url: '', name: '', rows: 6, cols: 22, kind: '', target: '', alerts: 'alerts', card: false };
  const settings = {
    menubarLabel: 'runway', density: scenario === 'compact' ? 'compact' : 'comfortable', warnAt: 75, critAt: 90, leadMinutes: 60,
    notifyThresholds: true, notifyProjection: true, notifyReset: false, pausedUntil: 0, requestAt: 85,
    admins: { 'acct-1': { name: 'Jordan Lee', email: 'jordan@iokig.com', slack: '@Jordan Lee', teams: '' } },
    myName: 'Sam', updateCheck: true, startAtLogin: false,
    rattleboard: { ...rbDefault },
  };
  const rbq = q.get('rb') || '';
  const hhmm = () => new Date().toTimeString().slice(0, 5);

  const claude = () => ({
    id: 'acct-1', provider: 'anthropic', name: 'Claude — work', plan: 'Enterprise', mono: 'C', updatedAt: at(-12000), state: 'ok', problem: null, method: 'cli',
    windows: [
      win('5h', '5-hour', '412k / 530k tokens', 78, 2 * H + 41 * M, 46, 130, 'crit', 'Out at 13:05, 1h 15m before reset', 1 * H + 26 * M),
      win('weekly', 'Weekly', null, 64, 2 * D + 21 * H, 59, 79, 'ok', 'On track'),
      win('weekly-opus', 'Weekly Opus', null, 83, 2 * D + 21 * H, 59, 97, 'warn', 'Tight · ~97% at reset'),
    ],
  });
  const chatgpt = () => ({
    id: 'acct-2', provider: 'openai', name: 'ChatGPT — personal', plan: 'Pro', mono: 'O', updatedAt: at(-48000), state: 'ok', problem: null, method: 'cli',
    windows: [
      win('5h', 'Codex 5-hour', null, 12, 3 * H + 52 * M, 23, 52, 'ok', 'On track'),
      win('weekly', 'Codex weekly', null, 31, 4 * D + 6 * H, 39, 79, 'ok', 'On track'),
    ],
  });
  const bedrock = () => ({
    id: 'acct-3', provider: 'bedrock', name: 'AWS Bedrock — iokig-prod', plan: 'On-demand', mono: 'B', updatedAt: at(-4 * M), state: 'ok', problem: null, method: 'aws',
    windows: [win('monthly', 'Monthly budget', '$184 / $500', 37, 8 * D + 12 * H, 72, 51, 'est', 'Rough estimate · ~$255')],
  });
  const detected = [
    { kind: 'claude-code', label: 'Claude Code login', detail: 'sam@iokig.com', provider: 'anthropic', mono: 'C' },
    { kind: 'codex', label: 'Codex CLI login', detail: '', provider: 'openai', mono: 'O' },
    { kind: 'aws', label: 'AWS profiles', detail: '2 in ~/.aws', provider: 'bedrock', mono: 'B' },
  ];

  let app = {
    version: '0.1.0', os: q.get('os') === 'windows' ? 'windows' : 'mac', update: null, updateHow: 'install', loading: false,
    summary: { label: '1h 26m', status: 'crit', window: 'Claude — work · 5-hour', crit: 1, warn: 1, ok: 4 },
    accounts: [claude(), chatgpt(), bedrock()], detected: [], settings, rattleboardStatus: null, rattleboardKey: false,
  };
  if (['local', 'local-key', 'network', 'vestaboard', 'error'].includes(rbq)) {
    const local = rbq.startsWith('local');
    app.settings.rattleboard = {
      ...rbDefault, mode: local ? 'local' : 'network', url: local ? 'http://127.0.0.1:7070' : 'http://192.168.1.20:7070',
      name: local ? 'This computer' : rbq === 'vestaboard' ? 'Vestaboard' : 'Living room TV', kind: rbq === 'vestaboard' ? 'vestaboard' : 'rattleboard',
      card: rbq === 'network' || rbq === 'error',
    };
    app.rattleboardKey = rbq !== 'local';
    if (rbq === 'network') app.rattleboardStatus = `Added the AI runway card to the rotation at ${hhmm()}`;
    if (rbq === 'error') app.rattleboardStatus = `Couldn't send an alert at ${hhmm()}: Couldn't connect to 192.168.1.20:7070. Is the board on, with its API server turned on?`;
  }

  if (scenario === 'empty') Object.assign(app, { summary: null, accounts: [], detected });
  if (scenario === 'loading') {
    app.loading = true;
    app.summary = null;
    app.accounts = app.accounts.map((a) => ({ ...a, state: 'loading', updatedAt: null, windows: [] }));
    setTimeout(() => { app = { ...app, loading: false, summary: { label: '1h 26m', status: 'crit', window: 'Claude — work · 5-hour', crit: 1, warn: 1, ok: 4 }, accounts: [claude(), chatgpt(), bedrock()] }; emit('tr://state', app); }, +(q.get('loadms') || 600000));
  }
  if (scenario === 'problems') {
    app.update = '0.2.0';
    app.summary = null;
    app.accounts = [
      { ...claude(), state: 'stale', updatedAt: at(-38 * M), problem: "Couldn't reach claude.ai from this network.",
        windows: [win('5h', '5-hour', null, 61, 2 * H + 41 * M, 46, null, 'est', '')] },
      { ...chatgpt(), state: 'login', problem: 'The Codex CLI session expired. Run `codex login`, or sign in here.', windows: [] },
      { id: 'acct-4', provider: 'gemini', name: 'Gemini — personal', plan: 'AI Pro', mono: 'G', updatedAt: at(-3 * H), state: 'broken', method: 'login',
        problem: 'Google changed the usage endpoint this reader uses. v0.2.0 ships a fixed reader.', windows: [] },
    ];
  }
  if (scenario === 'compact') {
    app.summary = { label: '2d 21h', status: 'ok', window: 'Claude — work · Weekly', crit: 0, warn: 0, ok: 7 };
    app.accounts = [
      { ...claude(), updatedAt: at(-9000), windows: [
        win('5h', '5-hour', null, 18, 4 * H + 13 * M, 30, 40, 'ok', 'On track'),
        win('weekly', 'Weekly', null, 52, 2 * D + 21 * H, 59, 70, 'ok', 'On track'),
        win('weekly-opus', 'Weekly Opus', null, 47, 2 * D + 21 * H, 59, 66, 'ok', 'On track')] },
      { ...chatgpt(), updatedAt: at(-41000) },
      { ...bedrock(), updatedAt: at(-3 * M), windows: [win('monthly', 'Monthly $', '$184 / $500', 37, 8 * D + 12 * H, 72, 51, 'ok', 'On track')] },
      { id: 'acct-5', provider: 'cursor', name: 'Cursor — work', plan: 'Pro', mono: 'Cu', updatedAt: at(-1 * M), state: 'ok', method: 'login',
        windows: [win('monthly', 'Monthly reqs', '220 / 500', 44, 16 * D + 12 * H, 72, 60, 'ok', 'On track')] },
    ];
  }
  if (scenario === 'update') app.update = '0.2.0';

  // ---------- stats / harness ----------
  const dayIso = (offset) => { const d = new Date(now + offset * D); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`; };
  const week = [12.4, 8.1, 71.3, 88.6, 94.2, 83.9, 54.3];
  const kinds = [['Build', 31], ['Bug fix', 18], ['Discovery', 12], ['Refactor', 10], ['Test', 8], ['Review', 7], ['Docs', 5], ['Ops', 4], ['Planning', 3], ['Chat', 2]];
  const stats = (period) => {
    const f = { today: 0.13, '7d': 1, '30d': 3.9, '90d': 10.5, lifetime: 14.2 }[period] || 1;
    const n = period === 'today' ? 1 : period === '7d' ? 7 : 30;
    const daily = Array.from({ length: n }, (_, i) => {
      const off = i - (n - 1);
      const wd = new Date(now + off * D).getDay();
      const cost = n === 7 ? week[i] : n === 1 ? 54.3 : (wd === 0 || wd === 6 ? 7 + 6 * Math.abs(Math.sin(i * 1.7)) : 46 + 38 * Math.abs(Math.sin(i * 0.9 + 1.3)));
      return { day: dayIso(off), cost, tokens: Math.round(cost * 117000) };
    });
    return {
      period, spendApi: 412.8 * f, spendActual: 46.2 * f, actualNote: 'Bedrock billed',
      tokens: { total: 48.2e6 * f, cache: 31.6e6 * f, input: 12.9e6 * f, output: 3.7e6 * f },
      daily,
      models: [['Opus 5.5', 248.1], ['Sonnet 5.5', 82.4], ['GPT-5 Codex', 61.9], ['Haiku 5.5', 12.0]].map(([name, c]) => ({ name, cost: c * f, tokens: Math.round(c * f * 1e5) })),
      projects: [['third-eye', 151.2], ['tokenrunway', 122.6], ['intake-classifier', 88.3], ['dotfiles', 21.4]].map(([name, c]) => ({ name, cost: c * f, tokens: Math.round(c * f * 1e5) })),
      kinds: kinds.map(([kind, share]) => ({ kind, share })),
    };
  };
  const harness = (period) => {
    const f = { today: 0.13, '7d': 1, '30d': 3.9, '90d': 10.5, lifetime: 14.2 }[period] || 1;
    const row = (id, name, share, cost, tok, sessions, avgMin, cache, model, kind, daily) => ({
      id, name, share, cost: cost * f, tokens: Math.round(tok * f), sessions: Math.round(sessions * f), avgSessionMs: avgMin * M, cacheHit: cache, topModel: model, topKind: kind, daily,
    });
    return {
      period, totalCost: 412.8 * f,
      harnesses: [
        row('claude-code', 'Claude Code', 77, 318.4, 36.1e6, 58, 34, 71, 'Opus 5.5', 'Build', [8, 5, 56, 70, 74, 64, 41]),
        row('codex', 'Codex', 15, 61.9, 7.4e6, 21, 22, 48, 'GPT-5 Codex', 'Bug fix', [3, 2, 10, 12, 13, 13, 9]),
        row('opencode', 'opencode', 6, 24.1, 3.2e6, 9, 18, 52, 'Sonnet 5.5', 'Refactor', [1, 1, 4, 5, 5, 5, 3]),
        row('gemini-cli', 'Gemini CLI', 2, 8.4, 1.5e6, 6, 11, 33, 'Gemini Pro', 'Discovery', [0.4, 0.1, 1.3, 1.6, 2.2, 1.9, 0.9]),
      ],
    };
  };

  // ---------- dashboard ----------
  const dashboard = (period, group, account) => {
    const n = { '7d': 7, '30d': 30, '90d': 90, lifetime: 120 }[period] || 30;
    const names = {
      model: ['Opus 5.5', 'Sonnet 5.5', 'GPT-5 Codex', 'Other'],
      harness: ['Claude Code', 'Codex', 'opencode', 'Gemini CLI'],
      category: ['Build', 'Bug fix', 'Discovery', 'Other'],
    }[group] || ['Opus 5.5', 'Sonnet 5.5', 'GPT-5 Codex', 'Other'];
    const scale = account && account !== 'acct-1' ? 0.3 : 1;
    const days = [], vals = names.map(() => []);
    for (let i = 0; i < n; i++) {
      const off = i - (n - 1);
      const wd = new Date(now + off * D).getDay();
      const weekend = wd === 0 || wd === 6;
      let v = weekend ? 7 + 6 * Math.abs(Math.sin(i * 1.7)) : 46 + 38 * Math.abs(Math.sin(i * 0.9 + 1.3)) + 9 * Math.sin(i * 2.3);
      if (i === n - 1) v = 54.3;
      v *= scale;
      const so = 0.55 + 0.08 * Math.sin(i), ss = 0.2 + 0.05 * Math.cos(i * 1.3), sc = 0.15 + 0.05 * Math.sin(i * 0.7);
      [so, ss, sc, 1 - so - ss - sc].forEach((s, k) => vals[k].push(+(v * s).toFixed(2)));
      days.push(dayIso(off));
    }
    const heatmap = Array.from({ length: 182 }, (_, i) => {
      const off = i - 181;
      const wd = new Date(now + off * D).getDay();
      const r = Math.abs(Math.sin(i * 12.9898) * 43758.5453) % 1;
      const lvl = wd === 0 || wd === 6 ? (r > 0.7 ? 1 : 0) : Math.min(4, Math.floor(r * 3.2) + (i > 120 ? 1 : 0));
      const tokens = lvl === 0 ? 0 : Math.round((lvl * 1.6 + r) * 1e6 * scale);
      return { day: dayIso(off), cost: tokens / 1.2e5, tokens };
    });
    const today = new Date(now); today.setHours(0, 0, 0, 0);
    const t = (dayOff, h, m) => today.getTime() + dayOff * D + h * H + m * M;
    const sessions = [
      [t(0, 11, 2), 'Claude Code', 'third-eye', 'Opus 5.5', 'Build', 37, 2.4e6, 14.8],
      [t(0, 10, 21), 'Claude Code', 'tokenrunway', 'Opus 5.5', 'Bug fix', 41, 3.1e6, 19.6],
      [t(0, 9, 48), 'Codex', 'intake-classifier', 'GPT-5 Codex', 'Test', 18, 0.9e6, 3.1],
      [t(0, 9, 20), 'Claude Code', 'third-eye', 'Sonnet 5.5', 'Refactor', 26, 1.6e6, 4.9],
      [t(-1, 17, 44), 'opencode', 'dotfiles', 'Sonnet 5.5', 'Ops', 12, 0.4e6, 1.2],
      [t(-1, 16, 5), 'Claude Code', 'tokenrunway', 'Opus 5.5', 'Planning', 22, 1.1e6, 7.4],
      [t(-1, 14, 31), 'Gemini CLI', 'intake-classifier', 'Gemini Pro', 'Discovery', 9, 0.3e6, 0.6],
      [t(-2, 15, 12), 'Claude Code', 'third-eye', 'Opus 5.5', 'Build', 48, 3.6e6, 22.1],
      [t(-2, 11, 40), 'Codex', 'tokenrunway', 'GPT-5 Codex', 'Review', 15, 0.7e6, 2.4],
      [t(-3, 16, 2), 'Claude Code', 'intake-classifier', 'Opus 5.5', 'Bug fix', 33, 2.2e6, 13.5],
      [t(-3, 10, 18), 'Claude Code', 'dotfiles', 'Haiku 5.5', 'Docs', 8, 0.2e6, 0.3],
    ].map(([started, harness, project, model, kind, mins, tokens, cost]) => ({ started, harness, project, model, kind, durationMs: mins * M, tokens, cost }));
    const start = now - 2.32 * H;
    const P = (h, r) => [Math.round(start + h * H), 100 - r];
    return {
      period, group, days, series: names.map((name, k) => ({ name, values: vals[k] })), heatmap, sessions, sessionsTotal: Math.round(312 * n / 30 * scale),
      billed: 212.6 * n / 30 * scale, billedNote: 'Bedrock + Anthropic API', cacheShare: 68, ranOut: 3, ranOutNote: 'last Tue 15:40, 5-hour',
      resets: [
        { account: 'Claude — work', provider: 'anthropic', window: '5-hour', resetsAt: at(2 * H + 41 * M), pct: 78, status: 'crit' },
        { account: 'ChatGPT', provider: 'openai', window: 'Codex 5-hour', resetsAt: at(3 * H + 52 * M), pct: 12, status: 'ok' },
        { account: 'Claude — work', provider: 'anthropic', window: 'Weekly Opus', resetsAt: at(2 * D + 21 * H), pct: 83, status: 'warn' },
        { account: 'Bedrock', provider: 'bedrock', window: 'iokig-prod budget', resetsAt: at(8 * D + 12 * H), pct: 37, status: 'est' },
      ],
      burndown: {
        title: 'Claude — work · 5-hour', start, reset: start + 5 * H,
        points: [P(0, 100), P(0.5, 95), P(1, 87), P(1.5, 72), P(2, 41), P(2.32, 22)],
        outAt: Math.round(start + 3.77 * H), cone: [Math.round(start + 3.33 * H), Math.round(start + 4.33 * H)],
      },
    };
  };

  const notes = [
    { version: '0.2.0', date: '2026-10-30', notes: '### Fixed\n- The **Gemini** reader follows Google\'s new usage endpoint.\n- Countdowns no longer skip a minute after sleep.\n\n### New\n- Compact density for people with many accounts.\n- `Request more` remembers the channel you used last.' },
  ];

  const commands = {
    get_state: () => app,
    get_info: () => ({ version: app.version, update: app.update, updateHow: app.updateHow }),
    get_settings: () => app.settings,
    set_settings: ({ patch }) => {
      const p = { ...patch };
      if (p.rattleboard) {
        const { alerts, card, target } = { ...app.settings.rattleboard, ...p.rattleboard };
        if (card && !app.settings.rattleboard.card && (!app.rattleboardKey || app.settings.rattleboard.kind === 'vestaboard')) {
          throw new Error('The card needs the board’s key: paste its pair link from Rattleboard → Settings → API server.');
        }
        p.rattleboard = { ...app.settings.rattleboard, alerts, card, target };
        if (card !== app.settings.rattleboard.card) app.rattleboardStatus = card ? `Added the AI runway card to the rotation at ${hhmm()}` : `Took the card off the board at ${hhmm()}`;
      }
      app = { ...app, settings: { ...app.settings, ...p } };
      setTimeout(() => emit('tr://state', app), 50);
      return app.settings;
    },
    rattleboard_detect: async () => { await wait(250); const there = rbq !== 'none'; return { found: there, binary: there, server: there }; },
    rattleboard_connect: async ({ mode, link, url, key }) => {
      await wait(500);
      let name = 'This computer', rows = 6, cols = 22, base = 'http://127.0.0.1:7070', hasKey = false;
      if (link) {
        if (!/^rattleboard:\/\/pair\?/.test(link)) throw new Error("That isn't a pair link. Copy it from Rattleboard → Settings → API server.");
        const lq = new URLSearchParams(link.split('?')[1]);
        if (mode === 'network') { name = lq.get('name') || 'Rattleboard'; base = lq.get('url') || base; }
        rows = +(lq.get('rows') || 6); cols = +(lq.get('cols') || 22); hasKey = true;
      } else if (mode === 'network') {
        if (!url) throw new Error('Enter the board’s address, like 192.168.1.20:7070');
        if (!key) throw new Error('Paste the board’s pair link, or its key.');
        name = 'Living room TV'; base = /^https?:/.test(url) ? url : `http://${url.includes(':') ? url : url + ':7070'}`; hasKey = true;
      }
      const prev = app.settings.rattleboard;
      app = { ...app, rattleboardKey: hasKey, rattleboardStatus: null, settings: { ...app.settings, rattleboard: { ...prev, mode, url: base, name, rows, cols, kind: 'rattleboard', card: prev.card && hasKey } } };
      setTimeout(() => emit('tr://state', app), 50);
      return { name, rows, cols, message: null };
    },
    rattleboard_test: async () => {
      await wait(700);
      const r = app.settings.rattleboard;
      const message = app.rattleboardKey ? `Sent to “${r.name}” (${r.rows}×${r.cols})` : 'Sent with rattleboard --notify (exit 0)';
      app = { ...app, rattleboardStatus: `${message} at ${hhmm()}` };
      setTimeout(() => emit('tr://state', app), 50);
      return { name: r.name, rows: r.rows, cols: r.cols, message };
    },
    rattleboard_disconnect: async () => {
      await wait(300);
      app = { ...app, rattleboardKey: false, rattleboardStatus: null, settings: { ...app.settings, rattleboard: { ...rbDefault } } };
      setTimeout(() => emit('tr://state', app), 50);
    },
    get_stats: ({ period }) => stats(period),
    get_harness: ({ period }) => harness(period),
    get_dashboard: ({ period, group, account }) => dashboard(period, group, account),
    list_detected: () => detected,
    list_aws_profiles: () => ['iokig-prod', 'default'],
    add_account: async ({ provider, method, label }) => {
      await wait(500);
      const base = provider === 'openai' ? chatgpt() : provider === 'bedrock' ? bedrock() : claude();
      const acct = { ...base, id: 'acct-' + Math.round(Math.random() * 1e6), name: label || base.name, method, updatedAt: Date.now() };
      acct.windows = acct.windows.map((w) => ({ ...w, status: w.status === 'est' ? 'est' : w.status, proj: null, pace: w.pace }));
      app = { ...app, accounts: [...app.accounts.filter((a) => a.name !== acct.name), acct], detected: app.detected.filter((d) => d.provider !== provider) };
      if (!app.summary) app.summary = { label: '1h 26m', status: 'crit', window: 'Claude — work · 5-hour', crit: 1, warn: 1, ok: 1 };
      setTimeout(() => emit('tr://state', app), 50);
      return acct;
    },
    rename_account: ({ accountId, name }) => { app = { ...app, accounts: app.accounts.map((a) => (a.id === accountId ? { ...a, name } : a)) }; emit('tr://state', app); },
    remove_account: ({ accountId }) => { app = { ...app, accounts: app.accounts.filter((a) => a.id !== accountId) }; emit('tr://state', app); },
    refresh: async ({ accountId }) => {
      await wait(700);
      app = { ...app, accounts: app.accounts.map((a) => (!accountId || a.id === accountId ? { ...a, updatedAt: Date.now() } : a)) };
      emit('tr://state', app);
    },
    login_account: () => null,
    open_provider: () => null,
    open_dashboard: () => { window.open('dashboard.html' + location.search, '_blank'); },
    open_site: () => { window.open('https://www.iokig.com/tokenrunway', '_blank'); },
    request_draft: ({ accountId, windowId }) => {
      const a = app.accounts.find((x) => x.id === accountId) || claude();
      const w = a.windows.find((x) => x.id === windowId) || a.windows[0];
      return {
        accountId: a.id, title: `${a.name} · ${w.label} at ${Math.round(w.pct)}%`, forecast: w.forecast,
        channels: ['provider', 'email', 'slack', 'teams'], to: app.settings.admins[a.id] || { name: '', email: '', slack: '', teams: '' },
        body: "Hi Jordan, I'm close to my Claude limit on the Enterprise seat.\n\n• 5-hour: 78% used (412k / 530k tokens), resets 14:20\n• At this pace I run out at 13:05, 1h 15m before reset\n• Weekly 64% · Weekly Opus 83%, both reset Mon 09:00\n\nCould you raise my limit or add usage for today? I'm in the middle of [what you're working on].\n\nThanks, Sam",
      };
    },
    copy_text: async ({ text }) => { try { await navigator.clipboard.writeText(text); } catch { /* no clipboard in the preview */ } },
    send_request: async () => { await wait(400); },
    check_for_updates: async () => { await wait(300); return { current: app.version, latest: app.update || app.version, newer: !!app.update }; },
    update_notes: () => notes,
    install_update: async () => {
      for (let got = 0; got <= 10; got++) { emit('tr://update-progress', { got: got * 1e6, total: 10e6 }); await wait(150); }
      throw new Error('This is the mock: nothing to install.');
    },
    open_update: () => null,
  };

  window.TRMock = {
    async invoke(cmd, args = {}) {
      // ?fixture=<url of a folder>: real data dumped by the ingest_local example (and a
      // get_state.json copied from the app's debug state file) wins where a file exists.
      const fx = new URLSearchParams(location.search).get('fixture');
      if (fx) {
        const name = [cmd, args.period, args.group].filter(Boolean).join('-');
        const r = await fetch(`${fx}/${name}.json`).catch(() => null);
        if (r && r.ok) {
          const v = await r.json();
          if (cmd === 'get_state') app = v;
          return v;
        }
      }
      const f = commands[cmd];
      if (!f) throw new Error(`mock: no command ${cmd}`);
      await wait(30);
      return structuredClone(await f(args));
    },
    async listen(event, cb) { (listeners[event] ||= []).push(cb); return () => { listeners[event] = listeners[event].filter((x) => x !== cb); }; },
    emit,
  };
})();
