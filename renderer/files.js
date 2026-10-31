// The file browser beside the terminal (files.js in the main process): the active session's folder
// as a tree, opened a folder at a time, with the files Git ignores dimmed; finding files by name or
// by content; and a file's whole content beside the tree. It follows the active session's folder,
// like the Git panel, and shares the right side with it.

const filesEl = $('#files');
const files = { open: false, root: null, expanded: new Set(), mode: 'name', selected: null, query: '' };

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

function openFiles(folder) {
  if (!folder) return;
  if (git.open) closeGit(); // they share the right side
  const switching = norm(folder) !== norm(files.root);
  files.root = folder;
  if (switching) {
    files.expanded = new Set();
    files.selected = null;
    $('#files-view').replaceChildren(el('div', { class: 'files-empty' }, 'Click a file to see its content.'));
  }
  if (!files.open) {
    files.open = true;
    if (!localStorage.getItem('filesWidth')) filesEl.style.width = `${Math.max(520, Math.round((innerWidth - $('#sidebar').getBoundingClientRect().right) * 0.5))}px`;
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

function closeFiles() {
  if (!files.open) return;
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

async function showFile(rel, line) {
  files.selected = rel;
  for (const r of document.querySelectorAll('#files-tree .files-row')) r.classList.toggle('selected', r.title === rel);
  const view = $('#files-view');
  const head = el('div', { class: 'files-view-head' },
    el('span', { class: 'files-view-path', title: rel }, rel),
    el('button', { class: 'icon-btn small', title: 'Open in its default app', html: '<svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/></svg>', onclick: () => api.openFile(joinPath(files.root, rel)) })); // joinPath: git.js
  view.replaceChildren(head, el('div', { class: 'files-empty' }, 'Loading…'));
  const r = await api.readFile(files.root, rel);
  if (files.selected !== rel) return;
  let body;
  if (r.error) body = el('div', { class: 'files-empty' }, r.error);
  else if (r.image) body = el('div', { class: 'files-image' }, el('img', { src: r.image, alt: rel }));
  else {
    const lines = r.text.replace(/\r\n/g, '\n').split('\n');
    if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop();
    const gutter = el('div', { class: 'files-gutter' }, lines.map((_, i) => i + 1).join('\n'));
    const code = el('pre', { class: 'files-code hljs' });
    const html = highlightFile(rel, lines.join('\n'));
    if (html != null) code.innerHTML = html;
    else code.textContent = lines.join('\n');
    body = el('div', { class: 'files-text' }, gutter, code);
    if (line) {
      requestAnimationFrame(() => {
        const lh = parseFloat(getComputedStyle(code).lineHeight) || 18;
        body.scrollTop = Math.max(0, (line - 4) * lh);
        const mark = el('div', { class: 'files-mark', style: `top: ${(line - 1) * lh + 8}px; height: ${lh}px` });
        body.append(mark);
      });
    }
  }
  view.replaceChildren(head, body);
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

// The panel's left edge, and the line between the tree and the content: drag to resize.
(() => {
  const resizer = $('#files-resizer');
  const saved = parseInt(localStorage.getItem('filesWidth') || '', 10);
  if (saved) filesEl.style.width = `${saved}px`;
  resizer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    resizer.classList.add('dragging');
    document.body.classList.add('resizing', 'resizing-x');
    const right = filesEl.getBoundingClientRect().right;
    const move = (ev) => {
      const max = right - $('#sidebar').getBoundingClientRect().right - 360; // keep the terminal usable
      filesEl.style.width = `${Math.max(380, Math.min(max, right - ev.clientX))}px`;
    };
    const up = () => {
      resizer.classList.remove('dragging');
      document.body.classList.remove('resizing', 'resizing-x');
      localStorage.setItem('filesWidth', parseInt(filesEl.style.width, 10));
      removeEventListener('mousemove', move);
      removeEventListener('mouseup', up);
      fitTerminals();
    };
    addEventListener('mousemove', move);
    addEventListener('mouseup', up);
  });
})();
dragSplit($('#files-split'), $('#files-side'), 'x', 'filesTreeWidth', 180, () => filesEl.offsetWidth - 200); // git.js
