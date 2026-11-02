// The file browser beside the terminal (files.js in the main process): the active session's folder
// as a tree, opened a folder at a time, with the files Git ignores dimmed, and finding files by
// name or by content. A file opens in a pane of its own in the main area, where it can be edited.
// The panel follows the active session's folder, like the Git panel, and shares the right side.

const filesEl = $('#files');
const files = { open: false, root: null, expanded: new Set(), mode: 'name', selected: null, query: '', edit: null, shown: null };

const fileIcon = '<svg viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>';
const folderIcon = '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
const chevIcon = '<svg viewBox="0 0 24 24"><path d="m9 6 6 6-6 6"/></svg>';

function activeFolder() {
  return state.terms.get(state.active)?.cwd || null;
}

// The Files button shows whenever the active session has a folder; an open panel follows it.
function updateFilesButton() {
  const cwd = activeFolder();
  $('#btn-files').classList.toggle('hidden', !cwd && !files.open);
  if (files.open && cwd && norm(cwd) !== norm(files.root)) openFiles(cwd);
}

async function openFiles(folder) {
  if (!folder) return;
  const switching = norm(folder) !== norm(files.root);
  if (switching && !(await leaveEdits())) return;
  if (git.open) closeGit(); // they share the right side
  files.root = folder;
  if (switching) {
    files.expanded = new Set();
    files.selected = null;
  }
  if (!files.open) {
    files.open = true;
    if (!localStorage.getItem('filesWidth2')) filesEl.style.width = '320px';
    filesEl.classList.remove('hidden');
    $('#files-resizer').classList.remove('hidden');
    document.body.classList.add('files-open');
    $('#btn-files').classList.add('on');
    fitTerminals(); // git.js
  }
  $('#files-title').textContent = folder.split(/[\\/]/).filter(Boolean).pop() || folder;
  $('#files-sub').textContent = folder;
  $('#files-sub').title = folder;
  renderFiles();
}

async function closeFiles() {
  if (!files.open) return;
  if (!(await leaveEdits())) return;
  files.open = false;
  filesEl.classList.add('hidden');
  $('#files-resizer').classList.add('hidden');
  document.body.classList.remove('files-open');
  $('#btn-files').classList.remove('on');
  updateFilesButton();
  fitTerminals();
}

// ---------- the tree ----------

async function renderFiles() {
  const tree = $('#files-tree');
  if (files.query) { renderSearch(); return; }
  const root = files.root;
  const rows = await folderRows(root, '', 0);
  if (root !== files.root || files.query) return; // moved on meanwhile
  tree.replaceChildren(...rows);
}

// A folder's rows, and those of its open subfolders, depth-first.
async function folderRows(root, rel, depth) {
  const r = await api.listFiles(root, rel);
  if (r.error) return depth ? [] : [el('div', { class: 'files-empty' }, r.error)];
  const rows = [];
  for (const it of r.items) {
    const open = it.dir && files.expanded.has(it.rel);
    const row = el('div', {
      class: `files-row${it.dir ? ' dir' : ''}${it.ignored ? ' ignored' : ''}${files.selected === it.rel ? ' selected' : ''}${open ? ' open' : ''}`,
      title: it.rel,
      style: `padding-left: ${8 + depth * 14}px`,
      onclick: () => (it.dir ? toggleFolder(it.rel) : showFile(it.rel)),
    },
    el('span', { class: 'files-chev', html: it.dir ? chevIcon : '' }),
    el('span', { class: 'files-icon', html: it.dir ? folderIcon : fileIcon }),
    el('span', { class: 'files-name' }, it.name));
    rows.push(row);
    if (open) rows.push(...await folderRows(root, it.rel, depth + 1));
  }
  if (!r.items.length && depth) rows.push(el('div', { class: 'files-row empty', style: `padding-left: ${8 + depth * 14 + 18}px` }, 'Empty'));
  return rows;
}

function toggleFolder(rel) {
  if (files.expanded.has(rel)) files.expanded.delete(rel); else files.expanded.add(rel);
  renderFiles();
}

// ---------- finding files ----------

