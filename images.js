// The image behind an "[Image #N]" in a session's terminal (renderer: imageLinkProvider). Claude
// Code and Codex show a pasted image only as that placeholder; the image itself is in the
// session's transcript. Returns a file to show: the original when it still exists, else the
// image written out of the transcript into a cache folder.
//   Claude: numbers images across the session; a message's imagePasteIds name its image blocks
//     in order.
//   Codex: numbers them per message, as <image name=[Image #N] path="…"> followed by the image.
//     When several messages have an #N, the one whose text best matches the terminal line wins.

const { ipcMain } = require('electron');
const fs = require('fs');
const path = require('path');

let deps = {}; // { sessionFile(id) -> { file, agent } | null, cacheDir }
const indexCache = new Map(); // file -> { mtimeMs, size, entries }

const textOf = (content) => (Array.isArray(content) ? content : [{ type: 'text', text: String(content || '') }])
  .filter((b) => (b.type === 'text' || b.type === 'input_text') && b.text && !b.text.startsWith('<image') && !b.text.startsWith('</image') && !/^\s*# Files mentioned/.test(b.text))
  .map((b) => b.text).join(' ');

// Every pasted image in a transcript: { n, text, line, block, path }, oldest first.
function indexOf(file) {
  const st = fs.statSync(file);
  const cached = indexCache.get(file);
  if (cached && cached.mtimeMs === st.mtimeMs && cached.size === st.size) return cached.entries;
  const entries = [];
  const lines = fs.readFileSync(file, 'utf8').split('\n');
  lines.forEach((l, line) => {
    if (!l.includes('imagePasteIds') && !(l.includes('"input_image"') && l.includes('response_item'))) return;
    let o;
    try { o = JSON.parse(l); } catch { return; }
    if (Array.isArray(o.imagePasteIds) && Array.isArray(o.message?.content)) { // Claude
      const text = textOf(o.message.content);
      let k = 0;
      o.message.content.forEach((b, block) => {
        if (b.type !== 'image') return;
        const n = o.imagePasteIds[k++];
        if (n != null) entries.push({ n: +n, text, line, block, path: null });
      });
      return;
    }
    const c = o.payload?.content;
    if (o.type !== 'response_item' || o.payload?.role !== 'user' || !Array.isArray(c)) return; // Codex
    const text = textOf(c);
    let pending = null;
    c.forEach((b, block) => {
      const m = b.type === 'input_text' && /^<image name=\[Image #(\d+)\](?: path="((?:[^"\\]|\\.)*)")?/.exec(b.text || '');
      if (m) { pending = { n: +m[1], path: m[2] ? m[2].replace(/\\\\/g, '\\') : null }; return; }
      if (b.type === 'input_image' && pending) { entries.push({ ...pending, text, line, block }); pending = null; }
    });
  });
  indexCache.set(file, { mtimeMs: st.mtimeMs, size: st.size, entries });
  return entries;
}

// How many words of the terminal line the message's text shares.
function overlap(text, context) {
  const words = new Set(String(text).toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 1));
  return String(context || '').toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 1 && words.has(w)).length;
}

const EXT = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp' };

function writeOut(file, e) {
  const l = fs.readFileSync(file, 'utf8').split('\n')[e.line];
  const o = JSON.parse(l);
  const b = (o.message?.content || o.payload?.content)[e.block];
  let mime;
  let data;
  if (b.source?.data) { mime = b.source.media_type || 'image/png'; data = b.source.data; } // Claude
  else {
    const m = /^data:([^;]+);base64,(.*)$/s.exec(b.image_url || ''); // Codex
    if (!m) return null;
    [, mime, data] = m;
  }
  fs.mkdirSync(deps.cacheDir, { recursive: true });
  const out = path.join(deps.cacheDir, `${path.basename(file, '.jsonl')}-${e.line}-${e.block}.${EXT[mime] || 'png'}`);
  if (!fs.existsSync(out)) fs.writeFileSync(out, Buffer.from(data, 'base64'));
  return out;
}

function find(sessionId, n, context) {
  const s = deps.sessionFile(sessionId);
  if (!s?.file || !fs.existsSync(s.file)) return null;
  const all = indexOf(s.file).filter((e) => e.n === n);
  if (!all.length) return null;
  // The best match for the line's text; among equals, the latest.
  let best = all[all.length - 1];
  let bestScore = all.length > 1 ? overlap(best.text, context) : 0;
  for (let i = all.length - 2; i >= 0 && all.length > 1; i--) {
    const sc = overlap(all[i].text, context);
    if (sc > bestScore) { best = all[i]; bestScore = sc; }
  }
  if (best.path && fs.existsSync(best.path)) return best.path;
  return writeOut(s.file, best);
}

function init(d) {
  deps = d;
  ipcMain.handle('images:find', (_e, sessionId, n, context) => {
    try { return find(sessionId, +n, context); } catch { return null; }
  });
}

module.exports = { init, find };
