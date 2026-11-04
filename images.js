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
// Transcripts can be hundreds of MB and only grow, so each file is scanned once, a chunk at a time
// so terminals keep running, and later only what was added: file -> { scanned, entries, running }.
const indexes = new Map();
const files = new Map(); // session id -> its transcript, once known

const CHUNK = 1024 * 1024;
const MAX_TEXT = 2000; // of a message's text, kept for matching it to a terminal line
const pause = () => new Promise(setImmediate);

const textOf = (content) => (Array.isArray(content) ? content : [{ type: 'text', text: String(content || '') }])
  .filter((b) => (b.type === 'text' || b.type === 'input_text') && b.text && !b.text.startsWith('<image') && !b.text.startsWith('</image') && !/^\s*# Files mentioned/.test(b.text))
  .map((b) => b.text).join(' ').slice(0, MAX_TEXT);

const EXT = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp' };
const extOf = (mime) => EXT[mime] || 'png';

// Adds the pasted images in one transcript line: { n, text, start, len, block, path, ext }.
// Returns whether the line had to be parsed.
function imagesInLine(line, start, entries) {
  if (!line.includes('imagePasteIds') && !(line.includes('"input_image"') && line.includes('response_item'))) return false;
  let o;
  try { o = JSON.parse(line.toString('utf8')); } catch { return false; }
  const len = line.length;
  if (Array.isArray(o.imagePasteIds) && Array.isArray(o.message?.content)) { // Claude
    const text = textOf(o.message.content);
    let k = 0;
    o.message.content.forEach((b, block) => {
      if (b.type !== 'image') return;
      const n = o.imagePasteIds[k++];
      if (n != null) entries.push({ n: +n, text, start, len, block, path: null, ext: extOf(b.source?.media_type) });
    });
    return true;
  }
  const c = o.payload?.content;
  if (o.type !== 'response_item' || o.payload?.role !== 'user' || !Array.isArray(c)) return true; // Codex
  const text = textOf(c);
  let pending = null;
  c.forEach((b, block) => {
    const m = b.type === 'input_text' && /^<image name=\[Image #(\d+)\](?: path="((?:[^"\\]|\\.)*)")?/.exec(b.text || '');
    if (m) { pending = { n: +m[1], path: m[2] ? m[2].replace(/\\\\/g, '\\') : null }; return; }
    if (b.type === 'input_image' && pending) {
      entries.push({ ...pending, text, start, len, block, ext: extOf(/^data:([^;]+);/.exec(b.image_url || '')?.[1]) });
      pending = null;
    }
  });
  return true;
}

async function scan(file, ix, size) {
  let lineStart = ix.scanned;
  let parts = [];
  for await (const chunk of fs.createReadStream(file, { start: ix.scanned, end: size - 1, highWaterMark: CHUNK })) {
    let from = 0;
    for (let nl = chunk.indexOf(10, from); nl >= 0; nl = chunk.indexOf(10, from)) {
      parts.push(chunk.subarray(from, nl));
      const line = parts.length === 1 ? parts[0] : Buffer.concat(parts);
      parts = [];
      if (imagesInLine(line, lineStart, ix.entries)) await pause(); // parsing a big line takes a while
      lineStart += line.length + 1;
      from = nl + 1;
    }
    if (from < chunk.length) parts.push(chunk.subarray(from));
    ix.scanned = lineStart; // a line still being written is read again next time
  }
}

async function indexOf(file) {
  let ix = indexes.get(file);
  if (!ix) { ix = { scanned: 0, entries: [], running: null }; indexes.set(file, ix); }
  if (ix.running) await ix.running;
  const { size } = await fs.promises.stat(file);
  if (size < ix.scanned) { ix.scanned = 0; ix.entries = []; } // rewritten
  if (size > ix.scanned) {
    ix.running = scan(file, ix, size).finally(() => { ix.running = null; });
    await ix.running;
  }
  return ix.entries;
}

// How many words of the terminal line the message's text shares.
function overlap(text, context) {
  const words = new Set(String(text).toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 1));
  return String(context || '').toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 1 && words.has(w)).length;
}

// Writes the image out of the transcript, reading only its line; once per image.
async function writeOut(file, e) {
  const out = path.join(deps.cacheDir, `${path.basename(file, '.jsonl')}-${e.start}-${e.block}.${e.ext}`);
  if (fs.existsSync(out)) return out;
  const fh = await fs.promises.open(file, 'r');
  const line = Buffer.alloc(e.len);
  try { await fh.read(line, 0, e.len, e.start); } finally { await fh.close(); }
  const o = JSON.parse(line.toString('utf8'));
  const b = (o.message?.content || o.payload?.content)[e.block];
  const data = b.source?.data || /^data:[^;]+;base64,(.*)$/s.exec(b.image_url || '')?.[1]; // Claude, Codex
  if (!data) return null;
  await fs.promises.mkdir(deps.cacheDir, { recursive: true });
  await fs.promises.writeFile(out, Buffer.from(data, 'base64'));
  return out;
}

async function find(sessionId, n, context) {
  let file = files.get(sessionId);
  if (!file) {
    file = deps.sessionFile(sessionId)?.file;
    if (!file) return null;
    files.set(sessionId, file);
  }
  if (!fs.existsSync(file)) return null;
  const all = (await indexOf(file)).filter((e) => e.n === n);
  if (!all.length) return null;
  // The best match for the line's text; among equals, the latest.
  let best = all[all.length - 1];
  let bestScore = all.length > 1 ? overlap(best.text, context) : 0;
  for (let i = all.length - 2; i >= 0 && all.length > 1; i--) {
    const sc = overlap(all[i].text, context);
    if (sc > bestScore) { best = all[i]; bestScore = sc; }
  }
  if (best.path && fs.existsSync(best.path)) return best.path;
  return writeOut(file, best);
}

function init(d) {
  deps = d;
  ipcMain.handle('images:find', async (_e, sessionId, n, context) => {
    try { return await find(sessionId, +n, context); } catch { return null; }
  });
}

module.exports = { init, find };