let searchSeq = 0;
async function renderSearch() {
  const seq = ++searchSeq;
  const tree = $('#files-tree');
  const r = await api.searchFiles(files.root, files.query, files.mode);
  if (seq !== searchSeq) return;
  if (!r.results.length) { tree.replaceChildren(el('div', { class: 'files-empty' }, 'No matches')); return; }
  tree.replaceChildren(...r.results.map((m) => el('div', {
    class: `files-row hit${files.selected === m.rel ? ' selected' : ''}`,
    title: m.rel,
    onclick: () => showFile(m.rel, m.line),
  },
  el('span', { class: 'files-icon', html: fileIcon }),
  el('span', { class: 'files-hit' },
    el('span', { class: 'files-name' }, m.rel.split('/').pop()),
    el('span', { class: 'files-where' }, m.line ? `${m.rel}:${m.line}` : m.rel),
    m.text ? el('span', { class: 'files-line' }, m.text) : null))));
}

let searchTimer = null;
$('#files-query').addEventListener('input', (e) => {
  files.query = e.target.value.trim();
  clearTimeout(searchTimer);
  searchTimer = setTimeout(renderFiles, files.mode === 'content' ? 300 : 150);
});
for (const b of document.querySelectorAll('.files-mode')) {
  b.addEventListener('click', () => {
    files.mode = b.dataset.mode;
    for (const x of document.querySelectorAll('.files-mode')) x.classList.toggle('on', x === b);
    $('#files-query').placeholder = files.mode === 'content' ? 'Find in files' : 'Find files';
    $('#files-query').focus();
    renderFiles();
  });
}

// ---------- a file's content ----------

// ---------- the file pane ----------

// One pane in the main area shows the opened file, as a preview: opening another file replaces it
// (after asking about unsaved edits). It takes part in the pane layout like a session.
const FILE_PANE = 'file:view';
const filePaneTitle = () => (files.shown ? files.shown.rel.split('/').pop() : 'File');

function filePane() {
  let entry = state.terms.get(FILE_PANE);
  if (entry) return entry;
  const view = el('div', { class: 'file-pane-view' });
  const container = el('div', { class: 'term file-pane' }, view);
  termStash.append(container); // renderer.js
  entry = {
    agent: 'file', termId: null, el: container, view, projectPath: files.root, cwd: files.root, exited: false,
    term: { cols: 80, rows: 24, modes: { mouseTrackingMode: 'none' }, buffer: { active: { length: 0, baseY: 0, getLine: () => null } }, focus() {}, dispose() {}, reset() {}, write() {}, hasSelection: () => false },
    fit: { fit() {} }, resizeObserver: { disconnect() {} },
    draft: '', attachments: [], fileSettings: {}, live: {}, overrides: {},
    beforeClose: () => leaveEdits(),
  };
  state.terms.set(FILE_PANE, entry);
  return entry;
}

// Shows the file pane: where it is already, else in an empty pane, else split beside the focused one.
function showFilePane() {
  const entry = filePane();
  entry.projectPath = files.root;
  entry.cwd = files.root;
  entry.where = files.shown ? `${files.root}\\${files.shown.rel.replace(/\//g, '\\')}` : files.root;
  const shown = leafOf(FILE_PANE);
  if (shown) { focusPane(shown); activate(FILE_PANE); return; }
  const empty = leaves().find((l) => !l.session);
  if (empty) state.focus = empty.id;
  else splitPane('row', focusedLeaf());
  activate(FILE_PANE);
}

async function showFile(rel, line) {
  if (!(await leaveEdits())) return;
  files.selected = rel;
  files.shown = { rel };
  for (const r of document.querySelectorAll('#files-tree .files-row')) r.classList.toggle('selected', r.title === rel);
  const view = filePane().view;
  showFilePane();
  const saveBtn = el('button', { class: 'files-save hidden', title: 'Save (Ctrl+S)', onclick: () => saveEdits() }, 'Save');
  const head = el('div', { class: 'files-view-head' },
    el('span', { class: 'files-view-path', title: rel }, rel),
    el('span', { class: 'files-dirty hidden', title: 'Unsaved changes' }, '●'),
    saveBtn,
    el('button', { class: 'icon-btn small', title: 'Open in its default app', html: '<svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/></svg>', onclick: () => api.openFile(joinPath(files.root, rel)) })); // joinPath: git.js
  view.replaceChildren(head, el('div', { class: 'files-empty' }, 'Loading…'));
  const r = await api.readFile(files.root, rel);
  if (files.selected !== rel) return;
  let body;
  if (r.error) body = el('div', { class: 'files-empty' }, r.error);
  else if (r.image) body = el('div', { class: 'files-image' }, el('img', { src: r.image, alt: rel }));
  else {
    body = makeEditor(rel, r.text, head);
    if (line) {
      // A content match: scroll to its line, mark it, and put the caret there.
      requestAnimationFrame(() => {
        const input = body.querySelector('.files-input');
        const row = body.querySelector(`.fl[data-n="${line}"]`);
        if (row) {
          body.scrollTop = Math.max(0, row.offsetTop - 60);
          body.querySelector('.files-layer').append(el('div', { class: 'files-mark', style: `top: ${row.offsetTop}px; height: ${row.offsetHeight}px` }));
        }
        const at = input.value.split('\n').slice(0, line - 1).join('\n').length + (line > 1 ? 1 : 0);
        input.focus({ preventScroll: true });
        input.setSelectionRange(at, at);
      });
    }
  }
  view.replaceChildren(head, body);
  updatePaneHeads(); // renderer.js: the pane's title is the file's name
  $('#head-title').textContent = filePaneTitle();
  if (state.active === FILE_PANE) requestAnimationFrame(() => body.querySelector?.('.files-input')?.focus({ preventScroll: !!line }));
}

