// The dashboard window (docs/design/Dashboard.dc.html): KPIs, stacked daily spend, the burn-down
// with its forecast cone, sessions, upcoming resets and the 26-week heatmap. Charts are inline SVG
// drawn here; every mark has a hover tooltip and the sessions table is the table view.
(async () => {
  const C = window.TRC;
  const { esc } = C;
  const { invoke, listen } = await C.api;
  const $ = (s) => document.querySelector(s);
  const NS = 'http://www.w3.org/2000/svg';
  if (C.q.get('titlebar') === 'overlay') document.documentElement.classList.add('tr-overlay');

  const PERIODS = [['7d', '7 d'], ['30d', '30 d'], ['90d', '90 d'], ['lifetime', 'Lifetime']];
  const GROUPS = [['model', 'Model'], ['harness', 'Harness'], ['category', 'Category']];
  // project / model: the dashboard narrowed to one (from the Projects card, or the popover's
  // Top models and Top projects rows). ins: the insight cards' data.
  const S = { period: '30d', group: 'model', account: null, data: null, app: null, project: null, model: null, ins: null };

  // ---------- colour follows the entity, never its rank ----------
  const FALLBACK = ['--p-gemini', '--h-opencode', '--p-copilot', '--p-cursor', '--p-grok'];
  const KIND = { Build: '--m-opus', 'Bug fix': '--m-sonnet', Discovery: '--m-codex', Refactor: '--p-gemini', Test: '--h-opencode', Review: '--p-copilot', Docs: '--p-cursor', Ops: '--p-grok', Planning: '--p-api', Chat: '--p-bedrock' };
  function seriesVar(group, name, i) {
    if (/^other$/i.test(name)) return '--m-other';
    if (group === 'category') return KIND[name] || FALLBACK[i % FALLBACK.length];
    if (group === 'harness') { const v = C.harnessVar(name); return v === '--m-other' ? FALLBACK[i % FALLBACK.length] : v; }
    const n = name.toLowerCase();
    if (n.includes('opus')) return '--m-opus';
    if (n.includes('sonnet')) return '--m-sonnet';
    if (n.includes('codex') || n.includes('gpt')) return '--m-codex';
    if (n.includes('gemini')) return '--p-gemini';
    if (n.includes('haiku')) return '--h-opencode';
    return FALLBACK[i % FALLBACK.length];
  }

  // ---------- controls ----------
  function radios(el, items, value, onPick) {
    el.innerHTML = items.map(([v, l]) => `<button role="radio" aria-checked="${v === value}" tabindex="${v === value ? 0 : -1}" data-v="${v}">${l}</button>`).join('');
    el.onclick = (e) => { const b = e.target.closest('[role=radio]'); if (b && b.dataset.v !== value) onPick(b.dataset.v); };
  }
  function controls() {
    radios($('#period'), PERIODS, S.period, (v) => { S.period = v; controls(); load(); });
    radios($('#group'), GROUPS, S.group, (v) => { S.group = v; controls(); load(); });
    const acc = (S.app?.accounts || []).find((a) => a.id === S.account);
    $('#account-l').textContent = acc ? acc.name : 'All accounts';
  }
  $('#account').addEventListener('click', (e) => {
    const accs = S.app?.accounts || [];
    C.menu(e.currentTarget, [
      { label: 'All accounts', checked: !S.account, onSelect: () => { S.account = null; controls(); load(); } },
      ...accs.map((a) => ({ label: a.name, checked: S.account === a.id, onSelect: () => { S.account = a.id; controls(); load(); } })),
    ]);
  });

  // ---------- data ----------
  let seq = 0;
  async function load() {
    const my = ++seq;
    $('#body').classList.add('loading');
    try {
      const [d, ins] = await Promise.all([
        invoke('get_dashboard', { period: S.period, group: S.group, account: S.account, project: S.project, model: S.model }),
        invoke('get_insights', { period: S.period, project: S.project }).catch(() => null),
      ]);
      if (my !== seq) return;
      S.data = d;
      S.ins = ins;
      draw();
    } catch (e) {
      if (my !== seq) return;
      $('#kpis').innerHTML = `<div class="kpi" style="grid-column:1/-1"><span class="k">Couldn't load the dashboard</span><span class="s">${esc(e?.message || e)}</span></div>`;
    }
    $('#body').classList.remove('loading');
  }

  function draw() {
    filters(); kpis(); stacked(); burndown(); sessions(); resets(); heatmap();
    projects(); recap(); value(); tips();
    C.tick();
  }

  // Whole dollars from $10; cents below, so a $0.12 Codex day doesn't read as $0.
  const cash = (n) => C.money(n, Math.abs(n ?? 0) >= 10 ? 0 : 2);
  const periodWords = () => ({ '7d': '7 days', '30d': '30 days', '90d': '90 days', lifetime: 'Lifetime' })[S.period];
  function kpis() {
    const d = S.data;
    const days = d.days || [];
    const spend = (d.series || []).reduce((s, x) => s + x.values.reduce((a, b) => a + b, 0), 0);
    const tokens = d.tokens ?? (d.heatmap || []).slice(-days.length).reduce((s, x) => s + x.tokens, 0);
    const sess = d.sessions || [];
    const avg = sess.length ? sess.reduce((s, x) => s + x.durationMs, 0) / sess.length : 0;
    const harnesses = new Set(sess.map((x) => x.harness)).size;
    const acc = (S.app?.accounts || []).find((a) => a.id === S.account);
    const next = (d.resets || [])[0];
    const tiles = [
      ['Spend · API-equivalent', cash(spend), `${periodWords()} · ${S.project || S.model ? esc(S.project || S.model) : acc ? esc(d.scope || acc.name) : 'all harnesses'}`, ''],
      ['Actually billed', d.billed != null ? C.money(d.billed, 2) : '—', d.billed != null ? esc(d.billedNote || 'where a source reports it') : 'No source reports billing', ''],
      ['Tokens', C.tokens(tokens), d.cacheShare != null ? `${Math.round(d.cacheShare)}% cache reads` : 'in range', ''],
      ['Sessions', String(d.sessionsTotal ?? sess.length), sess.length ? `avg ${C.minutes(avg)} · ${harnesses} harness${harnesses === 1 ? '' : 'es'}` : 'none in range', ''],
      ['Ran out', d.ranOut != null ? `${d.ranOut}×` : '—', d.ranOut != null ? esc(d.ranOutNote || 'in range') : 'not tracked yet', d.ranOut ? 'color:var(--crit-text)' : ''],
      ['Next reset', next ? `<span data-left="${next.resetsAt}">${C.dur(next.resetsAt - Date.now())}</span>` : '—', next ? `${esc(next.account)} · ${esc(C.when(next.resetsAt))}` : 'no windows', ''],
    ];
    $('#kpis').innerHTML = tiles.map(([k, v, s, st]) => `<div class="kpi"><span class="k">${k}</span><span class="v" style="${st}">${v}</span><span class="s">${s}</span></div>`).join('');
  }

  // ---------- stacked daily spend ----------
  const nice = (max) => {
    if (max <= 0) return 1;
    const raw = max / 3;
    const p = 10 ** Math.floor(Math.log10(raw));
    const m = raw / p;
    return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 2.5 ? 2.5 : m <= 5 ? 5 : 10) * p;
  };
  const el = (tag, attrs = {}, parent) => {
    const e = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
    parent?.appendChild(e);
    return e;
  };
  const dayLabel = (iso) => (iso === isoToday() ? 'Today' : C.dayMonth(C.isoDay(iso)));
  const isoToday = () => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`; };

  function stacked() {
    const d = S.data;
    const days = d.days || [];
    // Two series never share a colour: a family colour (Opus, Sonnet…) goes to the first of its
    // family, and later ones (Opus 4.8 after Opus 5.5) take the next free colour.
    const used = new Set();
    const POOL = ['--m-opus', '--m-sonnet', '--m-codex', ...FALLBACK, '--p-api', '--p-bedrock'];
    const series = (d.series || []).map((s, i) => {
      let c = seriesVar(S.group, s.name, i);
      if (used.has(c) && c !== '--m-other') c = POOL.find((v) => !used.has(v)) || c;
      used.add(c);
      return { ...s, c, total: s.values.reduce((a, b) => a + b, 0) };
    });
    $('#chart-t').textContent = `Daily spend by ${S.group}`;
    $('#chart-s').textContent = days.length ? `API-equivalent, ${C.dayMonth(C.isoDay(days[0]))} to today` : 'API-equivalent';
    const legend = $('#legend');
    legend.textContent = '';
    for (const s of series) {
      const sp = document.createElement('span');
      const i = document.createElement('i'); i.style.background = `var(${s.c})`;
      const b = document.createElement('b'); b.textContent = cash(s.total);
      sp.append(i, document.createTextNode(s.name + ' '), b);
      legend.appendChild(sp);
    }
    const box = $('#chart');
    box.textContent = '';
    if (!days.length) { box.innerHTML = '<div class="empty">No activity in this range.</div>'; $('#xl').textContent = ''; return; }
    const W = box.clientWidth, H = 158, L = 34, top = 8, plotH = 150;
    const totals = days.map((_, i) => series.reduce((s, x) => s + (x.values[i] || 0), 0));
    const step = nice(Math.max(...totals));
    const yMax = Math.max(step * 3, Math.ceil(Math.max(...totals) / step) * step);
    const y = (v) => top + plotH - (v / yMax) * plotH;
    const svg = el('svg', { width: W, height: H, role: 'img', 'aria-label': `Stacked daily spend by ${S.group}, ${days.length} days. ${series.map((s) => `${s.name} ${cash(s.total)}`).join(', ')}.` }, box);
    for (let v = 0; v <= yMax + 1e-9; v += step) {
      el('line', { x1: L, x2: W, y1: y(v), y2: y(v), stroke: 'var(--grid)', 'stroke-width': 1 }, svg);
      const t = el('text', { x: 0, y: y(v) + 3.5, 'font-size': 10.5, fill: 'var(--fg3)' }, svg);
      t.textContent = '$' + (step < 0.1 ? v.toFixed(2) : step < 1 ? v.toFixed(1) : Math.round(v));
    }
    const n = days.length;
    const gap = n <= 31 ? 4 : n <= 60 ? 2 : 1;
    const bw = Math.max(1, (W - L - gap * (n - 1)) / n);
    const today = isoToday();
    days.forEach((day, i) => {
      const x = L + i * (bw + gap);
      const g = el('g', { class: 'day', opacity: day === today ? 1 : 0.92 }, svg);
      let acc = 0;
      series.forEach((s) => {
        const v = s.values[i] || 0;
        if (v <= 0) return;
        const y0 = y(acc), y1 = y(acc + v);
        const h = Math.max(1, y0 - y1 - 1);
        el('rect', { class: 'seg-r', x, y: y0 - h, width: bw, height: h, rx: Math.min(1, bw / 2), fill: `var(${s.c})` }, g);
        acc += v;
      });
      const hit = el('rect', { class: 'hit', x: x - gap / 2, y: top, width: bw + gap, height: plotH }, g);
      hit.addEventListener('pointermove', (e) => {
        svg.querySelectorAll('g.on').forEach((o) => o.classList.remove('on'));
        g.classList.add('on');
        C.tip.show(e.clientX, e.clientY, [{ head: true, label: `${dayLabel(day)} · ${C.money(totals[i])}` }, ...series.map((s) => ({ color: s.c, value: C.money(s.values[i] || 0), label: s.name }))]);
      });
      hit.addEventListener('pointerleave', () => { g.classList.remove('on'); C.tip.hide(); });
    });
    const picks = n < 5 ? days.map((_, i) => i) : [0, 1, 2, 3, 4].map((k) => Math.round((k * (n - 1)) / 4));
    $('#xl').innerHTML = picks.map((i) => `<span>${esc(dayLabel(days[i]))}</span>`).join('');
  }

  // ---------- burn-down ----------
  function burndown() {
    const b = S.data.burndown;
    const box = $('#burn');
    box.textContent = '';
    $('#bd-t').textContent = b ? b.title : '';
    $('#bd-t').style.display = b ? '' : 'none';
    if (!b || !b.points?.length) { box.innerHTML = '<div class="empty">No allowance window to burn down yet.</div>'; return; }
    const X = (t) => 36 + ((t - b.start) / (b.reset - b.start)) * 270;
    const Y = (rem) => 12 + ((100 - rem) / 100) * 140;
    const pts = b.points.map(([t, used]) => [X(t), Y(100 - used)]);
    const [nx, ny] = pts[pts.length - 1];
    const nowT = b.points[b.points.length - 1][0];
    const nowRem = 100 - b.points[b.points.length - 1][1];
    const outX = b.outAt ? X(b.outAt) : null;
    const f = (n) => n.toFixed(1);
    const desc = `Remaining allowance from ${Math.round(100 - b.points[0][1])}% at ${C.clock(b.start)} to ${Math.round(nowRem)}% now; `
      + (b.outAt ? `forecast reaches zero at ${C.clock(b.outAt)}${b.cone ? `, range ${C.clock(b.cone[0])} to ${C.clock(b.cone[1])}` : ''}, before the ${C.clock(b.reset)} reset` : `on track for the ${C.clock(b.reset)} reset`);
    let svg = `<svg width="100%" height="170" viewBox="0 0 320 190" role="img" aria-label="${esc(desc)}">`
      + '<path d="M36 12H306M36 47H306M36 82H306M36 117H306M36 152H306" stroke="var(--grid)" stroke-width="1"/>'
      + '<text x="30" y="15" text-anchor="end" font-size="9.5" fill="var(--fg3)">100%</text><text x="30" y="85" text-anchor="end" font-size="9.5" fill="var(--fg3)">50%</text><text x="30" y="155" text-anchor="end" font-size="9.5" fill="var(--fg3)">0</text>';
    if (b.cone) svg += `<path d="M${f(nx)} ${f(ny)}L${f(Math.min(306, X(b.cone[0])))} 152L${f(Math.min(306, X(b.cone[1])))} 152Z" fill="var(--cone)"/>`;
    svg += `<path d="M${pts.map(([x, y]) => `${f(x)} ${f(y)}`).join('L')}" fill="none" stroke="var(--fg1)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>`;
    if (outX != null) svg += `<path d="M${f(nx)} ${f(ny)}L${f(Math.min(306, outX))} 152" fill="none" stroke="var(--crit)" stroke-width="2" stroke-dasharray="4 3" stroke-linecap="round"/>`;
    else {
      const rate = (100 - nowRem - b.points[0][1]) / Math.max(1, nowT - b.start);
      const endRem = Math.max(0, nowRem - rate * (b.reset - nowT));
      svg += `<path d="M${f(nx)} ${f(ny)}L306 ${f(Y(endRem))}" fill="none" stroke="var(--ok)" stroke-width="2" stroke-dasharray="4 3" stroke-linecap="round"/>`;
    }
    svg += `<circle cx="${f(nx)}" cy="${f(ny)}" r="3.5" fill="var(--fg1)"/><path d="M${f(nx)} 12V152" stroke="var(--fg3)" stroke-width="1" stroke-dasharray="2 3"/>`
      + '<path d="M306 12V152" stroke="var(--ok)" stroke-width="1.5" stroke-dasharray="3 3"/>';
    if (outX != null && outX <= 306) svg += `<circle cx="${f(outX)}" cy="152" r="4" fill="var(--crit)"/><text x="${f(Math.min(290, Math.max(60, outX)))}" y="140" text-anchor="middle" font-size="10" font-weight="600" fill="var(--crit-text)">Out ${esc(C.clock(b.outAt))}</text>`;
    const nowLabelX = Math.min(250, Math.max(70, nx));
    svg += `<text x="${f(nowLabelX)}" y="170" text-anchor="middle" font-size="9.5" fill="var(--fg2)">now ${esc(C.clock(nowT))}</text>`
      + `<text x="36" y="170" text-anchor="start" font-size="9.5" fill="var(--fg3)">${esc(C.clock(b.start))}</text>`
      + `<text x="306" y="170" text-anchor="end" font-size="9.5" fill="var(--ok-text)">reset ${esc(C.clock(b.reset))}</text>`;
    if (b.cone) svg += `<text x="171" y="186" text-anchor="middle" font-size="9.5" fill="var(--fg3)">cone = ${esc(C.clock(b.cone[0]))} to ${esc(C.clock(b.cone[1]))} at recent pace</text>`;
    // Hover targets on each reading.
    svg += pts.map(([x, y], i) => `<circle cx="${f(x)}" cy="${f(y)}" r="9" fill="transparent" data-i="${i}"/>`).join('');
    box.innerHTML = svg + '</svg>';
    box.querySelectorAll('circle[data-i]').forEach((c) => {
      c.addEventListener('pointermove', (e) => { const [t, used] = b.points[+c.dataset.i]; C.tip.show(e.clientX, e.clientY, [{ head: true, label: C.clock(t) }, { value: `${Math.round(100 - used)}%`, label: 'left' }]); });
      c.addEventListener('pointerleave', () => C.tip.hide());
    });
  }

  // ---------- sessions ----------
  function startedText(ms) {
    const dd = C.dayDiff(ms);
    if (dd === 0) return C.clock(ms);
    if (dd > -7) return `${C.weekday(ms)} ${C.clock(ms)}`;
    return C.dayMonth(ms);
  }
  function sessions() {
    const d = S.data;
    const rows = d.sessions || [];
    $('#sess-n').textContent = `${d.sessionsTotal ?? rows.length} in range · newest first`;
    $('#sessions').innerHTML = rows.length ? rows.map((s) => `<tr><td class="dim">${esc(startedText(s.started))}</td>`
      + `<td><span class="dot" style="--c:var(${C.harnessVar(s.harness)})"></span>${esc(s.harness)}</td>`
      + `<td class="mono">${esc(s.project)}</td><td class="dim">${esc(s.model)}</td><td class="dim">${esc(s.kind)}</td>`
      + `<td class="r dim">${esc(C.minutes(s.durationMs))}</td><td class="r">${esc(C.tokens(s.tokens))}</td><td class="r cost">${esc(C.isLocal(s.harness) ? 'Free' : C.money(s.cost))}</td></tr>`).join('')
      : '<tr><td colspan="8" class="dim" style="text-align:center;padding:24px">No sessions in this range.</td></tr>';
  }

  // ---------- resets ----------
  function resets() {
    const rows = (S.data.resets || []).slice(0, 5);
    const st = S.app?.settings || { warnAt: 75, critAt: 90 };
    $('#resets').innerHTML = rows.length ? rows.map((r) => {
      const status = r.status || (r.pct >= st.critAt ? 'crit' : r.pct >= st.warnAt ? 'warn' : 'ok');
      const color = status === 'crit' ? 'var(--crit-text)' : status === 'warn' ? 'var(--warn-text)' : 'var(--fg2)';
      const icon = status === 'crit' ? C.ICON.crit(11, 1.8) : status === 'warn' ? C.ICON.warn(11) : '';
      return `<div class="rr"><span class="at"><b>${esc(C.when(r.resetsAt))}</b><span data-in="${r.resetsAt}">in ${esc(C.dur(r.resetsAt - Date.now()))}</span></span>`
        + `<span class="what"><span class="dot" style="--c:var(${C.providerVar(r.provider)})"></span>${esc(r.account)} · ${esc(r.window)}</span>`
        + `<span class="used" style="color:${color}">${icon}${Math.round(r.pct)}%</span></div>`;
    }).join('') : '<div class="empty" style="padding:8px 0">No allowance windows yet.</div>';
  }

  // ---------- heatmap ----------
  function heatmap() {
    const days = S.data.heatmap || [];
    const box = $('#heat');
    box.textContent = '';
    if (!days.length) return;
    const vals = days.map((d) => d.tokens).filter((v) => v > 0).sort((a, b) => a - b);
    const q = (p) => vals[Math.min(vals.length - 1, Math.floor(p * vals.length))] || 0;
    const cuts = [q(0.25), q(0.5), q(0.75)];
    const level = (v) => (v <= 0 ? 0 : v <= cuts[0] ? 1 : v <= cuts[1] ? 2 : v <= cuts[2] ? 3 : 4);
    const offset = new Date(C.isoDay(days[0].day)).getDay();
    const cols = Math.ceil((offset + days.length) / 7);
    const today = isoToday();
    const svg = el('svg', { width: cols * 11 - 2, height: 7 * 11 - 2, role: 'img', 'aria-label': `Tokens per day for the last 26 weeks, ${C.tokens(vals.reduce((a, b) => a + b, 0))} in total` }, box);
    days.forEach((d, i) => {
      const k = offset + i;
      const r = el('rect', { class: 'cell', x: Math.floor(k / 7) * 11, y: (k % 7) * 11, width: 9, height: 9, rx: 2, fill: `var(--heat${level(d.tokens)})` }, svg);
      if (d.day === today) { r.setAttribute('stroke', 'var(--fg1)'); r.setAttribute('stroke-width', '1.5'); }
      r.addEventListener('pointermove', (e) => C.tip.show(e.clientX, e.clientY, [{ head: true, label: `${C.weekday(C.isoDay(d.day))} ${C.dayMonth(C.isoDay(d.day))}` }, { value: C.tokens(d.tokens), label: `tokens · ${C.money(d.cost)}` }]));
      r.addEventListener('pointerleave', () => C.tip.hide());
    });
  }

  // ---------- filter chips (project / model) ----------
  const X = '<svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8"/></svg>';
  function filters() {
    const chip = (k, kind, v) => `<span class="chip"><span class="k">${k}</span><span class="n" title="${esc(v)}">${esc(v)}</span><button data-clear="${kind}" aria-label="Show all ${kind === 'project' ? 'projects' : 'models'}">${X}</button></span>`;
    $('#filters').innerHTML = (S.project ? chip('Project', 'project', S.project) : '') + (S.model ? chip('Model', 'model', S.model) : '');
  }
  $('#filters').addEventListener('click', (e) => {
    const b = e.target.closest('[data-clear]');
    if (!b) return;
    S[b.dataset.clear] = null;
    load();
  });
  function setTarget(t) {
    if (!t) return;
    // The dashboard has no Today: a day's worth reads best within the week.
    if (t.period) S.period = PERIODS.some(([v]) => v === t.period) ? t.period : '7d';
    S.project = t.project || null;
    S.model = t.model || null;
    controls();
    load().then(() => { if (S.project) $('#projects-p').scrollIntoView({ block: 'start' }); });
  }

  // ---------- projects and branches ----------
  const branchName = (b) => (b ? `<span class="mono">${esc(b)}</span>` : '<span>No branch</span>');
  function projects() {
    const ins = S.ins;
    const rows = ins?.projects || [];
    const one = S.project ? rows.find((p) => p.name === S.project) : null;
    $('#proj-back').hidden = !S.project;
    const usual = ins?.usualCacheHit;
    const cache = (c) => (c == null ? '<span class="dim">—</span>' : `<span class="${usual != null && c < usual / 2 && usual - c >= 20 ? 'low' : ''}">${Math.round(c)}%</span>`);
    if (S.project) {
      $('#proj-t').textContent = S.project;
      $('#proj-s').textContent = one ? `${one.branchesTotal} branch${one.branchesTotal === 1 ? '' : 'es'} · ${C.money(one.cost)} · cache ${one.cacheHit == null ? '—' : Math.round(one.cacheHit) + '%'}` : 'nothing in range';
      $('#proj-head').innerHTML = '<tr><th scope="col">Branch</th><th scope="col" class="r">Sessions</th><th scope="col" class="r">Tokens</th><th scope="col">Last active</th><th scope="col" class="r">Cost</th></tr>';
      const top = Math.max(0, ...(one?.branches || []).map((b) => b.cost));
      $('#projects').innerHTML = one?.branches?.length ? one.branches.map((b) => `<tr><td>${branchName(b.name)}</td><td class="r dim">${b.sessions}</td><td class="r">${esc(C.tokens(b.tokens))}</td>`
        + `<td class="dim">${esc(startedText(b.last))}</td><td class="r cost">${esc(C.money(b.cost))}<span class="bar-in" aria-hidden="true"><i style="width:${top ? Math.round((b.cost / top) * 100) : 0}%"></i></span></td></tr>`).join('')
        : '<tr><td colspan="5" class="dim" style="text-align:center;padding:24px">No activity for this project in this range.</td></tr>';
      return;
    }
    $('#proj-t').textContent = 'Projects';
    $('#proj-s').textContent = ins ? `${ins.projectsTotal} in range · by cost${ins.projectsTotal > rows.length ? `, top ${rows.length}` : ''}${S.model ? ' · all models' : ''}` : '';
    $('#proj-head').innerHTML = '<tr><th scope="col">Project</th><th scope="col">Top branches</th><th scope="col" class="r">Sessions</th><th scope="col" class="r">Cache</th><th scope="col" class="r">Tokens</th><th scope="col" class="r">Cost</th></tr>';
    $('#projects').innerHTML = rows.length ? rows.map((p) => `<tr class="go" tabindex="0" data-project="${esc(p.name)}" aria-label="${esc(p.name)}: ${esc(C.money(p.cost))}. Show its branches and sessions">`
      + `<td class="mono">${esc(p.name)}</td>`
      + `<td class="br">${p.branches.map((b) => `<span>${branchName(b.name)} ${esc(cash(b.cost))}</span>`).join('')}${p.branchesTotal > p.branches.length ? `<span>+${p.branchesTotal - p.branches.length} more</span>` : ''}</td>`
      + `<td class="r dim">${p.sessions}</td><td class="r">${cache(p.cacheHit)}</td><td class="r">${esc(C.tokens(p.tokens))}</td><td class="r cost">${esc(C.money(p.cost))}</td></tr>`).join('')
      : '<tr><td colspan="6" class="dim" style="text-align:center;padding:24px">No projects in this range.</td></tr>';
  }
  const openProject = (tr) => { if (tr?.dataset.project) { S.project = tr.dataset.project; load(); } };
  $('#projects').addEventListener('click', (e) => openProject(e.target.closest('tr.go')));
  $('#projects').addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openProject(e.target.closest('tr.go')); } });
  $('#proj-back').addEventListener('click', () => { S.project = null; load(); });

  // ---------- last week ----------
  function recap() {
    const r = S.ins?.recap;
    const box = $('#recap');
    if (!r) { $('#recap-d').textContent = ''; box.innerHTML = '<div class="empty" style="padding:8px 0">Nothing last week.</div>'; return; }
    $('#recap-d').textContent = `${C.dayMonth(C.isoDay(r.from))} – ${C.dayMonth(C.isoDay(r.to))}`;
    const delta = r.changePct == null ? 'nothing the week before' : Math.abs(r.changePct) < 3 ? 'about the same as the week before' : `${r.changePct > 0 ? '▲' : '▼'} ${Math.abs(Math.round(r.changePct))}% on the week before`;
    const fact = (k, v) => (v ? `<dt>${k}</dt><dd title="${esc(v)}">${esc(v)}</dd>` : '');
    const b = r.biggest;
    box.innerHTML = `<div class="big"><b>${esc(cash(r.spend))}</b><span class="delta">${esc(delta)}</span></div>`
      + `<dl class="facts">${fact('Tokens', `${C.tokens(r.tokens)} in ${r.sessions} session${r.sessions === 1 ? '' : 's'}`)}`
      + fact('Top project', r.topProject && `${r.topProject.name} · ${cash(r.topProject.cost)}`)
      + fact('Top model', r.topModel && `${r.topModel.name} · ${cash(r.topModel.cost)}`)
      + fact('Biggest session', b && `${C.money(b.cost)} · ${b.project} · ${b.harness}`) + '</dl>';
  }

  // ---------- subscription value ----------
  function value() {
    const rows = S.ins?.value || [];
    // The big number says the multiple, so the line drops its "(16×)".
    $('#value').innerHTML = rows.length ? rows.map((v) => `<div class="vr"><p>${esc(v.multiple != null ? v.line.replace(/ \([\d.]+×\)$/, '.') : v.line)}</p>`
      + (v.multiple != null ? `<span class="x" aria-hidden="true">${esc(v.multiple >= 10 ? Math.round(v.multiple) : v.multiple.toFixed(1))}×</span>` : '<span class="x dim" aria-hidden="true">—</span>') + '</div>').join('')
      : '<div class="empty" style="padding:8px 0">No subscription work this month yet. Claude Code, Codex and Gemini CLI count towards their plans.</div>';
  }

  // ---------- cache tips ----------
  function tips() {
    const list = S.ins?.tips || [];
    const usual = S.ins?.usualCacheHit;
    $('#tips-s').textContent = usual != null ? `usual cache hit ${Math.round(usual)}%` : '';
    $('#tips').innerHTML = list.length ? list.map((t) => `<div class="tip"><b>${esc(t.title)}</b><p>${esc(t.detail)}</p></div>`).join('')
      : usual != null ? `<div class="tip good"><b>Your prompt cache is working well</b><p>${Math.round(usual)}% of prompt tokens were read from the cache in this range, and no project with real volume falls far behind.</p></div>`
        : '<div class="empty" style="padding:8px 0">Nothing cached in this range.</div>';
  }

  // ---------- live ----------
  let rz;
  new ResizeObserver(() => { clearTimeout(rz); rz = setTimeout(() => { if (S.data) stacked(); }, 60); }).observe($('#chart'));
  listen('tr://stats-changed', () => load());
  listen('tr://state', ({ payload }) => { S.app = payload; C.setOs(payload.os); controls(); if (S.data) { resets(); kpis(); } });

  controls();
  try { S.app = await invoke('get_state'); C.setOs(S.app.os); controls(); } catch { /* accounts menu stays empty */ }
  if (C.q.get('period')) S.period = C.q.get('period');
  if (C.q.get('group')) S.group = C.q.get('group');
  listen('tr://dash-target', ({ payload }) => setTarget(payload));
  const target = await invoke('take_dashboard_target').catch(() => null);
  if (target) { setTarget(target); return; }
  controls();
  load();
})();
