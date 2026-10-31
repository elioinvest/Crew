// The file browser beside the terminal (renderer/files.js): a folder's tree, read one level at a
// time, the files Git ignores marked, finding files by name or by content, and a file's content.

const { ipcMain } = require('electron');
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const SKIP = new Set(['.git', 'node_modules']); // never walked when searching
const MAX_TEXT = 5 * 1024 * 1024;
const MAX_IMAGE = 15 * 1024 * 1024;
const IMAGE_TYPES = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', bmp: 'image/bmp', ico: 'image/x-icon', svg: 'image/svg+xml' };

const git = (cwd, args, input) => spawnSync('git', args, { cwd, input, encoding: 'utf8', windowsHide: true, maxBuffer: 32 * 1024 * 1024 });

// Of these paths (relative to root), the ones Git ignores. Nothing outside a repository.
function ignoredOf(root, rels) {
  if (!rels.length) return new Set();
  const r = git(root, ['check-ignore', '--stdin'], rels.join('\n'));
  return new Set((r.status === 0 ? r.stdout : '').split(/\r?\n/).filter(Boolean).map((p) => p.replace(/\\/g, '/')));
}

// One folder's entries: folders first, then files, by name.
ipcMain.handle('files:list', (_e, root, rel = '') => {
  const dir = path.join(root, rel);
  let entries;
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch (e) { return { error: e.message }; }
  const items = entries
    .filter((d) => d.name !== '.git')
    .map((d) => ({ name: d.name, rel: (rel ? `${rel}/` : '') + d.name, dir: d.isDirectory() }))
    .sort((a, b) => (b.dir - a.dir) || a.name.localeCompare(b.name, undefined, { sensitivity: 'base', numeric: true }));
  const ignored = ignoredOf(root, items.map((i) => (i.dir ? `${i.rel}/` : i.rel)));
  for (const i of items) i.ignored = ignored.has(i.dir ? `${i.rel}/` : i.rel) || ignored.has(i.rel);
  return { items };
});

// A file's content: text, an image, or why it isn't shown.
ipcMain.handle('files:read', (_e, root, rel) => {
  const f = path.join(root, rel);
  let st;
  try { st = fs.statSync(f); } catch (e) { return { error: e.message }; }
  const ext = path.extname(f).slice(1).toLowerCase();
  if (IMAGE_TYPES[ext]) {
    if (st.size > MAX_IMAGE) return { error: 'The image is too large to show.' };
    return { image: `data:${IMAGE_TYPES[ext]};base64,${fs.readFileSync(f).toString('base64')}`, size: st.size };
  }
  if (st.size > MAX_TEXT) return { error: `The file is ${(st.size / 1048576).toFixed(1)} MB, too large to show.`, size: st.size };
  const buf = fs.readFileSync(f);
  if (buf.subarray(0, 8000).includes(0)) return { error: 'This is a binary file.', size: st.size };
  return { text: buf.toString('utf8'), size: st.size };
});

// Files whose path contains the query (by name), or lines containing it (by content).
let hasRg = null;
ipcMain.handle('files:search', (_e, root, query, mode) => {
  const q = String(query || '').trim();
  if (!q) return { results: [] };
  if (hasRg === null) hasRg = spawnSync('rg', ['--version'], { windowsHide: true }).status === 0;
  if (mode === 'content') {
    if (hasRg) {
      const r = spawnSync('rg', ['-n', '--no-heading', '--color', 'never', '-F', '-S', '--max-count', '20', '--max-columns', '240', '--', q, '.'], { cwd: root, encoding: 'utf8', windowsHide: true, maxBuffer: 32 * 1024 * 1024 });
      const results = (r.stdout || '').split(/\r?\n/).filter(Boolean).slice(0, 500).map((l) => {
        const m = /^(.*?):(\d+):(.*)$/.exec(l);
        return m && { rel: m[1].replace(/^\.[\\/]/, '').replace(/\\/g, '/'), line: +m[2], text: m[3].trim() };
      }).filter(Boolean);
      return { results };
    }
    const results = [];
    const needle = q.toLowerCase();
    walk(root, '', (rel, full) => {
      let st;
      try { st = fs.statSync(full); } catch { return; }
      if (st.size > 2 * 1024 * 1024) return;
      const buf = fs.readFileSync(full);
      if (buf.subarray(0, 8000).includes(0)) return;
      buf.toString('utf8').split(/\r?\n/).forEach((t, i) => { if (results.length < 500 && t.toLowerCase().includes(needle)) results.push({ rel, line: i + 1, text: t.trim().slice(0, 240) }); });
    }, () => results.length >= 500);
    return { results };
  }
  // By name: the whole tree (as Git sees it when it's a repository, else a walk).
  let rels;
  const r = git(root, ['ls-files', '-co', '--exclude-standard']);
  if (r.status === 0) rels = r.stdout.split(/\r?\n/).filter(Boolean);
  else { rels = []; walk(root, '', (rel) => rels.push(rel), () => rels.length >= 50000); }
  const needle = q.toLowerCase();
  return { results: rels.filter((p) => p.toLowerCase().includes(needle)).slice(0, 500).map((rel) => ({ rel })) };
});

function walk(root, rel, onFile, done) {
  let entries;
  try { entries = fs.readdirSync(path.join(root, rel), { withFileTypes: true }); } catch { return; }
  for (const d of entries) {
    if (done()) return;
    const r = (rel ? `${rel}/` : '') + d.name;
    if (d.isDirectory()) { if (!SKIP.has(d.name)) walk(root, r, onFile, done); } else onFile(r, path.join(root, r));
  }
}
