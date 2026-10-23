// The popover: Now / Stats / Harness tabs, add account, request more, settings and updates, with
// the footer and its iokig.com credit on every view (SPEC.md §6.2, §6.5). Draws the data contract
// in crates/tokenrunway-core/src/view.rs as docs/design/*.dc.html shows it.
(async () => {
  const C = window.TRC;
  const { esc, ICON } = C;
  const { invoke, listen } = await C.api;
  const $ = (s) => document.querySelector(s);

  const TABS = [['now', 'Now'], ['stats', 'Stats'], ['harness', 'Harness']];
  const PERIODS = [['today', 'Today'], ['7d', '7 d'], ['30d', '30 d'], ['lifetime', 'Lifetime']];
  const PROBLEM = new Set(['stale', 'login', 'broken']);
  const SHORT = { anthropic: 'Claude', openai: 'ChatGPT', bedrock: 'AWS Bedrock', 'anthropic-api': 'Anthropic API', 'openai-api': 'OpenAI API', gemini: 'Gemini', xai: 'Grok', copilot: 'Copilot', cursor: 'Cursor' };

  const S = {
    app: null,
    view: 'now',
    tab: 'now', // last tab, where sub-views return to
    period: { stats: '7d', harness: '7d' },
    stats: null, harness: null, statsErr: '', harnessErr: '', loadingData: false,
    busy: new Set(),
    add: null,
    req: null,
    settings: null, openAdmin: new Set(), confirmRemove: null, renames: {},
    upd: { phase: 'idle' },
    dirty: false,
  };

  const machine = () => (S.app?.os === 'windows' ? 'PC' : S.app?.os === 'linux' ? 'computer' : 'Mac');
  const isWin = () => S.app?.os === 'windows';
  const announce = (t) => { const l = $('#live'); l.textContent = ''; setTimeout(() => { l.textContent = t; }, 30); };
  const inlineMd = (s) => esc(s).replace(/`([^`]+)`/g, '<code>$1</code>');
  const tile = (provider, mono, cls = '') => `<span class="tile ${cls}" style="--c:var(${C.providerVar(provider)})" aria-hidden="true">${esc(mono || '?')}</span>`;
  const plural = (n, one, many = one + 's') => `${n} ${n === 1 ? one : many}`;

  // ---------- render with focus + scroll preserved ----------
  function render() {
    const view = $('#view');
    const act = document.activeElement;
    const key = act?.dataset?.k;
    const sel = act && 'selectionStart' in act ? [act.selectionStart, act.selectionEnd] : null;
    const scroll = view.querySelector('.scroll')?.scrollTop || 0;
    const sameView = view.dataset.v === S.view;
    view.dataset.v = S.view;
    view.innerHTML = ({ now: viewNow, stats: viewStats, harness: viewHarness, add: viewAdd, request: viewRequest, settings: viewSettings, update: viewUpdate })[S.view]?.() || viewNow();
    if (sameView) { const sc = view.querySelector('.scroll'); if (sc) sc.scrollTop = scroll; }
    if (key) {
      const el = document.querySelector(`[data-k="${CSS.escape(key)}"]`);
      if (el) { el.focus({ preventScroll: true }); if (sel && 'setSelectionRange' in el) { try { el.setSelectionRange(sel[0], sel[1]); } catch { /* not a text field */ } } }
    }
    renderFoot();
    C.tick();
  }
  const editing = () => { const a = document.activeElement; return a && (a.tagName === 'INPUT' || a.tagName === 'TEXTAREA') && a.type !== 'checkbox'; };
  // Don't re-render under someone's typing; catch up when they leave the field.
  function softRender() { if (editing()) { S.dirty = true; renderFoot(); } else render(); }
  document.addEventListener('focusout', () => setTimeout(() => { if (S.dirty && !editing()) { S.dirty = false; render(); } }, 0));

  function renderFoot() {
    const a = S.app || {};
    const f = $('#foot');
    f.classList.toggle('has-update', !!a.update);
    const left = a.update
      ? `<button class="upd-link" data-act="view" data-v="update" data-k="foot-upd">Update available: v${esc(a.update)} →</button>`
      : `<span>Tokenrunway v${esc(a.version || '')}</span>`;
    f.innerHTML = `${left}<span aria-hidden="true">·</span><button class="link site" data-act="site" data-k="foot-site">iokig.com</button><span class="flex"></span>`
      + `<button class="ib" data-act="settings" data-k="foot-gear" aria-label="Settings" aria-pressed="${S.view === 'settings'}">${ICON.gear(isWin() ? 16 : 15, isWin() ? 1.3 : 1.4)}</button>`;
  }

  // ---------- shared pieces ----------
  function tabs() {
    return `<div class="tabs" role="tablist" aria-label="Views">${TABS.map(([id, label]) => {
      const on = S.view === id;
      return `<button role="tab" id="tab-${id}" aria-selected="${on}" aria-controls="panel" tabindex="${on ? 0 : -1}" data-act="tab" data-v="${id}" data-k="tab-${id}">${label}</button>`;
    }).join('')}</div>`;
  }
  const bar = (right = '') => `<div class="bar">${tabs()}<span class="flex"></span>${right}</div>`;
  const subbar = (backLabel, title, backAct = 'back') => `<div class="subbar"><button class="back" data-act="${backAct}" data-k="back" aria-label="Back to ${esc(backLabel)}">${ICON.back()}${esc(backLabel)}</button><h2>${title}</h2></div>`;
  const periodGroup = (which) => `<div class="period" role="radiogroup" aria-label="Period">${PERIODS.map(([id, label]) => {
    const on = S.period[which] === id;
    return `<button role="radio" aria-checked="${on}" tabindex="${on ? 0 : -1}" data-act="period" data-w="${which}" data-v="${id}" data-k="p-${which}-${id}">${label}</button>`;
  }).join('')}</div>`;
  const seg = (label, name, options, value, act = 'set') => `<div class="seg" role="radiogroup" aria-label="${esc(label)}">${options.map(([v, l]) => {
    const on = value === v;
    return `<button role="radio" aria-checked="${on}" tabindex="${on ? 0 : -1}" data-act="${act}" data-name="${name}" data-v="${esc(v)}" data-k="seg-${name}-${esc(v)}">${esc(l)}</button>`;
  }).join('')}</div>`;
  const refreshBtn = (id, label, cls = '') => {
    const busy = S.busy.has(id || 'all');
    return `<button class="ib ${cls} ${busy ? 'busy' : ''}" data-act="refresh" data-id="${esc(id || '')}" data-k="refresh-${esc(id || 'all')}" aria-label="${esc(label)}"${busy ? ' aria-busy="true"' : ''}>${ICON.refresh(cls ? 14 : isWin() ? 16 : 15, isWin() ? 1.3 : 1.5)}</button>`;
  };

  // ---------- Now ----------
  function viewNow() {
    const a = S.app;
    if (!a) return bar() + '<div class="scroll" id="panel"></div>';
    if (a.loading) return viewLoading();
    if (!a.accounts.length) return viewEmpty();
    const problems = a.accounts.filter((x) => PROBLEM.has(x.state)).length;
    const right = problems
      ? `<span class="chip lg st-warn" role="status">${ICON.warn()}${problems} need${problems === 1 ? 's' : ''} attention</span>`
      : (isWin() ? '' : `<button class="ib" data-act="add" data-k="add" aria-label="Add account">${ICON.plus()}</button>`) + refreshBtn('', 'Refresh all');
    return bar(right) + summary(a.summary)
      + `<div class="scroll" id="panel" role="tabpanel" aria-labelledby="tab-now"><div class="list">${a.accounts.map(card).join('')}</div></div>`;
  }

  function summary(s) {
    if (!s) return '';
    const color = s.status === 'crit' ? 'var(--crit-text)' : s.status === 'warn' ? 'var(--warn-text)' : 'var(--fg1)';
    const total = s.crit + s.warn + s.ok;
    let right;
    if (!s.crit && !s.warn) right = `<span class="chip lg st-ok">${ICON.ok()}All ${plural(total, 'window')} on track</span>`;
    else {
      const parts = [];
      if (s.crit) parts.push(`${s.crit} critical`);
      if (s.warn) parts.push(`${s.warn} tight`);
      if (s.ok) parts.push(`${s.ok} on track`);
      right = `<div class="r"><span class="w">${esc(s.window)}</span><span class="c">${parts.join(' · ')}</span></div>`;
    }
    return `<div class="summary"><div class="l"><span class="caps">Shortest runway</span><span class="big" style="color:${color}">${esc(s.label)}</span></div>${right}</div>`;
  }

  function idBlock(acc, subHtml) {
    return `<div class="acct-id"><div class="acct-n"><span class="name">${esc(acc.name)}</span>${acc.plan ? `<span class="plan">${esc(acc.plan)}</span>` : ''}</div>${subHtml}</div>`;
  }
  const updated = (acc) => (acc.updatedAt ? `<span class="sub" data-pre="Updated " data-ago="${acc.updatedAt}">Updated ${C.ago(acc.updatedAt)}</span>` : '<span class="sub">Waiting for the first reading</span>');

  function cardActions(acc) {
    if (isWin()) return `<button class="ib sm" data-act="more" data-id="${esc(acc.id)}" data-k="more-${esc(acc.id)}" aria-label="More actions for ${esc(acc.name)}" aria-haspopup="menu" aria-expanded="false">${ICON.dots()}</button>`;
    return `<button class="ib sm" data-act="open-provider" data-id="${esc(acc.id)}" data-k="open-${esc(acc.id)}" aria-label="Open provider page for ${esc(acc.name)}">${ICON.external()}</button>`
      + refreshBtn(acc.id, `Refresh ${acc.name}`, 'sm');
  }

  function card(acc) {
    if (acc.state === 'loading') return skelCard(acc, 1);
    if (acc.state === 'stale') return staleCard(acc);
    if (acc.state === 'login') return loginCard(acc);
    if (acc.state === 'broken') return brokenCard(acc);
    if (S.app.settings?.density === 'compact') return compactCard(acc);
    const wins = acc.windows.length ? acc.windows.map((w) => windowRow(acc, w)).join('') : '<p class="note">No usage windows reported for this account yet.</p>';
    return `<article class="card" aria-label="${esc(acc.name)}"><div class="acct-h">${tile(acc.provider, acc.mono)}${idBlock(acc, updated(acc))}${cardActions(acc)}</div>${wins}</article>`;
  }

  function wantsRequest(w) {
    return w.status === 'crit' || (w.pct >= (S.app.settings?.requestAt ?? 85) && w.status !== 'est');
  }
  function requestBanner(acc, w) {
    const crit = w.status === 'crit';
    return `<div class="banner ${crit ? '' : 'warn'}">${crit ? ICON.crit() : ICON.warn(14, 1.7)}<span class="msg">${esc(w.forecast || `${Math.round(w.pct)}% used`)}</span>`
      + `<button class="btn accent" data-act="request" data-id="${esc(acc.id)}" data-w="${esc(w.id)}" data-k="req-${esc(acc.id)}-${esc(w.id)}">Request more</button></div>`;
  }

  function ubar(w, opts = {}) {
    const pct = Math.max(0, Math.min(100, w.pct));
    const projW = w.proj != null ? Math.max(0, Math.min(100, w.proj) - pct) : 0;
    const label = `${w.label}: ${Math.round(w.pct)}% used${w.pace != null ? `, ${Math.round(w.pace)}% of the window elapsed` : ''}${w.proj != null ? `, projected ${Math.round(w.proj)}% at reset` : ''}`;
    return `<div class="ubar" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(w.pct)}" aria-label="${esc(label)}">`
      + `<div class="fill ${opts.dashed ? 'dashed' : ''}" style="width:${pct}%"></div>`
      + (projW && !opts.noProj ? `<div class="proj" style="left:${pct}%;width:${projW}%"></div>` : '')
      + (w.pace != null && !opts.noPace ? `<div class="pace" style="left:${Math.max(0, Math.min(100, w.pace))}%"></div>` : '')
      + '</div>';
  }

  function windowRow(acc, w) {
    const banner = wantsRequest(w);
    const chip = w.status !== 'crit' && w.forecast ? `<span class="chip">${ICON.status(w.status)}${esc(w.forecast)}</span>` : '';
    const reset = w.resetsAt ? `<span class="reset" data-reset="${w.resetsAt}">${C.resetText(w.resetsAt)}</span>` : '<span class="reset">No fixed reset</span>';
    return `<div class="win st-${esc(w.status)}">`
      + `<div class="win-h"><span class="wl">${esc(w.label)}</span>${w.abs ? `<span class="wa">${esc(w.abs)}</span>` : ''}<span class="wp">${Math.round(w.pct)}%</span></div>`
      + ubar(w)
      + `<div class="win-f">${reset}${chip}</div>`
      + (banner ? requestBanner(acc, w) : '')
      + '</div>';
  }

  function compactCard(acc) {
    const rows = acc.windows.map((w) => `<div class="crow st-${esc(w.status)}"><span class="cl">${esc(w.label)}</span>${ubar(w, { noProj: true })}`
      + `<span class="cp ${w.status === 'ok' ? '' : 'hot'}">${Math.round(w.pct)}%</span><span class="cr">${w.resetsAt ? esc(C.when(w.resetsAt)) : '—'}</span></div>`
      + (w.status === 'crit' ? requestBanner(acc, w) : '')).join('');
    return `<article class="card compact" aria-label="${esc(acc.name)}"><div class="acct-h">${tile(acc.provider, acc.mono, 's20')}<span class="name">${esc(acc.name)}</span>`
      + `${acc.updatedAt ? `<span class="sub" data-ago="${acc.updatedAt}">${C.ago(acc.updatedAt)}</span>` : ''}</div>${rows}</article>`;
  }

  function staleCard(acc) {
    const flag = `<span class="flag" style="color:var(--warn-text)">${ICON.clock()}<span data-pre="Stale · last reading " data-ago="${acc.updatedAt || Date.now()}">Stale · last reading ${C.ago(acc.updatedAt || Date.now())}</span></span>`;
    const wins = acc.windows.map((w) => `<div class="win"><div class="win-h"><span class="wl">${esc(w.label)}</span>${acc.updatedAt ? `<span class="wa">at ${esc(C.clock(acc.updatedAt))}</span>` : ''}<span class="wp" style="color:var(--fg1)">~${Math.round(w.pct)}%</span></div>`
      + ubar(w, { dashed: true, noProj: true, noPace: true }) + '</div>').join('');
    return `<article class="card problem stale" aria-label="${esc(acc.name)}, stale"><div class="acct-h">${tile(acc.provider, acc.mono)}${idBlock(acc, flag)}`
      + `<button class="btn s24" data-act="refresh" data-id="${esc(acc.id)}" data-k="retry-${esc(acc.id)}"${S.busy.has(acc.id) ? ' disabled' : ''}>${S.busy.has(acc.id) ? 'Retrying…' : 'Retry'}</button></div>`
      + wins + `<p class="note">${inlineMd(acc.problem || 'The last reading is old.')} Forecasts are paused until a fresh reading.</p></article>`;
  }

  function loginCard(acc) {
    const flag = `<span class="flag" style="color:var(--crit-text)">${ICON.lock()}Needs login again</span>`;
    const cmd = (acc.problem || '').match(/`([^`]+)`/)?.[1];
    const second = acc.method === 'cli'
      ? `<button class="btn" data-act="refresh" data-id="${esc(acc.id)}" data-k="ran-${esc(acc.id)}">${cmd ? `I've run ${esc(cmd)}` : "I've logged in again"}</button>` : '';
    return `<article class="card problem" aria-label="${esc(acc.name)}, needs login"><div class="acct-h">${tile(acc.provider, acc.mono)}${idBlock(acc, flag)}</div>`
      + `<p class="note">${inlineMd(acc.problem || 'The login expired or was revoked.')}</p>`
      + `<div class="row-btns"><button class="btn accent" data-act="login" data-id="${esc(acc.id)}" data-k="login-${esc(acc.id)}">Log in again</button>${second}</div></article>`;
  }

  function brokenCard(acc) {
    const flag = `<span class="flag" style="color:var(--fg2)">${ICON.tool()}Source needs an update</span>`;
    const upd = S.app.update
      ? `<button class="btn strong" data-act="view" data-v="update" data-k="bupd-${esc(acc.id)}">Update Tokenrunway</button>`
      : `<button class="btn strong" data-act="view" data-v="update" data-k="bupd-${esc(acc.id)}">Check for updates</button>`;
    return `<article class="card problem" aria-label="${esc(acc.name)}, source needs an update"><div class="acct-h">${tile(acc.provider, acc.mono)}${idBlock(acc, flag)}</div>`
      + `<p class="note">${inlineMd(acc.problem || 'The provider changed something this reader relies on.')}</p>`
      + `<div class="row-btns">${upd}<button class="btn ghost" data-act="details" data-k="details-${esc(acc.id)}">Details</button></div></article>`;
  }

  function skelCard(acc, o) {
    return `<div class="card skel-card" style="opacity:${o}" aria-hidden="true"><div class="acct-h"><span class="tile" style="--c:var(${C.providerVar(acc.provider)})"></span>`
      + `<div style="display:flex;flex-direction:column;gap:5px"><span style="font-size:13px;font-weight:600">${esc(acc.name)}</span><div class="skel" style="width:70px;height:8px"></div></div></div>`
      + '<div style="display:flex;flex-direction:column;gap:7px"><div style="display:flex;justify-content:space-between"><div class="skel" style="width:60px;height:9px"></div><div class="skel" style="width:28px;height:9px"></div></div>'
      + '<div class="skel" style="height:6px;border-radius:3px"></div><div class="skel" style="width:150px;height:8px"></div></div></div>';
  }

  function viewLoading() {
    const accs = S.app.accounts;
    const n = accs.length;
    const note = `<span class="bar-note busy" role="status">${ICON.spinner()}Reading ${n ? plural(n, 'account') : 'accounts'}…</span>`;
    return bar(note)
      + '<div style="padding:4px 16px 14px;display:flex;flex-direction:column;gap:7px;flex-shrink:0" aria-hidden="true"><div class="skel" style="width:96px;height:9px"></div><div class="skel" style="width:120px;height:26px;border-radius:6px"></div></div>'
      + `<div class="scroll" id="panel" role="tabpanel" aria-labelledby="tab-now" aria-busy="true"><div class="list">${accs.map((a, i) => skelCard(a, Math.max(0.4, 1 - i * 0.2))).join('')}</div></div>`;
  }

  function viewEmpty() {
    const det = S.app.detected || [];
    const found = det.length ? `<div class="found"><span class="caps">Found on this ${machine()}</span>${det.map((d) => `<div class="found-row">${tile(d.provider, d.mono, 's20')}`
      + `<span class="n">${esc(d.label)}${d.detail ? ` <span>· ${esc(d.detail)}</span>` : ''}</span>`
      + `<button class="btn s24" data-act="add-detected" data-kind="${esc(d.kind)}" data-k="det-${esc(d.kind)}" aria-label="Add ${esc(d.label)}"${S.busy.has('det-' + d.kind) ? ' disabled' : ''}>${S.busy.has('det-' + d.kind) ? 'Adding…' : 'Add'}</button></div>`).join('')}</div>` : '';
    return bar() + `<div class="scroll" id="panel" role="tabpanel" aria-labelledby="tab-now"><div class="empty">`
      + '<svg width="72" height="72" viewBox="0 0 64 64" aria-hidden="true"><path d="M14 30a18 18 0 0 1 36 0" fill="none" stroke="var(--track)" stroke-width="4" stroke-linecap="round"/><path d="M14 30a18 18 0 0 1 4.2-11.6" fill="none" stroke="#f5a524" stroke-width="4" stroke-linecap="round"/><path d="M29 32 20 54M35 32l9 22" fill="none" stroke="var(--fg2)" stroke-width="2.6" stroke-linecap="round"/><path d="M32 36v3M32 43v4M32 50v4" stroke="#f5a524" stroke-width="2.2" stroke-linecap="round"/></svg>'
      + '<div style="display:flex;flex-direction:column;gap:6px"><h2>No accounts yet</h2><p>Add a subscription or API account to see how much runway you have left. Readings stay on this machine; keys live in your keychain.</p></div>'
      + `<button class="btn s32 accent icon" data-act="add" data-k="add-empty">${ICON.plus(14, 1.8)}Add account</button>${found}${S.addErr ? `<p class="err">${esc(S.addErr)}</p>` : ''}</div></div>`;
  }

  // ---------- Stats ----------
  const tipAttrs = (head, value, label) => ` data-tip-h="${esc(head)}" data-tip-v="${esc(value)}"${label ? ` data-tip-l="${esc(label)}"` : ''}`;

  function viewStats() {
    const right = `<button class="textbtn" data-act="dashboard" data-k="dash" aria-label="Open dashboard">Dashboard${ICON.external(13)}</button>`;
    const st = S.stats;
    let body;
    if (S.statsErr) body = `<div class="msgbox"><h2>Couldn't load stats</h2>${esc(S.statsErr)}</div>`;
    else if (!st) body = '<div class="msgbox" aria-busy="true">Loading…</div>';
    else if (!st.spendApi && !st.tokens?.total) body = '<div class="msgbox"><h2>Nothing in this period</h2>Stats come from your harnesses\' local session logs (Claude Code, Codex, opencode…). Use one and they appear here.</div>';
    else body = statsBody(st);
    return bar(right) + `<div class="scroll" id="panel" role="tabpanel" aria-labelledby="tab-stats"><div class="pane" style="${S.loadingData ? 'opacity:.6' : ''}">${periodGroup('stats')}${body}</div></div>`;
  }

  function statsBody(st) {
    const t = st.tokens || {};
    // A $0 "actual" (free or local models) says nothing useful, so it reads as subscription use.
    const actual = st.spendActual ? `Actual ${C.money(st.spendActual)}${st.actualNote ? ' · ' + esc(st.actualNote) : ''}` : 'What it would cost at API prices';
    const kpis = `<div class="kpis"><div class="kpi"><span class="k">Spend · API-equivalent</span><span class="v">${C.money(st.spendApi)}</span><span class="s">${actual}</span></div>`
      + `<div class="kpi"><span class="k">Tokens</span><span class="v">${C.tokens(t.total)}</span><span class="s">${C.tokens(t.cache)} cache · ${C.tokens(t.input)} in · ${C.tokens(t.output)} out</span></div></div>`;

    const daily = st.daily || [];
    const max = Math.max(0, ...daily.map((d) => d.cost));
    const todayIso = isoToday();
    const peakI = daily.findIndex((d) => d.cost === max);
    const dense = daily.length > 10;
    const dayName = (d, i) => (d.day === todayIso ? 'Today' : dense ? C.dayMonth(C.isoDay(d.day)) : C.weekday(C.isoDay(d.day)));
    const cols = `grid-template-columns:repeat(${Math.max(1, daily.length)}, minmax(0, 1fr))`;
    const bars = daily.map((d, i) => `<div class="d ${d.day === todayIso ? 'today' : ''}" style="height:${max ? Math.max(1, Math.round((d.cost / max) * 56)) : 1}px"${tipAttrs(dayName(d, i), C.money(d.cost), C.tokens(d.tokens) + ' tokens')}></div>`).join('');
    let labels;
    if (!dense) labels = `<div class="daylab" style="${cols}">${daily.map((d, i) => `<span>${esc(dayName(d, i))}</span>`).join('')}</div>`;
    else {
      const picks = [0, Math.floor((daily.length - 1) / 2), daily.length - 1];
      labels = `<div class="daylab dense">${picks.map((i) => `<span>${esc(dayName(daily[i], i))}</span>`).join('')}</div>`;
    }
    const peak = peakI >= 0 && max > 0 ? `peak ${C.money(max)} ${esc(dense ? C.dayMonth(C.isoDay(daily[peakI].day)) : C.weekday(C.isoDay(daily[peakI].day)))}` : '';
    const chartLabel = `Daily spend, ${daily.length} days${peak ? ', ' + peak : ''}`;
    const dailySec = `<div class="sec"><div class="sec-h"><span class="caps">Daily spend</span><span class="aside">${peak}</span></div>`
      + `<div class="days ${dense ? 'dense' : ''}" style="${cols}" role="img" aria-label="${esc(chartLabel)}">${bars}</div>${labels}</div>`;

    const ranked = (title, rows, mono) => {
      const top = Math.max(0, ...rows.map((r) => r.cost));
      return `<div class="rank"><span class="caps">${title}</span>${rows.length ? rows.slice(0, 4).map((r) => `<div class="rank-r"><div class="t"><span class="n ${mono ? 'mono' : ''}" title="${esc(r.name)}">${esc(r.name)}</span><span class="v">${C.money(r.cost)}</span></div>`
        + `<div class="thin"><div style="width:${top ? Math.round((r.cost / top) * 100) : 0}%"></div></div></div>`).join('') : '<span class="fine">None yet</span>'}</div>`;
    };
    const two = `<div class="two">${ranked('Top models', st.models || [], false)}${ranked('Top projects', st.projects || [], true)}</div>`;

    const kinds = st.kinds || [];
    const kmax = Math.max(0, ...kinds.map((k) => k.share));
    const half = Math.ceil(kinds.length / 2);
    const order = [];
    for (let i = 0; i < half; i++) { order.push(kinds[i]); if (kinds[i + half]) order.push(kinds[i + half]); }
    const kindSec = kinds.length ? `<div class="sec" style="gap:8px"><div class="sec-h"><span class="caps">Kind of work</span><span class="aside m">share of tokens</span></div>`
      + `<div class="kinds">${order.map((k) => `<div class="kind"><span class="n">${esc(k.kind)}</span><div class="b"><div style="width:${kmax ? Math.round((k.share / kmax) * 100) : 0}%"></div></div><span class="p">${Math.round(k.share)}%</span></div>`).join('')}</div></div>` : '';
    return kpis + dailySec + two + kindSec;
  }
  const isoToday = () => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`; };

  // ---------- Harness ----------
  function viewHarness() {
    const h = S.harness;
    const n = h?.harnesses?.length || 0;
    const right = h ? `<span class="bar-note">${plural(n, 'harness', 'harnesses')} found</span>` : '';
    let body;
    if (S.harnessErr) body = `<div class="msgbox"><h2>Couldn't load harnesses</h2>${esc(S.harnessErr)}</div>`;
    else if (!h) body = '<div class="msgbox" aria-busy="true">Loading…</div>';
    else if (!n) body = '<div class="msgbox"><h2>No harness activity</h2>Tokenrunway reads Claude Code, Codex, opencode, Gemini CLI and Cline session logs on this machine. Nothing in this period yet.</div>';
    else body = harnessBody(h);
    return bar(right) + `<div class="scroll" id="panel" role="tabpanel" aria-labelledby="tab-harness"><div class="pane" style="${S.loadingData ? 'opacity:.6' : ''}">${periodGroup('harness')}${body}</div></div>`;
  }

  function harnessBody(h) {
    const hs = h.harnesses.slice(0, 4);
    const cols = `grid-template-columns:70px repeat(${hs.length}, minmax(0, 1fr))`;
    const share = `<div class="sec" style="gap:8px"><div class="sec-h"><span class="caps">Share of spend</span><span class="aside">${C.money(h.totalCost)} API-equivalent</span></div>`
      + `<div class="share" role="img" aria-label="${esc(h.harnesses.map((x) => `${x.name} ${Math.round(x.share)}%`).join(', '))}">${h.harnesses.map((x) => `<div style="width:${x.share}%;--c:var(${C.harnessVar(x.id)})"${tipAttrs(x.name, Math.round(x.share) + '%', C.money(x.cost))}></div>`).join('')}</div></div>`;
    const head = `<div class="cmp-r head" style="${cols}"><span></span>${hs.map((x) => `<div class="hh" style="--c:var(${C.harnessVar(x.id)})"><span class="sw"></span><span class="n">${esc(x.name)}</span><span class="p">${Math.round(x.share)}%</span></div>`).join('')}</div>`;
    const lead = (vals, best) => {
      const ok = vals.map((v, i) => [v, i]).filter(([v]) => v != null && Number.isFinite(v));
      if (ok.length < 2) return -1;
      return ok.reduce((a, b) => ((best === 'min' ? b[0] < a[0] : b[0] > a[0]) ? b : a))[1];
    };
    const row = (label, vals, fmt, best, small) => {
      const li = best ? lead(vals, best) : -1;
      return `<div class="cmp-r" style="${cols}"><span class="lab">${label}</span>${vals.map((v, i) => `<span class="cell ${small ? 'sm' : ''} ${i === li ? 'lead' : ''}" title="${esc(fmt(v))}">${esc(fmt(v))}</span>`).join('')}</div>`;
    };
    const perSession = hs.map((x) => (x.sessions ? x.cost / x.sessions : null));
    const rows = [
      row('Spend', hs.map((x) => x.cost), (v) => C.money(v), 'max'),
      row('Tokens', hs.map((x) => x.tokens), C.tokens, 'max'),
      row('Sessions', hs.map((x) => x.sessions), (v) => String(v), 'max'),
      row('Avg session', hs.map((x) => x.avgSessionMs), C.minutes, 'max'),
      row('$ / session', perSession, (v) => (v == null ? '—' : C.money(v)), 'min'),
      row('Cache hit', hs.map((x) => x.cacheHit), (v) => Math.round(v) + '%', 'max'),
      row('Top model', hs.map((x) => x.topModel), (v) => v || '—', null, true),
      row('Top kind', hs.map((x) => x.topKind), (v) => v || '—', null, true),
    ].join('');
    const spark = `<div class="cmp-r spk" style="${cols}"><span class="lab" style="align-self:center">Daily shape</span>${hs.map((x) => {
      const d = x.daily || [];
      const m = Math.max(0, ...d);
      return `<div class="spark" style="--c:var(${C.harnessVar(x.id)})" role="img" aria-label="${esc(x.name)} daily spend, last ${d.length} days">${d.map((v, i) => `<div style="height:${m ? Math.max(2, Math.round((v / m) * 24)) : 2}px;opacity:${i === d.length - 1 ? 1 : 0.55}"${tipAttrs(x.name + (i === d.length - 1 ? ' · today' : ` · ${d.length - 1 - i} d ago`), C.money(v))}></div>`).join('')}</div>`;
    }).join('')}</div>`;
    return share + `<div class="cmp">${head}${rows}${spark}</div><p class="fine">Read locally from each harness's session logs. Bold marks the leader in each row.</p>`;
  }

  // ---------- Add account ----------
  const PROVIDERS = [
    { id: 'anthropic', m: 'C', n: 'Claude', d: 'Pro, Max, Team, Enterprise', site: 'claude.ai', tool: 'Claude Code', kind: 'claude-code', cmd: 'claude' },
    { id: 'openai', m: 'O', n: 'ChatGPT / Codex', d: 'Plus, Pro, Business', site: 'chatgpt.com', tool: 'Codex CLI', kind: 'codex', cmd: 'codex login' },
    { id: 'bedrock', m: 'B', n: 'AWS Bedrock', d: 'Budgets and on-demand spend', kind: 'aws' },
    { id: 'anthropic-api', m: 'A', n: 'Anthropic API', d: 'Console spend limits' },
    { id: 'openai-api', m: 'O', n: 'OpenAI API', d: 'Project budgets' },
  ];
  const SOON = [{ id: 'xai', m: 'X', n: 'Grok' }, { id: 'gemini', m: 'G', n: 'Gemini' }, { id: 'copilot', m: 'P', n: 'Copilot' }, { id: 'cursor', m: 'Cu', n: 'Cursor' }];

  function methodsFor(p, detected) {
    const det = detected.find((d) => d.kind === p.kind);
    const cli = () => ({ id: 'cli', n: 'Use my CLI login', rec: !!det, d: det ? `Found a ${det.label} on this ${machine()}. Nothing to type.` : `Uses your ${p.tool} login. Run ${p.cmd} first.` });
    const login = () => ({ id: 'login', n: 'Log in', soon: true, disabled: true, d: `Opens ${p.site} in your browser and returns here.` });
    const key = (d) => ({ id: 'key', n: 'Paste API key', d });
    const aws = (d, disabled) => ({ id: 'aws', n: 'Use AWS profile', d, disabled, rec: !disabled && !!det });
    switch (p.id) {
      case 'anthropic': return [cli(), login(), key('For Console orgs. Stored in the keychain, never shown again.'), aws('Only for Claude through Bedrock. Pick AWS Bedrock instead.', true)];
      case 'openai': return [cli(), login(), { ...key('For API projects. Pick OpenAI API instead.'), disabled: true }];
      case 'bedrock': return [aws(det ? `Found ${det.detail || 'profiles'}. Reads Budgets and Cost Explorer, nothing else.` : 'Uses a profile from ~/.aws. Reads Budgets and Cost Explorer, nothing else.')];
      case 'anthropic-api': return [key('An Admin API key from console.anthropic.com. Stored in the keychain, never shown again.')];
      default: return [key('An admin key from platform.openai.com. Stored in the keychain, never shown again.')];
    }
  }

  async function startAdd() {
    S.add = { step: 'providers', search: '', detected: S.app?.detected || [], error: '' };
    go('add');
    try { S.add.detected = await invoke('list_detected'); } catch { /* keep state.detected */ }
  }
  function pickProvider(id) {
    const p = PROVIDERS.find((x) => x.id === id);
    const methods = methodsFor(p, S.add.detected);
    const first = methods.find((m) => m.rec && !m.disabled) || methods.find((m) => !m.disabled);
    Object.assign(S.add, { step: 'method', provider: id, method: first?.id, label: '', key: '', profile: '', error: '' });
    render();
    if (methods.some((m) => m.id === 'aws' && !m.disabled)) loadProfiles();
    setTimeout(() => document.querySelector('[data-k="m-' + (first?.id || '') + '"]')?.focus(), 0);
  }
  async function loadProfiles() {
    if (S.add.profiles) return;
    try { S.add.profiles = await invoke('list_aws_profiles'); } catch { S.add.profiles = []; }
    if (!S.add.profile && S.add.profiles.length) S.add.profile = S.add.profiles[0];
    if (S.view === 'add') softRender();
  }

  function viewAdd() {
    const A = S.add;
    if (A.step === 'method') return viewMethod();
    if (A.step === 'done') return viewAdded();
    const s = A.search.trim().toLowerCase();
    const match = (p) => !s || p.n.toLowerCase().includes(s) || (p.d || '').toLowerCase().includes(s);
    const list = PROVIDERS.filter(match);
    const soon = SOON.filter(match);
    return subbar(tabLabel(), 'Add account')
      + `<div style="padding:12px 14px 6px;flex-shrink:0"><label for="psearch" class="sr-only">Search providers</label><input id="psearch" class="field" type="search" placeholder="Search providers" value="${esc(A.search)}" data-k="psearch" data-bind="add.search" autocomplete="off" spellcheck="false" style="height:28px;border-radius:7px;padding:0 10px"></div>`
      + `<div class="scroll"><div class="provs"><span class="caps">Subscriptions &amp; APIs</span>${list.map((p) => `<button class="prov" data-act="pick-provider" data-v="${p.id}" data-k="prov-${p.id}">${tile(p.id, p.m)}`
        + `<span class="t"><b>${esc(p.n)}</b><span>${esc(p.d)}</span></span>${ICON.chevron()}</button>`).join('') || '<p class="fine" style="padding:6px">No provider matches.</p>'}`
      + (soon.length ? `<span class="caps" style="padding:10px 6px 4px">Coming soon</span><div class="soon">${soon.map((p) => `<div>${tile(p.id, p.m, 's20')}<span>${esc(p.n)}</span></div>`).join('')}</div>` : '')
      + '</div></div>';
  }

  function viewMethod() {
    const A = S.add;
    const p = PROVIDERS.find((x) => x.id === A.provider);
    const methods = methodsFor(p, A.detected);
    const ms = methods.map((m) => {
      const on = A.method === m.id;
      return `<button class="method" role="radio" aria-checked="${on}" tabindex="${on ? 0 : -1}" ${m.disabled ? 'disabled' : ''} data-act="pick-method" data-v="${m.id}" data-k="m-${m.id}">`
        + `<span class="radio" aria-hidden="true"></span><span class="t"><span class="h">${esc(m.n)}${m.rec ? '<span class="badge">Recommended</span>' : ''}${m.soon ? '<span class="badge muted">Coming soon</span>' : ''}</span><span class="d">${esc(m.d)}</span></span></button>`;
    }).join('');
    let extra = '';
    if (A.method === 'key') extra = `<div class="formrow"><label for="akey">API key</label><input id="akey" class="field" type="password" autocomplete="off" spellcheck="false" placeholder="${p.id.startsWith('openai') ? 'sk-admin-…' : 'sk-ant-admin…'}" value="${esc(A.key)}" data-k="akey" data-bind="add.key"></div>`;
    if (A.method === 'aws') {
      const profs = A.profiles;
      extra = `<div class="formrow"><label for="aprof">AWS profile</label>${profs && profs.length
        ? `<select id="aprof" class="field" data-k="aprof" data-bind="add.profile">${profs.map((x) => `<option ${x === A.profile ? 'selected' : ''}>${esc(x)}</option>`).join('')}</select>`
        : `<input id="aprof" class="field" placeholder="${profs ? 'default' : 'Looking in ~/.aws…'}" value="${esc(A.profile)}" data-k="aprof" data-bind="add.profile" spellcheck="false">`}</div>`;
    }
    const ready = A.method && (A.method !== 'key' || A.key.trim().length > 8) && !A.busy;
    return subbar('Providers', `${tile(p.id, p.m, 's16')}${esc(p.n)}`, 'add-providers')
      + `<div class="scroll"><div style="padding:12px 10px;display:flex;flex-direction:column;gap:6px;min-height:100%">`
      + `<div class="methods" role="radiogroup" aria-label="Connection method">${ms}</div>`
      + `<div style="display:flex;flex-direction:column;gap:8px;margin-top:8px;padding:0 4px">`
      + `<div class="formrow"><label for="aname">Account name</label><input id="aname" class="field" placeholder="${esc(SHORT[p.id] || p.n)} — work" value="${esc(A.label)}" data-k="aname" data-bind="add.label" spellcheck="false"></div>${extra}</div>`
      + '<p class="fine" style="margin:6px 4px 0;line-height:1.45">Read-only. Tokenrunway reads usage pages and local logs; it never sends prompts, and keys stay in the system keychain.</p>'
      + (A.error ? `<p class="err" style="margin:4px" role="alert">${esc(A.error)}</p>` : '')
      + '<span class="flex"></span><div style="display:flex;justify-content:flex-end;gap:8px;padding:8px 4px 2px">'
      + '<button class="btn s30" data-act="cancel" data-k="cancel">Cancel</button>'
      + `<button class="btn s30 accent" style="padding:0 18px" data-act="connect" data-k="connect" ${ready ? '' : 'disabled'}>${A.busy ? 'Connecting…' : 'Connect'}</button></div></div></div>`;
  }

  function viewAdded() {
    const A = S.add;
    const acc = A.result || {};
    const how = A.method === 'cli' ? (A.detected.find((d) => d.provider === acc.provider)?.label || 'CLI login')
      : A.method === 'aws' ? `AWS profile ${A.profile || 'default'}` : A.method === 'key' ? 'API key' : 'login';
    const wins = acc.windows || [];
    const sub = [acc.plan, wins.length ? `${plural(wins.length, 'usage window')} found` : 'No usage windows yet', `via ${how}`].filter(Boolean).join(' · ');
    const rows = wins.map((w) => `<div class="win st-${esc(w.status)}"><div class="win-h"><span class="wl">${esc(w.label)}</span><span class="wp">${Math.round(w.pct)}%</span></div>`
      + ubar(w, { noProj: true, noPace: true })
      + (w.resetsAt ? `<span style="font-size:11.5px;color:var(--fg2)" data-reset="${w.resetsAt}">${C.resetText(w.resetsAt)}</span>` : '') + '</div>').join('');
    return `<div class="scroll"><div style="padding:34px 14px 12px;display:flex;flex-direction:column;align-items:center;gap:12px;min-height:100%">`
      + `<span class="done-ic">${ICON.check()}</span>`
      + `<div style="display:flex;flex-direction:column;align-items:center;gap:3px;text-align:center"><h2 style="margin:0;font-size:17px;font-weight:600" tabindex="-1" data-k="done-h">${esc(acc.name || 'Account')} is connected</h2><span style="font-size:12.5px;color:var(--fg2)">${esc(sub)}</span></div>`
      + `<article class="card" style="align-self:stretch;margin-top:8px"><div style="display:flex;align-items:baseline;justify-content:space-between"><span class="caps">First reading</span>`
      + `<span style="font-size:11px;color:var(--fg3)" data-ago="${acc.updatedAt || Date.now()}">${C.ago(acc.updatedAt || Date.now())}</span></div>${rows}`
      + '<p class="fine">Forecasts appear after a few more readings (about 10 minutes).</p></article>'
      + '<span class="flex"></span><div style="align-self:stretch;display:flex;gap:8px">'
      + '<button class="btn s30" style="flex:1" data-act="add" data-k="again">Add another</button><button class="btn s30 accent" style="flex:1" data-act="done" data-k="done">Done</button></div></div></div>';
  }

  async function connect() {
    const A = S.add;
    A.busy = true; A.error = ''; render();
    try {
      const res = await invoke('add_account', {
        provider: A.provider, method: A.method, label: A.label.trim() || null,
        key: A.method === 'key' ? A.key.trim() : null, profile: A.method === 'aws' ? (A.profile || null) : null,
      });
      A.key = '';
      A.result = res && res.windows ? res : (S.app?.accounts || []).find((x) => x.id === res?.id || x.id === res) || { name: A.label || SHORT[A.provider], provider: A.provider, windows: [] };
      A.step = 'done';
      render();
      document.querySelector('[data-k="done-h"]')?.focus();
      announce(`${A.result.name} is connected`);
    } catch (e) {
      A.error = String(e?.message || e);
      render();
    } finally { A.busy = false; if (A.step === 'method') render(); }
  }

  async function addDetected(kind) {
    const d = (S.app?.detected || []).find((x) => x.kind === kind);
    if (!d) return;
    S.busy.add('det-' + kind); S.addErr = ''; render();
    const method = kind === 'aws' ? 'aws' : 'cli';
    try {
      const res = await invoke('add_account', { provider: d.provider, method, label: null, key: null, profile: null });
      S.add = { step: 'done', method, provider: d.provider, detected: S.app.detected, profile: '', result: res && res.windows ? res : { name: d.label, provider: d.provider, windows: [] } };
      S.busy.delete('det-' + kind);
      go('add');
    } catch (e) {
      S.busy.delete('det-' + kind);
      S.addErr = String(e?.message || e);
      render();
    }
  }

  // ---------- Request more ----------
  const CHANNEL = { provider: (p) => SHORT[p] || 'Provider', email: () => 'Email', slack: () => 'Slack', teams: () => 'Teams' };
  const toFor = (to, ch) => (ch === 'email' ? to.email : ch === 'slack' ? to.slack || (to.name ? '@' + to.name : '') : ch === 'teams' ? to.teams || to.email : '');

  async function openRequest(accountId, windowId) {
    S.req = { accountId, windowId, loading: true, error: '', status: '' };
    go('request');
    try {
      const d = await invoke('request_draft', { accountId, windowId });
      Object.assign(S.req, { loading: false, draft: d, channel: d.channels?.[0] || 'email', body: d.body, to: {} });
      for (const ch of d.channels || []) S.req.to[ch] = toFor(d.to || {}, ch);
      render();
    } catch (e) { Object.assign(S.req, { loading: false, error: String(e?.message || e) }); render(); }
  }

  function viewRequest() {
    const R = S.req;
    const head = subbar('Now', 'Request more');
    if (R.loading) return head + '<div class="msgbox" aria-busy="true">Writing the request…</div>';
    if (!R.draft) return head + `<div class="msgbox"><h2>Couldn't write the request</h2>${esc(R.error)}</div>`;
    const d = R.draft;
    const acc = (S.app?.accounts || []).find((x) => x.id === d.accountId) || {};
    const w = (acc.windows || []).find((x) => x.id === R.windowId);
    const warn = w && w.status !== 'crit';
    const label = (ch) => CHANNEL[ch]?.(acc.provider) || ch;
    const ch = R.channel;
    const provider = ch === 'provider';
    const sendLabel = provider ? `Request in ${label(ch)}` : `Send via ${label(ch)}`;
    const ph = { email: 'name@company.com', slack: '@name or #channel', teams: 'name@company.com or chat link' }[ch] || '';
    return head + '<div style="flex:1;min-height:0;padding:12px 14px;display:flex;flex-direction:column;gap:12px">'
      + `<div class="req-h ${warn ? 'warn' : ''}">${tile(acc.provider, acc.mono, 's22')}<div class="t"><b>${esc(d.title)}</b><span>${esc(d.forecast)}</span></div></div>`
      + `<div class="lab6"><span class="lbl" id="via-l">Send via</span><div class="seg" role="radiogroup" aria-labelledby="via-l" style="grid-auto-columns:minmax(0,1fr)">${(d.channels || []).map((c) => {
        const on = c === ch;
        return `<button role="radio" aria-checked="${on}" tabindex="${on ? 0 : -1}" data-act="channel" data-v="${esc(c)}" data-k="ch-${esc(c)}">${esc(label(c))}</button>`;
      }).join('')}</div></div>`
      + (provider
        ? `<p class="fine" style="font-size:11.5px;color:var(--fg2)">Uses ${esc(label(ch))}'s own request flow, signed in as you. Your admin gets it there.</p>`
        : `<div class="formrow" style="grid-template-columns:44px 1fr"><label for="rto">To</label><input id="rto" class="field" value="${esc(R.to[ch] || '')}" placeholder="${esc(ph)}" data-k="rto" data-bind="req.to.${esc(ch)}" spellcheck="false"></div>`)
      + `<div class="lab6" style="flex:1;min-height:0"><label for="rmsg">Message</label><textarea id="rmsg" class="field" style="flex:1;min-height:0" data-k="rmsg" data-bind="req.body">${esc(R.body)}</textarea></div>`
      + '<label class="check" title="Coming soon"><input type="checkbox" disabled>Attach usage snapshot (image) · coming soon</label>'
      + (R.error ? `<p class="err" role="alert">${esc(R.error)}</p>` : '')
      + `<div style="display:flex;gap:8px;align-items:center"><button class="btn s30" data-act="copy" data-k="copy">${R.status === 'copied' ? 'Copied' : 'Copy'}</button>`
      + `<span class="flex"></span>${R.status === 'sent' ? '<span class="ok-note" role="status">Sent</span>' : ''}`
      + `<button class="btn s30 accent icon" style="padding:0 16px" data-act="send" data-k="send" ${R.status === 'sending' || (!provider && !(R.to[ch] || '').trim()) ? 'disabled' : ''}>${R.status === 'sending' ? 'Sending…' : esc(sendLabel)}${ICON.send()}</button></div></div>`;
  }

  // ---------- Settings ----------
  const sw = (name, on, label) => `<button class="switch" role="switch" aria-checked="${!!on}" aria-label="${esc(label)}" data-act="toggle" data-name="${name}" data-k="sw-${name}"></button>`;
  const numField = (name, value, label, min, max, unit) => `<input class="field num" type="number" inputmode="numeric" min="${min}" max="${max}" value="${esc(value)}" aria-label="${esc(label)}" data-k="n-${name}" data-num="${name}"><span class="unit">${unit}</span>`;

  function viewSettings() {
    const st = S.settings || S.app?.settings || {};
    const accs = S.app?.accounts || [];
    const admins = st.admins || {};
    const accRows = accs.map((a) => {
      const id = a.id;
      const ad = admins[id] || {};
      const open = S.openAdmin.has(id);
      const confirm = S.confirmRemove === id;
      return `<div class="acc"><div class="acc-h">${tile(a.provider, a.mono, 's20')}`
        + `<input class="field" value="${esc(S.renames[id] ?? a.name)}" aria-label="Name of ${esc(a.name)}" data-k="ren-${esc(id)}" data-rename="${esc(id)}" spellcheck="false">`
        + (a.plan ? `<span class="plan">${esc(a.plan)}</span>` : '')
        + `<button class="ib sm" data-act="admin" data-id="${esc(id)}" data-k="adm-${esc(id)}" aria-expanded="${open}" aria-label="Admin contact for ${esc(a.name)}" title="Admin contact">${open ? ICON.down() : ICON.chevron(11)}</button>`
        + `<button class="ib sm" data-act="remove" data-id="${esc(id)}" data-k="rm-${esc(id)}" aria-label="Remove ${esc(a.name)}">${ICON.trash()}</button></div>`
        + (confirm ? `<div style="display:flex;align-items:center;gap:6px;padding-left:28px"><span style="flex:1;font-size:12px;color:var(--fg2)">Remove ${esc(a.name)}? Its key leaves the keychain.</span>`
          + `<button class="btn s24" data-act="remove-no" data-k="rmno-${esc(id)}">Cancel</button><button class="btn s24" style="background:var(--crit);color:#fff;font-weight:600" data-act="remove-yes" data-id="${esc(id)}" data-k="rmyes-${esc(id)}">Remove</button></div>` : '')
        + (open ? `<div class="admin"><span class="caps" style="grid-column:1/-1">Admin contact, for Request more</span>${['name', 'email', 'slack', 'teams'].map((f) => `<label for="ad-${esc(id)}-${f}">${f[0].toUpperCase() + f.slice(1)}</label>`
          + `<input id="ad-${esc(id)}-${f}" class="field" value="${esc(ad[f] || '')}" placeholder="${{ name: 'Jordan Lee', email: 'jordan@company.com', slack: '@jordan or a link', teams: 'Teams chat link or email' }[f]}" data-k="ad-${esc(id)}-${f}" data-admin="${esc(id)}" data-field="${f}" spellcheck="false">`).join('')}</div>` : '')
        + '</div>';
    }).join('');
    const paused = st.pausedUntil && st.pausedUntil > Date.now();
    return subbar(tabLabel(), 'Settings', 'back') + '<div class="scroll"><div class="set">'
      + `<div class="group"><span class="caps">Accounts</span><div class="box">${accRows}`
      + `<div class="srow"><button class="btn icon s24" data-act="add" data-k="set-add">${ICON.plus(12, 1.8)}Add account</button></div></div></div>`
      + '<div class="group"><span class="caps">Menubar and popover</span><div class="box">'
      + `<div class="srow"><span class="t"><b>Menubar label</b><span>Most urgent window</span></span>${seg('Menubar label', 'menubarLabel', [['none', 'None'], ['percent', '% left'], ['runway', 'Runway']], st.menubarLabel)}</div>`
      + `<div class="srow"><span class="t"><b>Density</b></span>${seg('Density', 'density', [['comfortable', 'Comfortable'], ['compact', 'Compact']], st.density)}</div></div></div>`
      + '<div class="group"><span class="caps">Notifications</span><div class="box">'
      + (paused ? `<div class="srow"><span class="t"><b>Paused</b><span>${st.pausedUntil > 8e15 ? 'Until you resume' : 'Until ' + esc(C.when(st.pausedUntil))}</span></span><button class="btn s24" data-act="resume" data-k="resume">Resume</button></div>` : '')
      + `<div class="srow"><span class="t"><b>Thresholds</b><span>When a window passes the warning or critical level</span></span>${sw('notifyThresholds', st.notifyThresholds, 'Notify at thresholds')}</div>`
      + `<div class="srow"><span class="t"><b>Warning at</b></span>${numField('warnAt', st.warnAt, 'Warning at percent used', 1, 100, '%')}</div>`
      + `<div class="srow"><span class="t"><b>Critical at</b></span>${numField('critAt', st.critAt, 'Critical at percent used', 1, 100, '%')}</div>`
      + `<div class="srow"><span class="t"><b>Projected to run out</b><span>When the pace runs out before the reset</span></span>${sw('notifyProjection', st.notifyProjection, 'Notify when projected to run out')}</div>`
      + `<div class="srow"><span class="t"><b>Lead time</b><span>Warn if running out this long before reset</span></span>${numField('leadMinutes', st.leadMinutes, 'Lead time in minutes', 5, 1440, 'min')}</div>`
      + `<div class="srow"><span class="t"><b>Window resets</b></span>${sw('notifyReset', st.notifyReset, 'Notify when a window resets')}</div>`
      + `<div class="srow"><span class="t"><b>Offer Request more at</b></span>${numField('requestAt', st.requestAt, 'Offer request more at percent used', 1, 100, '%')}</div></div></div>`
      + '<div class="group"><span class="caps">Request messages</span><div class="box">'
      + `<div class="srow"><span class="t"><b>Your name</b><span>Signed under request messages</span></span><input class="field" style="width:130px" value="${esc(st.myName || '')}" placeholder="Sam" aria-label="Your name" data-k="myName" data-text="myName" spellcheck="false"></div></div></div>`
      + '<div class="group"><span class="caps">General</span><div class="box">'
      + `<div class="srow"><span class="t"><b>Start at login</b></span>${sw('startAtLogin', st.startAtLogin, 'Start at login')}</div>`
      + `<div class="srow"><span class="t"><b>Check for updates</b><span>Daily, from GitHub releases</span></span>${sw('updateCheck', st.updateCheck, 'Check for updates automatically')}</div>`
      + `<div class="srow"><span class="t"><b>Tokenrunway v${esc(S.app?.version || '')}</b><span>${S.app?.update ? `v${esc(S.app.update)} is available` : 'MIT licence · made by iokig'}</span></span><button class="btn s24" data-act="view" data-v="update" data-k="set-upd">${S.app?.update ? 'Update' : 'Check now'}</button></div>`
      + `<div class="srow"><span class="t"><b>iokig.com</b><span>More tools and the changelog</span></span><button class="btn s24" data-act="site" data-k="set-site">Open</button></div>`
      + '</div></div></div></div>';
  }

  async function setSettings(patch) {
    S.settings = { ...(S.settings || S.app?.settings || {}), ...patch };
    if (S.app) S.app.settings = { ...S.app.settings, ...patch };
    try { await invoke('set_settings', { patch }); } catch (e) { announce(`Couldn't save: ${e}`); }
  }

  // ---------- Update ----------
  // Release notes are short Markdown: paragraphs, "- " lists, "#" headings, **bold** and `code`.
  function md(text) {
    const inline = (s) => esc(s).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/`(.+?)`/g, '<code>$1</code>');
    let html = '', list = false;
    for (const line of String(text || '').split('\n')) {
      const item = line.match(/^\s*[-*] (.*)/);
      if (item && !list) { html += '<ul>'; list = true; }
      if (!item && list) { html += '</ul>'; list = false; }
      if (item) html += `<li>${inline(item[1])}</li>`;
      else if (/^#+ /.test(line)) html += `<p><b>${inline(line.replace(/^#+ /, ''))}</b></p>`;
      else if (line.trim()) html += `<p>${inline(line)}</p>`;
    }
    return html + (list ? '</ul>' : '');
  }

  async function checkUpdates() {
    S.upd = { phase: 'checking' };
    if (S.view === 'update') render();
    try {
      const checked = await invoke('check_for_updates');
      if (!checked.newer) { S.upd = { phase: 'current', checked }; }
      else {
        let notes = [];
        try { notes = await invoke('update_notes'); } catch { /* notes are optional */ }
        S.upd = { phase: 'available', checked, notes };
      }
    } catch (e) { S.upd = { phase: 'error', err: String(e?.message || e) }; }
    if (S.view === 'update') render();
  }

  function viewUpdate() {
    const U = S.upd;
    const head = subbar(tabLabel(), 'Updates');
    let body;
    if (U.phase === 'checking' || U.phase === 'idle') body = `<p class="hint" role="status" style="display:flex;align-items:center;gap:6px">${ICON.spinner()}Checking for updates…</p>`;
    else if (U.phase === 'error') body = `<div class="card"><h3>Couldn't check</h3><p class="hint">${esc(U.err)}</p><div><button class="btn" data-act="recheck" data-k="recheck">Try again</button></div></div>`;
    else if (U.phase === 'current') body = `<div class="card"><h3>You're up to date</h3><p class="hint">Tokenrunway v${esc(U.checked.current)} is the latest version.</p></div>`;
    else {
      const how = S.app?.updateHow || 'download';
      const action = how === 'install'
        ? `<button class="btn s30 accent" data-act="install" data-k="install" ${U.installing ? 'disabled' : ''}>${U.installing ? 'Installing…' : 'Update and restart'}</button>`
        : how === 'homebrew' ? '<p class="hint">Update with <code>brew upgrade --cask tokenrunway</code></p>'
          : '<button class="btn s30 accent" data-act="download" data-k="download">Download</button>';
      const prog = U.progress != null ? `<div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${U.progress}" aria-label="Download"><div style="width:${U.progress}%"></div></div>` : '';
      body = `<div class="card"><h3>v${esc(U.checked.latest)} is available</h3><p class="hint">You have v${esc(U.checked.current)}.</p><div>${action}</div>${prog}<p class="hint" id="progress" role="status">${esc(U.msg || '')}</p></div>`
        + (U.notes || []).map((n) => `<div class="card"><h3>v${esc(n.version)} <span class="hint" style="font-weight:400">${esc(n.date)}</span></h3><div class="notes">${md(n.notes) || '<p>No notes.</p>'}</div></div>`).join('');
    }
    return head + `<div class="scroll"><div class="upd">${body}</div></div>`;
  }

  // ---------- navigation ----------
  const tabLabel = () => TABS.find(([id]) => id === S.tab)?.[1] || 'Now';
  function go(view) {
    if (TABS.some(([id]) => id === view)) S.tab = view;
    S.view = view;
    C.closeMenu(); C.tip.hide();
    render();
    if (view === 'stats' && (!S.stats || S.stats.period !== S.period.stats)) loadData('stats');
    if (view === 'harness' && (!S.harness || S.harness.period !== S.period.harness)) loadData('harness');
    if (view === 'settings') { S.settings = { ...(S.app?.settings || {}) }; invoke('get_settings').then((s) => { S.settings = s; if (S.view === 'settings') softRender(); }).catch(() => {}); }
    if (view === 'update' && S.upd.phase !== 'available') checkUpdates();
  }
  async function loadData(which) {
    S.loadingData = !!S[which];
    if (S.view === which) render();
    const period = S.period[which];
    try {
      const r = await invoke(which === 'stats' ? 'get_stats' : 'get_harness', { period });
      if (period !== S.period[which]) return;
      S[which] = r; S[which + 'Err'] = '';
    } catch (e) { S[which + 'Err'] = String(e?.message || e); }
    S.loadingData = false;
    if (S.view === which) softRender();
  }
  async function refresh(accountId) {
    const key = accountId || 'all';
    if (S.busy.has(key)) return;
    S.busy.add(key); render();
    try { await invoke('refresh', { accountId: accountId || null }); } catch (e) { announce(`Couldn't refresh: ${e}`); }
    S.busy.delete(key); render();
  }

  function show(view) {
    const v = String(view || 'now');
    if (v === 'add') startAdd();
    else if (v === 'request') { const a = S.app?.accounts.find((x) => x.windows?.some(wantsRequest)); if (a) openRequest(a.id, a.windows.find(wantsRequest).id); else go('now'); }
    else if (['now', 'stats', 'harness', 'settings', 'update'].includes(v)) go(v);
    else go('now');
  }

  // ---------- events ----------
  document.addEventListener('click', (e) => {
    const t = e.target.closest('[data-act]');
    if (!t || t.disabled) return;
    const { act, v, id } = t.dataset;
    switch (act) {
      case 'tab': if (S.view !== v) go(v); break;
      case 'view': go(v); break;
      case 'back': go(S.tab); break;
      case 'settings': go(S.view === 'settings' ? S.tab : 'settings'); break;
      case 'site': invoke('open_site'); break;
      case 'dashboard': invoke('open_dashboard'); break;
      case 'add': startAdd(); break;
      case 'add-detected': addDetected(t.dataset.kind); break;
      case 'refresh': refresh(id); break;
      case 'open-provider': invoke('open_provider', { accountId: id }); break;
      case 'login': invoke('login_account', { accountId: id }).catch((err) => announce(String(err))); break;
      case 'details': invoke('open_update', { url: null }); break;
      case 'request': openRequest(id, t.dataset.w); break;
      case 'more': {
        const acc = S.app.accounts.find((a) => a.id === id);
        const hot = acc?.windows.find(wantsRequest);
        C.menu(t, [
          { label: 'Open provider page', onSelect: () => invoke('open_provider', { accountId: id }) },
          { label: 'Refresh', onSelect: () => refresh(id) },
          ...(hot ? [{ label: 'Request more…', onSelect: () => openRequest(id, hot.id) }] : []),
        ]);
        break;
      }
      case 'period': if (S.period[t.dataset.w] !== v) { S.period[t.dataset.w] = v; loadData(t.dataset.w); } break;
      case 'pick-provider': pickProvider(v); break;
      case 'add-providers': S.add.step = 'providers'; render(); break;
      case 'pick-method': S.add.method = v; S.add.error = ''; render(); if (v === 'aws') loadProfiles(); break;
      case 'cancel': go(S.tab); break;
      case 'connect': connect(); break;
      case 'done': go('now'); break;
      case 'channel': S.req.channel = v; S.req.status = ''; render(); break;
      case 'copy':
        invoke('copy_text', { text: S.req.body }).then(() => { S.req.status = 'copied'; render(); announce('Copied'); setTimeout(() => { if (S.req?.status === 'copied') { S.req.status = ''; softRender(); } }, 1500); })
          .catch((err) => { S.req.error = String(err); render(); });
        break;
      case 'send': {
        const R = S.req;
        R.status = 'sending'; R.error = ''; render();
        invoke('send_request', { accountId: R.draft.accountId, channel: R.channel, to: R.channel === 'provider' ? '' : (R.to[R.channel] || '').trim(), body: R.body })
          .then(() => { R.status = 'sent'; render(); announce('Request sent'); setTimeout(() => { if (S.view === 'request' && R.status === 'sent') go('now'); }, 1200); })
          .catch((err) => { R.status = ''; R.error = String(err?.message || err); render(); });
        break;
      }
      case 'set': setSettings({ [t.dataset.name]: v }); render(); break;
      case 'toggle': setSettings({ [t.dataset.name]: t.getAttribute('aria-checked') !== 'true' }); render(); break;
      case 'resume': setSettings({ pausedUntil: 0 }); render(); break;
      case 'admin': S.openAdmin.has(id) ? S.openAdmin.delete(id) : S.openAdmin.add(id); render(); break;
      case 'remove': S.confirmRemove = id; render(); break;
      case 'remove-no': S.confirmRemove = null; render(); break;
      case 'remove-yes': S.confirmRemove = null; invoke('remove_account', { accountId: id }).catch((err) => announce(String(err))); break;
      case 'recheck': checkUpdates(); break;
      case 'install':
        S.upd.installing = true; S.upd.msg = ''; render();
        invoke('install_update').catch((err) => { S.upd.installing = false; S.upd.progress = null; S.upd.msg = String(err?.message || err); render(); });
        break;
      case 'download': invoke('open_update', { url: null }); break;
      default: break;
    }
  });

  // Two-way bindings for text fields (no re-render while typing, except where the view depends on it).
  document.addEventListener('input', (e) => {
    const t = e.target;
    const b = t.dataset.bind;
    if (b) {
      const path = b.split('.');
      let o = S;
      for (let i = 0; i < path.length - 1; i++) o = o[path[i]];
      o[path[path.length - 1]] = t.value;
      if (b === 'add.search') render();
      if (b === 'add.key') { const c = $('[data-k="connect"]'); if (c) c.disabled = !(S.add.key.trim().length > 8) || S.add.busy; }
      if (b.startsWith('req.to.')) { const s = $('[data-k="send"]'); if (s) s.disabled = !t.value.trim(); }
    }
    if (t.dataset.rename) S.renames[t.dataset.rename] = t.value;
  });
  document.addEventListener('change', (e) => {
    const t = e.target;
    if (t.dataset.bind === 'add.profile') S.add.profile = t.value;
    if (t.dataset.num) {
      const n = Number(t.value);
      if (Number.isFinite(n)) setSettings({ [t.dataset.num]: n });
    }
    if (t.dataset.text) setSettings({ [t.dataset.text]: t.value.trim() });
    if (t.dataset.admin) {
      const id = t.dataset.admin;
      const admins = { ...((S.settings || S.app.settings).admins || {}) };
      admins[id] = { name: '', email: '', slack: '', teams: '', ...(admins[id] || {}), [t.dataset.field]: t.value.trim() };
      setSettings({ admins });
    }
    if (t.dataset.rename) {
      const id = t.dataset.rename;
      const name = t.value.trim();
      delete S.renames[id];
      const acc = S.app.accounts.find((a) => a.id === id);
      if (name && acc && name !== acc.name) { acc.name = name; invoke('rename_account', { accountId: id, name }).catch((err) => announce(String(err))); }
    }
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.target.dataset?.rename) e.target.blur();
    if (e.key === 'Escape' && !document.querySelector('.tr-menu') && !['now', 'stats', 'harness'].includes(S.view)) { e.preventDefault(); go(S.tab); }
  });

  // Chart tooltips: any mark with data-tip-v.
  document.addEventListener('pointermove', (e) => {
    const m = e.target.closest?.('[data-tip-v]');
    if (!m) { C.tip.hide(); return; }
    C.tip.show(e.clientX, e.clientY, [{ head: true, label: m.dataset.tipH }, { value: m.dataset.tipV, label: m.dataset.tipL || null }]);
  });
  document.addEventListener('pointerleave', () => C.tip.hide());

  const applyState = (app) => {
    S.app = app;
    C.setOs(app.os);
    if (S.view === 'add' && S.add?.step === 'providers' && app.detected) S.add.detected = S.add.detected?.length ? S.add.detected : app.detected;
    softRender();
  };
  listen('tr://state', ({ payload }) => applyState(payload));
  listen('tr://info', () => invoke('get_state').then(applyState).catch(() => {}));
  listen('tr://stats-changed', () => {
    if (S.view === 'stats' || S.view === 'harness') loadData(S.view);
    if (S.view !== 'stats') S.stats = null;
    if (S.view !== 'harness') S.harness = null;
  });
  listen('tr://show', ({ payload }) => show(payload));
  listen('tr://update-progress', ({ payload: p }) => {
    S.upd.progress = p.total ? Math.round((p.got / p.total) * 100) : null;
    S.upd.msg = p.total ? `Downloading… ${S.upd.progress}%` : 'Downloading…';
    const bar = $('.progress > div');
    const msg = $('#progress');
    if (bar && msg) { bar.style.width = S.upd.progress + '%'; msg.textContent = S.upd.msg; } else if (S.view === 'update') render();
  });

  window.TR = { show: (v) => { show(v); invoke('get_state').then(applyState).catch(() => {}); } };

  render();
  try { applyState(await invoke('get_state')); } catch (e) { $('#view').innerHTML = bar() + `<div class="msgbox"><h2>Couldn't read the state</h2>${esc(e?.message || e)}</div>`; }
  const initial = C.q.get('view');
  if (initial) show(initial);
})();