// ---------- editing ----------

// The editor: a transparent textarea over the highlighted code, both in the same font and size,
// so typing happens in the textarea and the colors below follow it. Long lines wrap to the pane's
// width in both; the code is drawn a line per block, its number beside it, so a wrapped line keeps
// one number. The file keeps its line endings (CRLF or LF) when saved.
function makeEditor(rel, original, head) {
  const eol = original.includes('\r\n') ? '\r\n' : '\n';
  const text = original.replace(/\r\n/g, '\n');
  const code = el('div', { class: 'files-code hljs' });
  const input = el('textarea', { class: 'files-input', spellcheck: 'false', autocomplete: 'off' });
  input.value = text;
  const layer = el('div', { class: 'files-layer' }, code, input);
  const body = el('div', { class: 'files-text' }, layer);
  files.edit = { rel, eol, saved: text, input, head };
  const paint = () => {
    const t = input.value;
    const html = highlightFile(rel, t) ?? escapeHtml(t);
    const lines = splitHtmlLines(html);
    // Room for the line numbers, by the widest one.
    layer.style.setProperty('--gutter', `calc(${String(lines.length).length}ch + 28px)`);
    code.innerHTML = lines.map((l, i) => `<div class="fl" data-n="${i + 1}">${l || '\u200b'}</div>`).join('');
  };
  let timer = null;
  input.addEventListener('input', () => {
    syncDirty();
    clearTimeout(timer);
    timer = setTimeout(paint, input.value.length > 200000 ? 250 : 30);
  });
  input.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); saveEdits(); return; }
    if (e.key === 'Tab') { e.preventDefault(); indent(input, e.shiftKey); }
  });
  paint();
  return body;
}

const escapeHtml = (t) => t.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

// Highlighted HTML split into lines; a span that crosses a line break (a long comment, a template
// string) is closed at the end of the line and opened again on the next.
function splitHtmlLines(html) {
  const lines = [];
  const open = [];
  let cur = '';
  for (const m of html.matchAll(/(<span[^>]*>)|(<\/span>)|(\n)|([^<\n]+)/g)) {
    if (m[1]) { cur += m[1]; open.push(m[1]); }
    else if (m[2]) { cur += m[2]; open.pop(); }
    else if (m[3]) { lines.push(cur + '</span>'.repeat(open.length)); cur = open.join(''); }
    else cur += m[4];
  }
  lines.push(cur);
  return lines;
}

// Tab indents (the selected lines, or inserts a tab); Shift+Tab takes one level off.
function indent(input, out) {
  const { selectionStart: a, selectionEnd: b, value: v } = input;
  const lineStart = v.lastIndexOf('\n', a - 1) + 1;
  if (!out && a === b) { input.setRangeText('\t', a, b, 'end'); input.dispatchEvent(new Event('input')); return; }
  const lineEnd = b > a && v[b - 1] === '\n' ? b - 1 : b;
  const block = v.slice(lineStart, lineEnd);
  const changed = block.split('\n').map((l) => (out ? l.replace(/^(\t| {1,4})/, '') : `\t${l}`)).join('\n');
  input.setRangeText(changed, lineStart, lineEnd, 'select');
  input.dispatchEvent(new Event('input'));
}

const isDirty = () => !!files.edit && files.edit.input.value !== files.edit.saved;
function syncDirty() {
  if (!files.edit) return;
  const dirty = isDirty();
  files.edit.head.querySelector('.files-dirty').classList.toggle('hidden', !dirty);
  files.edit.head.querySelector('.files-save').classList.toggle('hidden', !dirty);
}

