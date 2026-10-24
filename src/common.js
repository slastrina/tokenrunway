// Shared by the popover and the dashboard: theme classes, the Tauri bridge (or mock.js when there
// is no Tauri, for checking the UI in a browser), formatting, icons, the tooltip, popup menus and
// the one-second ticker that keeps countdowns live. Loaded in <head> so the theme is set before paint.
(() => {
  const q = new URLSearchParams(location.search);
  const root = document.documentElement;
  const mq = matchMedia('(prefers-color-scheme: dark)');
  const applyTheme = () => {
    const t = q.get('theme') || (mq.matches ? 'dark' : 'light');
    root.classList.toggle('tr-dark', t === 'dark');
    root.classList.toggle('tr-light', t !== 'dark');
  };
  root.classList.add('tr');
  applyTheme();
  mq.addEventListener?.('change', applyTheme);

  const mock = !window.__TAURI__;
  if (mock) root.classList.add('tr-mock');

  const api = mock
    ? new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = 'mock.js';
      s.onload = () => resolve(window.TRMock);
      s.onerror = reject;
      document.head.appendChild(s);
    })
    : Promise.resolve({ invoke: window.__TAURI__.core.invoke, listen: window.__TAURI__.event.listen });

  const setOs = (os) => root.classList.toggle('tr-win', os === 'windows');

  // ---------- formatting ----------
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  const pad = (n) => String(n).padStart(2, '0');
  const hc = new Intl.DateTimeFormat(undefined, { hour: 'numeric' }).resolvedOptions().hourCycle;
  const h12 = hc === 'h12' || hc === 'h11';
  root.classList.toggle('tr-h12', h12);
  const fmtWeekday = new Intl.DateTimeFormat(undefined, { weekday: 'short' });
  const fmtDayMonth = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });
  const clock = (ms) => {
    const d = new Date(ms);
    return h12 ? d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }) : `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  };
  const dayStart = (ms) => { const d = new Date(ms); d.setHours(0, 0, 0, 0); return d.getTime(); };
  const dayDiff = (ms) => Math.round((dayStart(ms) - dayStart(Date.now())) / 86400000);
  const weekday = (ms) => fmtWeekday.format(new Date(ms));
  const dayMonth = (ms) => fmtDayMonth.format(new Date(ms));
  /** "14:20" today, "Mon 09:00" within the week, else "1 Nov" */
  const when = (ms) => {
    const dd = dayDiff(ms);
    if (dd === 0) return clock(ms);
    if (Math.abs(dd) < 7) return `${weekday(ms)} ${clock(ms)}`;
    return dayMonth(ms);
  };
  /** "41m", "2h 41m", "2d 21h" */
  const dur = (ms) => {
    if (ms == null) return '';
    if (ms < 60000) return ms <= 0 ? 'now' : '<1m';
    const m = Math.floor(ms / 60000);
    if (m < 60) return `${m}m`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h}h ${m % 60}m`;
    return `${Math.floor(h / 24)}d ${h % 24}h`;
  };
  const ago = (ms) => {
    const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
    if (s < 3) return 'just now';
    if (s < 60) return `${s} s ago`;
    const m = Math.floor(s / 60);
    if (m < 60) return `${m} min ago`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h} h ago`;
    return `${Math.floor(h / 24)} d ago`;
  };
  const resetText = (ms) => {
    const left = ms - Date.now();
    return left <= 0 ? 'resetting now' : `resets in ${dur(left)} · ${when(ms)}`;
  };
  const money = (n, dp = 2) => (n < 0 ? '-' : '') + '$' + Math.abs(n ?? 0).toLocaleString('en-US', { minimumFractionDigits: dp, maximumFractionDigits: dp });
  const tokens = (n) => {
    n = n ?? 0;
    if (n >= 1e9) return (n / 1e9).toFixed(1) + 'B';
    if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
    if (n >= 1e3) return Math.round(n / 1e3) + 'k';
    return String(n);
  };
  const minutes = (ms) => {
    const m = Math.round((ms ?? 0) / 60000);
    return m < 60 ? `${m}m` : `${Math.floor(m / 60)}h ${m % 60}m`;
  };
  /** ISO date "2026-10-23" -> local midnight ms */
  const isoDay = (s) => { const [y, m, d] = s.split('-').map(Number); return new Date(y, m - 1, d).getTime(); };

  // ---------- colours ----------
  const PROVIDER = {
    anthropic: '--p-claude', openai: '--p-openai', bedrock: '--p-bedrock', 'anthropic-api': '--p-api', 'openai-api': '--p-api',
    xai: '--p-grok', grok: '--p-grok', gemini: '--p-gemini', copilot: '--p-copilot', cursor: '--p-cursor', openrouter: '--p-openrouter',
  };
  const providerVar = (p) => PROVIDER[p] || '--p-api';
  const HARNESS = { 'claude-code': '--p-claude', codex: '--p-openai', opencode: '--h-opencode', 'gemini-cli': '--p-gemini', cline: '--p-bedrock' };
  const harnessVar = (idOrName) => {
    const k = String(idOrName || '').toLowerCase().replace(/\s+/g, '-');
    return HARNESS[k] || (k.includes('claude') ? '--p-claude' : k.includes('codex') ? '--p-openai' : k.includes('gemini') ? '--p-gemini' : k.includes('opencode') ? '--h-opencode' : k.includes('cline') ? '--p-bedrock' : '--m-other');
  };
  const STATUS = {
    ok: { fill: '--ok', text: '--ok-text', bg: '--ok-bg', word: 'On track' },
    warn: { fill: '--warn', text: '--warn-text', bg: '--warn-bg', word: 'Warning' },
    crit: { fill: '--crit', text: '--crit-text', bg: '--crit-bg', word: 'Critical' },
    est: { fill: '--fg3', text: '--fg2', bg: '--btn', word: 'Estimate' },
  };
  const status = (s) => STATUS[s] || STATUS.est;

  // ---------- icons (16-unit grid, from the design) ----------
  const svg = (w, body, sw = 1.5, extra = '') => `<svg width="${w}" height="${w}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"${extra}>${body}</svg>`;
  const ICON = {
    ok: (w = 12, sw = 1.8) => svg(w, '<circle cx="8" cy="8" r="6"/><path d="m5.4 8.2 1.8 1.8 3.4-3.7"/>', sw),
    warn: (w = 12, sw = 1.8) => svg(w, '<path d="M8 2.2 14.2 13.2H1.8Z"/><path d="M8 6.6v3"/><path d="M8 11.4v.01"/>', sw),
    crit: (w = 14, sw = 1.7) => svg(w, '<path d="M5.5 1.8h5l3.7 3.7v5l-3.7 3.7h-5l-3.7-3.7v-5Z"/><path d="M8 5v3.6"/><path d="M8 11v.01"/>', sw),
    est: (w = 12, sw = 1.8) => svg(w, '<circle cx="8" cy="8" r="6" stroke-dasharray="2.2 2"/><path d="M5.2 8.6c.9-1.2 1.8-1.2 2.8 0s1.9 1.2 2.8 0"/>', sw),
    plus: (w = 15, sw = 1.5) => svg(w, '<path d="M8 3v10M3 8h10"/>', sw),
    refresh: (w = 15, sw = 1.5) => svg(w, '<path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9"/><path d="M13.5 2.5v3h-3"/>', sw),
    external: (w = 14, sw = 1.5) => svg(w, '<path d="M9 2.5h4.5V7"/><path d="M13.5 2.5 7.5 8.5"/><path d="M12 9.5v3a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3"/>', sw),
    gear: (w = 15, sw = 1.4) => svg(w, '<circle cx="8" cy="8" r="2.2"/><path d="M6.9 1.8h2.2l.4 1.7 1.2.6 1.5-.9 1.6 1.6-.9 1.5.6 1.2 1.7.4v2.2l-1.7.4-.6 1.2.9 1.5-1.6 1.6-1.5-.9-1.2.6-.4 1.7H6.9l-.4-1.7-1.2-.6-1.5.9-1.6-1.6.9-1.5-.6-1.2-1.7-.4V6.9l1.7-.4.6-1.2-.9-1.5 1.6-1.6 1.5.9 1.2-.6Z"/>', sw),
    back: (w = 14, sw = 1.7) => svg(w, '<path d="M10 3.5 5.5 8l4.5 4.5"/>', sw),
    chevron: (w = 13, sw = 1.7) => svg(w, '<path d="m6 3.5 4.5 4.5L6 12.5"/>', sw),
    down: (w = 11, sw = 1.8) => svg(w, '<path d="m4 6 4 4 4-4"/>', sw),
    clock: (w = 11, sw = 1.8) => svg(w, '<circle cx="8" cy="8" r="6"/><path d="M8 4.8V8l2.2 1.4"/>', sw),
    lock: (w = 11, sw = 1.8) => svg(w, '<rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/>', sw),
    tool: (w = 11, sw = 1.8) => svg(w, '<path d="M10.5 2.5a3 3 0 0 0-3.6 3.9L2.5 10.8a1.4 1.4 0 0 0 2 2l4.4-4.4a3 3 0 0 0 3.9-3.6l-1.8 1.8-1.7-.4-.4-1.7Z"/>', sw),
    send: (w = 13, sw = 1.7) => svg(w, '<path d="M14 2 7 9"/><path d="M14 2 9.5 14 7 9 2 6.5Z"/>', sw),
    check: (w = 24, sw = 1.8) => svg(w, '<path d="m4 8.3 2.6 2.6L12 5.4"/>', sw),
    dots: (w = 16) => `<svg width="${w}" height="${w}" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="3.5" cy="8" r="1.1"/><circle cx="8" cy="8" r="1.1"/><circle cx="12.5" cy="8" r="1.1"/></svg>`,
    spinner: (w = 13) => `<svg class="spin" width="${w}" height="${w}" viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="6" stroke="var(--track)" stroke-width="2"/><path d="M8 2a6 6 0 0 1 6 6" stroke="var(--fg2)" stroke-width="2" stroke-linecap="round"/></svg>`,
    trash: (w = 14, sw = 1.5) => svg(w, '<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5"/>', sw),
  };
  ICON.status = (s, w, sw) => (ICON[s] || ICON.est)(w, sw);

  // ---------- tooltip (one floating element; text only, values first) ----------
  let tipEl;
  const tip = {
    show(x, y, rows) {
      if (!tipEl) { tipEl = document.createElement('div'); tipEl.className = 'tr-tip'; tipEl.setAttribute('role', 'tooltip'); document.body.appendChild(tipEl); }
      tipEl.textContent = '';
      for (const r of rows) {
        const row = document.createElement('div');
        row.className = r.head ? 'tip-head' : 'tip-row';
        if (r.color) { const k = document.createElement('span'); k.className = 'tip-key'; k.style.background = `var(${r.color})`; row.appendChild(k); }
        if (r.value != null) { const v = document.createElement('b'); v.textContent = r.value; row.appendChild(v); }
        if (r.label != null) { const l = document.createElement('span'); l.textContent = r.label; row.appendChild(l); }
        tipEl.appendChild(row);
      }
      tipEl.style.display = 'block';
      const bw = tipEl.offsetWidth, bh = tipEl.offsetHeight;
      let left = x + 12, top = y + 12;
      if (left + bw > innerWidth - 6) left = x - bw - 12;
      if (top + bh > innerHeight - 6) top = y - bh - 12;
      tipEl.style.left = Math.max(6, left) + 'px';
      tipEl.style.top = Math.max(6, top) + 'px';
    },
    hide() { if (tipEl) tipEl.style.display = 'none'; },
  };

  // ---------- popup menu ----------
  let menuEl = null;
  const closeMenu = () => { if (menuEl) { const a = menuEl._anchor; menuEl.remove(); menuEl = null; a?.setAttribute('aria-expanded', 'false'); } };
  /** items: [{ label, onSelect, checked?, disabled? }] */
  const menu = (anchor, items, opts = {}) => {
    if (menuEl && menuEl._anchor === anchor) { closeMenu(); return; }
    closeMenu();
    const m = document.createElement('div');
    m.className = 'tr-menu';
    m.setAttribute('role', 'menu');
    m._anchor = anchor;
    items.forEach((it) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.setAttribute('role', it.checked != null ? 'menuitemradio' : 'menuitem');
      if (it.checked != null) b.setAttribute('aria-checked', String(!!it.checked));
      b.disabled = !!it.disabled;
      b.textContent = it.label;
      b.addEventListener('click', () => { closeMenu(); it.onSelect?.(); anchor.focus(); });
      m.appendChild(b);
    });
    document.body.appendChild(m);
    const r = anchor.getBoundingClientRect();
    const w = m.offsetWidth;
    let left = opts.align === 'left' ? r.left : r.right - w;
    left = Math.max(6, Math.min(left, innerWidth - w - 6));
    let top = r.bottom + 4;
    if (top + m.offsetHeight > innerHeight - 6) top = r.top - m.offsetHeight - 4;
    m.style.left = left + 'px';
    m.style.top = top + 'px';
    anchor.setAttribute('aria-expanded', 'true');
    menuEl = m;
    const btns = [...m.querySelectorAll('button:not(:disabled)')];
    (btns.find((b) => b.getAttribute('aria-checked') === 'true') || btns[0])?.focus();
    m.addEventListener('keydown', (e) => {
      const i = btns.indexOf(document.activeElement);
      if (e.key === 'ArrowDown') { e.preventDefault(); btns[(i + 1) % btns.length]?.focus(); }
      if (e.key === 'ArrowUp') { e.preventDefault(); btns[(i - 1 + btns.length) % btns.length]?.focus(); }
      if (e.key === 'Escape' || e.key === 'Tab') { e.preventDefault(); closeMenu(); anchor.focus(); }
    });
  };
  document.addEventListener('pointerdown', (e) => { if (menuEl && !menuEl.contains(e.target) && !menuEl._anchor.contains(e.target)) closeMenu(); });
  addEventListener('blur', closeMenu);

  // ---------- arrow keys inside tablists and radiogroups (roving focus) ----------
  document.addEventListener('keydown', (e) => {
    const group = e.target.closest?.('[role=tablist],[role=radiogroup]');
    if (!group || !['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
    const items = [...group.querySelectorAll('[role=tab],[role=radio]')].filter((b) => !b.disabled);
    const i = items.indexOf(e.target);
    if (i < 0) return;
    e.preventDefault();
    const n = e.key === 'Home' ? 0 : e.key === 'End' ? items.length - 1
      : (i + (e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 1) + items.length) % items.length;
    items[n].focus();
    items[n].click();
  });

  // ---------- ticker: [data-reset], [data-ago], [data-in], [data-left] stay live ----------
  const tick = () => {
    document.querySelectorAll('[data-reset]').forEach((el) => { el.textContent = resetText(+el.dataset.reset); });
    document.querySelectorAll('[data-ago]').forEach((el) => { el.textContent = (el.dataset.pre || '') + ago(+el.dataset.ago); });
    document.querySelectorAll('[data-in]').forEach((el) => { el.textContent = 'in ' + dur(+el.dataset.in - Date.now()); });
    document.querySelectorAll('[data-left]').forEach((el) => { el.textContent = dur(+el.dataset.left - Date.now()); });
  };
  setInterval(tick, 1000);

  window.TRC = {
    q, mock, api, setOs, esc, clock, when, dur, ago, resetText, money, tokens, minutes, weekday, dayMonth, dayDiff, isoDay,
    providerVar, harnessVar, status, ICON, tip, menu, closeMenu, tick,
  };
})();
