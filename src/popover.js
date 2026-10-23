// The popover: Now / Stats / Harness tabs, the update view, and the footer with the iokig.com
// credit, which every view keeps (SPEC.md §6.2, §6.5).
(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const $ = (s) => document.querySelector(s);
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  let info = { version: '', update: null, updateHow: 'download' };
  let view = 'now';

  const EMPTY = {
    now: ['No accounts yet', 'Claude, ChatGPT/Codex, Bedrock and more are coming in the next releases: every allowance window, when it resets and whether you\'ll run out first.'],
    stats: ['Stats are on the way', 'Spend per day, week, month and lifetime, your most-used models, and what the work was: building, discovery, bug fixing.'],
    harness: ['Harnesses are on the way', 'The same stats for Claude Code, Codex, opencode and the rest, side by side.'],
  };

  // Release notes are short Markdown: paragraphs, "- " lists, **bold** and `code`.
  function md(text) {
    const inline = (s) => esc(s).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/`(.+?)`/g, '<code>$1</code>');
    let html = '', list = false;
    for (const line of text.split('\n')) {
      const item = line.match(/^\s*[-*] (.*)/);
      if (item && !list) { html += '<ul>'; list = true; }
      if (!item && list) { html += '</ul>'; list = false; }
      if (item) html += `<li>${inline(item[1])}</li>`;
      else if (/^#+ /.test(line)) html += `<p><b>${inline(line.replace(/^#+ /, ''))}</b></p>`;
      else if (line.trim()) html += `<p>${inline(line)}</p>`;
    }
    return html + (list ? '</ul>' : '');
  }

  async function renderUpdate() {
    const main = $('#view');
    main.innerHTML = '<p class="hint">Checking for updates…</p>';
    let checked;
    try { checked = await invoke('check_for_updates'); } catch (e) { main.innerHTML = `<div class="empty"><h2>Couldn't check</h2>${esc(e)}</div>`; return; }
    if (!checked.newer) { main.innerHTML = `<div class="empty"><h2>You're up to date</h2>Tokenrunway v${esc(checked.current)} is the latest version.</div>`; return; }
    let notes = [];
    try { notes = await invoke('update_notes'); } catch {}
    const action = info.updateHow === 'install' ? '<button class="primary" id="install">Update and restart</button>'
      : info.updateHow === 'homebrew' ? '<p class="hint">Update with <code>brew upgrade --cask tokenrunway</code></p>'
      : '<button class="primary" id="download">Download</button>';
    main.innerHTML = `<div class="card"><h3>v${esc(checked.latest)} is available</h3><p class="hint">You have v${esc(checked.current)}.</p>${action}<p class="hint" id="progress"></p></div>`
      + notes.map((n) => `<div class="card notes"><h3>v${esc(n.version)} <span class="hint">${esc(n.date)}</span></h3>${md(n.notes) || '<p class="hint">No notes.</p>'}</div>`).join('');
    $('#install')?.addEventListener('click', async (e) => {
      e.target.disabled = true;
      try { await invoke('install_update'); } catch (err) { $('#progress').textContent = err; e.target.disabled = false; }
    });
    $('#download')?.addEventListener('click', () => invoke('open_update', { url: null }));
  }

  function render() {
    document.querySelectorAll('#tabs button').forEach((b) => b.classList.toggle('on', b.dataset.view === view));
    if (view === 'update') renderUpdate();
    else {
      const [h, p] = EMPTY[view] || EMPTY.now;
      $('#view').innerHTML = `<div class="empty"><h2>${esc(h)}</h2>${esc(p)}</div>`;
    }
    $('#footer').innerHTML = `<span>Tokenrunway v${esc(info.version)}</span>·<a data-act="site">iokig.com</a>`
      + (info.update ? `<a class="update" data-act="update">Update available: v${esc(info.update)} →</a>` : '');
  }

  async function refresh() { info = await invoke('get_info'); render(); }

  document.addEventListener('click', (e) => {
    const t = e.target.closest('[data-view],[data-act]');
    if (!t) return;
    if (t.dataset.view) { view = t.dataset.view; render(); }
    if (t.dataset.act === 'site') invoke('open_site');
    if (t.dataset.act === 'update') { view = 'update'; render(); }
  });
  listen('tr://info', refresh);
  listen('tr://update-progress', ({ payload: p }) => {
    const el = $('#progress');
    if (el) el.textContent = p.total ? `Downloading… ${Math.round((p.got / p.total) * 100)}%` : 'Downloading…';
  });
  window.TR = { show: (v) => { view = v; refresh(); } };
  refresh();
})();
