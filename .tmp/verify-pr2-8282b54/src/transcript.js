// A session's conversation for phones (remote.js), read from the transcript file its agent writes:
// Claude Code's projects/<dir>/<id>.jsonl, Codex's sessions/…/rollout-*-<id>.jsonl. Each line
// becomes zero or more chat items: what you said, what the agent said, and the tools it ran.
//
// Item: { id, role, text, name?, ref?, error?, images?, ts }, role being one of
//   user, assistant  what was said
//   tool             a tool call (name, one-line summary); ref ties it to its result
//   result           a tool's output (the start of it), for the call with the same ref
//   thinking         a reasoning summary, when the agent writes one
//   command, info    a slash command you ran, and notes: its output, an interruption, a compaction

const fs = require('fs');

const MAX_TEXT = 20000; // a very long message is cut for the phone
const cut = (s) => (s.length > MAX_TEXT ? `${s.slice(0, MAX_TEXT)}…` : s);
const MAX_RESULT = 1500; // the start of a tool's output is enough on a phone
const preview = (s) => { const t = String(s || '').replace(/\x1b\[[0-9;]*m/g, '').trim(); return t.length > MAX_RESULT ? `${t.slice(0, MAX_RESULT)}…` : t; };

// A tool result's content: a string, or parts with text.
function resultText(c) {
  if (typeof c === 'string') return c;
  if (Array.isArray(c)) return c.map((x) => (typeof x === 'string' ? x : x?.text || (x?.type === 'image' ? '[image]' : ''))).filter(Boolean).join('\n');
  return '';
}

// What a tool call did, in one line: the command it ran, the file it touched, what it searched.
function toolSummary(name, input) {
  let o = input;
  if (typeof o === 'string') { try { o = JSON.parse(o); } catch { return o.split('\n')[0].slice(0, 200); } }
  if (!o || typeof o !== 'object') return '';
  const cmd = Array.isArray(o.command) ? o.command.join(' ') : o.command || o.cmd;
  const v = cmd || o.description || o.file_path || o.path || o.pattern || o.url || o.query || o.prompt || o.skill || o.subject || '';
  return String(v).split('\n')[0].slice(0, 200);
}

// Claude Code's own wrappers around slash commands and their output.
function claudeUserText(text) {
  if (/^<local-command-caveat>/.test(text) || /^Caveat:/.test(text)) return null;
  const cmd = text.match(/<command-name>([^<]*)<\/command-name>/);
  if (cmd) {
    const args = text.match(/<command-args>([^<]*)<\/command-args>/);
    return { role: 'command', text: `${cmd[1].trim()}${args && args[1].trim() ? ` ${args[1].trim()}` : ''}` };
  }
  const out = text.match(/<local-command-stdout>([\s\S]*?)<\/local-command-stdout>/);
  if (out) return out[1].trim() ? { role: 'info', text: out[1].replace(/\x1b\[[0-9;]*m/g, '').trim() } : null;
  if (/^\[Request interrupted/.test(text.trim())) return { role: 'info', text: text.trim() };
  const bash = text.match(/<bash-input>([\s\S]*?)<\/bash-input>/);
  if (bash) return { role: 'command', text: `! ${bash[1].trim()}` };
  const bashOut = text.match(/<bash-stdout>([\s\S]*?)<\/bash-stdout>/);
  if (bashOut) return bashOut[1].trim() ? { role: 'info', text: preview(bashOut[1]) } : null;
  if (/^<(bash-stderr|system-reminder|task-notification)>/.test(text.trim())) return null;
  const clean = text.replace(/<system-reminder>[\s\S]*?<\/system-reminder>/g, '').trim();
  return clean ? { role: 'user', text: clean } : null;
}

function fromClaude(o, n) {
  const ts = Date.parse(o.timestamp) || 0;
  // Claude Code's own notes: "Conversation compacted", errors it shows in the session.
  if (o.type === 'system' && !o.isSidechain && typeof o.content === 'string' && o.content.trim() && o.level !== 'debug') {
    return [{ id: `${n}`, ts, role: 'info', text: preview(o.content.replace(/<[^>]+>/g, '')) }];
  }
  if (o.isSidechain || o.isMeta || !o.message) return [];
  const content = o.message.content;
  const out = [];
  if (o.type === 'user') {
    const parts = typeof content === 'string' ? [{ type: 'text', text: content }] : Array.isArray(content) ? content : [];
    const texts = [];
    let images = 0;
    parts.forEach((p, i) => {
      if (p.type === 'text' && p.text) texts.push(p.text);
      if (p.type === 'image') images++;
      if (p.type === 'tool_result') out.push({ id: `${n}.r${i}`, ts, role: 'result', ref: p.tool_use_id, text: preview(resultText(p.content)), ...(p.is_error ? { error: true } : {}) });
    });
    if (!texts.length && !images) return out;
    const item = claudeUserText(texts.join('\n'));
    if (item || images) out.push({ id: `${n}`, ts, ...(item || { role: 'user', text: '' }), ...(images ? { images } : {}) });
  } else if (o.type === 'assistant' && Array.isArray(content)) {
    content.forEach((p, i) => {
      if (p.type === 'text' && p.text?.trim()) out.push({ id: `${n}.${i}`, ts, role: 'assistant', text: cut(p.text.trim()) });
      if (p.type === 'thinking' && p.thinking?.trim()) out.push({ id: `${n}.${i}`, ts, role: 'thinking', text: cut(p.thinking.trim()) });
      if (p.type === 'tool_use') out.push({ id: `${n}.${i}`, ts, role: 'tool', name: p.name, ref: p.id, text: toolSummary(p.name, p.input) });
    });
  }
  return out;
}

function fromCodex(o, n) {
  if (o.type !== 'response_item' || !o.payload) return [];
  const p = o.payload;
  const ts = Date.parse(o.timestamp) || 0;
  if (p.type === 'message' && (p.role === 'user' || p.role === 'assistant') && Array.isArray(p.content)) {
    const texts = p.content.filter((c) => (c.type === 'input_text' || c.type === 'output_text') && c.text)
      .map((c) => c.text).filter((t) => !/^\s*(<|# AGENTS\.md instructions)/.test(t));
    const images = p.content.filter((c) => c.type === 'input_image').length;
    if (!texts.length && !images) return [];
    return [{ id: `${n}`, ts, role: p.role, text: cut(texts.join('\n').trim()), ...(images ? { images } : {}) }];
  }
  if (p.type === 'function_call' || p.type === 'custom_tool_call' || p.type === 'local_shell_call') {
    const name = p.name || (p.type === 'local_shell_call' ? 'shell' : 'tool');
    return [{ id: `${n}`, ts, role: 'tool', name, ref: p.call_id, text: toolSummary(name, p.arguments ?? p.input ?? p.action) }];
  }
  if (p.type === 'function_call_output' || p.type === 'custom_tool_call_output' || p.type === 'local_shell_call_output') {
    let out = p.output;
    if (typeof out === 'string') { try { const j = JSON.parse(out); out = j.output ?? j.content ?? out; } catch { /* plain text */ } }
    return [{ id: `${n}`, ts, role: 'result', ref: p.call_id, text: preview(resultText(out) || (typeof out === 'string' ? out : '')) }];
  }
  if (p.type === 'reasoning' && Array.isArray(p.summary)) {
    const text = p.summary.map((x) => x?.text || '').filter(Boolean).join('\n\n').trim();
    return text ? [{ id: `${n}`, ts, role: 'thinking', text: cut(text) }] : [];
  }
  return [];
}

// Lines -> items. `first` numbers the lines, so items keep their ids as the file grows.
function parse(lines, agent, first = 0) {
  const items = [];
  lines.forEach((l, i) => {
    if (!l.trim()) return;
    let o;
    try { o = JSON.parse(l); } catch { return; }
    items.push(...(agent === 'codex' ? fromCodex(o, first + i) : fromClaude(o, first + i)));
  });
  return items;
}

// Follows one session's file: read() returns the items so far, then poll() the new ones.
class Follower {
  constructor(file, agent, tailBytes = 4 * 1024 * 1024) {
    this.file = file;
    this.agent = agent;
    this.tailBytes = tailBytes;
    this.offset = 0;
    this.line = 0;
    this.rest = '';
  }

  // The end of the file, read further back until it holds enough messages: in a long session the
  // last few MB can be a handful of huge lines (screenshots, request records).
  read(enough = 200, maxBytes = 64 * 1024 * 1024) {
    const size = fs.statSync(this.file).size;
    let tail = this.tailBytes;
    for (;;) {
      const start = Math.max(0, size - tail);
      let text = this.slice(start, size);
      if (start > 0) text = text.slice(text.indexOf('\n') + 1); // a partial first line
      this.line = 0;
      this.rest = '';
      const items = this.take(text);
      if (items.length >= enough || start === 0 || tail >= maxBytes) {
        this.offset = size;
        return items;
      }
      tail *= 4;
    }
  }

  poll() {
    let size;
    try { size = fs.statSync(this.file).size; } catch { return []; }
    if (size < this.offset) { this.offset = 0; this.rest = ''; } // rewritten
    if (size === this.offset) return [];
    const text = this.slice(this.offset, size);
    this.offset = size;
    return this.take(text);
  }

  take(text) {
    const lines = (this.rest + text).split('\n');
    this.rest = lines.pop(); // not finished yet
    const items = parse(lines, this.agent, this.line);
    this.line += lines.length;
    return items;
  }

  slice(start, end) {
    const fd = fs.openSync(this.file, 'r');
    try {
      const buf = Buffer.alloc(end - start);
      const n = fs.readSync(fd, buf, 0, buf.length, start);
      return buf.subarray(0, n).toString('utf8');
    } finally { fs.closeSync(fd); }
  }
}

module.exports = { parse, Follower, toolSummary };