async function saveEdits() {
  const e = files.edit;
  if (!e || !isDirty()) return true;
  const text = e.input.value;
  const r = await api.writeFile(files.root, e.rel, e.eol === '\r\n' ? text.replace(/\n/g, '\r\n') : text);
  if (r.error) { toast(`Couldn't save ${e.rel}: ${r.error}`, 'error'); return false; } // hosts.js
  e.saved = text;
  syncDirty();
  toast(`Saved ${e.rel}`);
  return true;
}

// Before showing another file, closing or switching folders: save, discard or stay.
async function leaveEdits() {
  if (!isDirty()) { files.edit = null; return true; }
  const answer = await new Promise((resolve) => {
    const box = $('#ssh-prompt'); // the shared dialog (hosts.js)
    const done = (v) => { box.classList.add('hidden'); resolve(v); };
    box.firstElementChild.replaceChildren(
      el('h2', {}, `Save changes to ${files.edit.rel.split('/').pop()}?`),
      el('p', { class: 'prompt-text' }, 'Your changes are lost if you don\'t save them.'),
      el('div', { class: 'modal-actions' },
        el('button', { onclick: () => done('cancel') }, 'Cancel'),
        el('button', { onclick: () => done('discard') }, 'Don\'t save'),
        el('button', { class: 'primary', onclick: () => done('save') }, 'Save')));
    box.classList.remove('hidden');
  });
  if (answer === 'cancel') return false;
  if (answer === 'save' && !(await saveEdits())) return false;
  files.edit = null;
  return true;
}

// Syntax highlighting by the file's type (highlight.js, bundled): its extension, or its name for
// files like Dockerfile and Makefile. Returns HTML, or null to show plain text (unknown types,
// and files too large to highlight quickly).
const HIGHLIGHT_NAMES = { dockerfile: 'dockerfile', makefile: 'makefile', '.gitignore': 'bash', '.env': 'bash', '.npmrc': 'ini', '.editorconfig': 'ini' };
const HIGHLIGHT_EXTS = { cjs: 'javascript', mjs: 'javascript', jsonc: 'json', ps1: 'powershell', psm1: 'powershell', cmd: 'dos', bat: 'dos', toml: 'ini', vue: 'xml', svelte: 'xml', svg: 'xml', htm: 'xml', html: 'xml', yml: 'yaml', md: 'markdown' };
function highlightFile(rel, text) {
  if (typeof hljs === 'undefined' || text.length > 600 * 1024) return null;
  const name = rel.split('/').pop().toLowerCase();
  const ext = name.includes('.') ? name.split('.').pop() : '';
  const lang = HIGHLIGHT_NAMES[name] || HIGHLIGHT_EXTS[ext] || (ext && hljs.getLanguage(ext) ? ext : null);
  if (!lang || !hljs.getLanguage(lang)) return null;
  try { return hljs.highlight(text, { language: lang, ignoreIllegals: true }).value; } catch { return null; }
}

// ---------- wiring ----------

$('#btn-files').addEventListener('click', () => (files.open ? closeFiles() : openFiles(activeFolder() || files.root)));
$('#files-close').addEventListener('click', closeFiles);
$('#files-refresh').addEventListener('click', () => renderFiles());
addEventListener('beforeunload', (e) => { if (isDirty()) { e.preventDefault(); e.returnValue = ''; } });

// The panel's left edge, and the line between the tree and the content: drag to resize.
(() => {
  const resizer = $('#files-resizer');
  const saved = parseInt(localStorage.getItem('filesWidth2') || '', 10); // the tree alone (the content moved to a pane)
  if (saved) filesEl.style.width = `${saved}px`;
  resizer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    resizer.classList.add('dragging');
    document.body.classList.add('resizing', 'resizing-x');
    const right = filesEl.getBoundingClientRect().right;
    const move = (ev) => {
      const max = right - $('#sidebar').getBoundingClientRect().right - 360; // keep the terminal usable
      filesEl.style.width = `${Math.max(220, Math.min(max, right - ev.clientX))}px`;
    };
    const up = () => {
      resizer.classList.remove('dragging');
      document.body.classList.remove('resizing', 'resizing-x');
      localStorage.setItem('filesWidth2', parseInt(filesEl.style.width, 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
      fitTerminals();
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
})();
