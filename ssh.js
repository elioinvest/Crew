// SSH hosts for the agents, on the model of Hatoba (github.com/scarletkc/Hatoba): Crew keeps the
// hosts and their credentials, and the agents in its sessions reach them only through the
// `crew-ssh` command, by name. Secrets stay in this process (encrypted at rest with the
// system's data protection); the window and the agents see display data only. Commands that
// only read run at once, anything else waits for the user to approve it in Crew, and host keys
// are trusted on first use and blocked when they change.

const { app, ipcMain, safeStorage, dialog, shell } = require('electron');
const fs = require('fs');
const path = require('path');
const os = require('os');
const net = require('net');
const crypto = require('crypto');
const { Client, utils: sshUtils } = require('ssh2');

const IDLE_MS = 10 * 60e3; // a host's connection closes after this long unused
const LOG_SIZE = 20; // recent agent commands kept per host
const ALLOW_FOR_MS = 15 * 60e3; // "Allow on this host for 15 minutes"

let getWin = () => null;
const dataDir = () => app.getPath('userData');
const file = (name) => path.join(dataDir(), name);

function readJson(p, fallback) {
  try { return JSON.parse(fs.readFileSync(p, 'utf8')); } catch { return fallback; }
}
function writeJson(p, v) {
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, JSON.stringify(v, null, 2));
}

// ---------- hosts and secrets ----------

// { id, name, host, port, user, auth: 'auto' | 'key' | 'password' | 'agent', keyPath, group,
//   notes (instructions for agents), approval: 'changes' | 'all' | 'auto', secret: sealed }
let hosts = null;
const allHosts = () => (hosts ??= readJson(file('hosts.json'), []));
const saveHosts = () => writeJson(file('hosts.json'), hosts);

const canSeal = () => safeStorage.isEncryptionAvailable();
const seal = (s) => (s && canSeal() ? safeStorage.encryptString(s).toString('base64') : null);
function unseal(b) {
  if (!b) return null;
  try { return safeStorage.decryptString(Buffer.from(b, 'base64')); } catch { return null; }
}

function publicHost(h) {
  const { secret, ...rest } = h;
  return { ...rest, workspace: workspacePath(h), hasPassword: !!secret?.password, hasPassphrase: !!secret?.passphrase, connected: pool.has(h.id), terminals: shells.get(h.id)?.size || 0 };
}

// ---------- each host's workspace ----------

// The folder a host's agent sessions run in (so they list under the host, not as a project). Its
// CLAUDE.md and AGENTS.md tell Claude Code and Codex which machine they're managing and how.
// The path is fixed when it's first needed, so renaming the host keeps its sessions.
function workspacePath(h) {
  if (!h.workspace) {
    const safe = (h.name || h.host || 'host').replace(/[^\w.-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40) || 'host';
    h.workspace = path.join(dataDir(), 'host-workspaces', `${safe}-${h.id.slice(0, 8)}`);
    saveHosts();
  }
  return h.workspace;
}

function writeWorkspace(h) {
  const dir = workspacePath(h);
  fs.mkdirSync(dir, { recursive: true });
  const addr = `${h.user ? `${h.user}@` : ''}${h.host}${h.port && h.port !== 22 ? `:${h.port}` : ''}`;
  const text = `# Remote host: ${h.name}

You're in Crew's workspace for the remote machine **${h.name}** (${addr}). The user's terminal on it is open beside this chat. Work on that machine, not in this folder.

- Run commands on it: \`crew-ssh exec ${h.name} -- <command>\` (add \`--cwd DIR\` to run in a folder there).
- See what's on the user's terminal there: \`crew-ssh screen ${h.name}\`, e.g. when they say "look at this error".
- Copy files: \`crew-ssh cp <local> ${h.name}:<remote>\` or \`crew-ssh cp ${h.name}:<remote> <local>\`.

Crew holds the credentials: never ask for passwords or keys, and don't use \`ssh\`, \`scp\` or \`sftp\` directly.

Commands that only read run at once. Anything that may change the machine waits until the user approves it in Crew: say in one line what you'll run and why, and give crew-ssh a long timeout (10 minutes). If the user denies something, don't run it again or work around it; ask.

crew-ssh commands have no terminal and no input: use non-interactive forms (\`--no-pager\`, \`-y\`, \`DEBIAN_FRONTEND=noninteractive\`) and keep output short (\`| tail -n 100\`). When a step needs typing (a sudo password, an editor, a confirmation), ask the user to do it in their terminal beside you, then check the result with \`crew-ssh screen ${h.name}\`.

## The user's notes about ${h.name}

${h.notes?.trim() || 'None yet.'}
`;
  for (const f of ['CLAUDE.md', 'AGENTS.md']) fs.writeFileSync(path.join(dir, f), text);
  return dir;
}

const label = (h) => `${h.name} (${h.user ? `${h.user}@` : ''}${h.host}${h.port && h.port !== 22 ? `:${h.port}` : ''})`;
const expandHome = (p) => (p && p.startsWith('~') ? path.join(os.homedir(), p.slice(1)) : p);

// A host by id or name (exact first, then case-insensitive). Ambiguous names are an error.
function findHost(ref) {
  const list = allHosts();
  const byId = list.find((h) => h.id === ref);
  if (byId) return { host: byId };
  const exact = list.filter((h) => h.name === ref);
  const loose = exact.length ? exact : list.filter((h) => h.name.toLowerCase() === String(ref).toLowerCase());
  if (loose.length === 1) return { host: loose[0] };
  if (loose.length > 1) return { error: `Several saved hosts are named "${ref}": ${loose.map((h) => `${h.name} [${h.id}]`).join(', ')}. Use the id.` };
  return { error: `No saved host named "${ref}". Saved hosts: ${list.map((h) => h.name).join(', ') || 'none'}. Add it in Crew (Hosts in the sidebar).` };
}

// Hosts from ~/.ssh/config: one per Host alias without wildcards.
function parseSshConfig(text) {
  const out = [];
  let cur = []; // the entries of the current Host line (it can name several aliases)
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(/#.*/, '').trim();
    if (!line) continue;
    const m = /^(\S+)\s*=?\s*(.+)$/.exec(line);
    if (!m) continue;
    const key = m[1].toLowerCase();
    const val = m[2].trim().replace(/^"(.*)"$/, '$1');
    if (key === 'host' || key === 'match') {
      cur = key === 'host' ? val.split(/\s+/).filter((a) => !/[*?!]/.test(a)).map((a) => ({ name: a, host: a })) : [];
      out.push(...cur);
    } else {
      for (const c of cur) {
        if (key === 'hostname') c.host = val;
        else if (key === 'user') c.user = val;
        else if (key === 'port') c.port = +val || 22;
        else if (key === 'identityfile' && !c.keyPath) c.keyPath = val;
      }
    }
  }
  return out;
}

// ---------- prompts in the window ----------

// Host-key trust, passwords, 2FA and approvals are asked in Crew while the agent's call waits.
const prompts = new Map();
let promptSeq = 0;

function ask(kind, data) {
  const w = getWin();
  if (!w || w.isDestroyed()) return { promise: Promise.resolve(null), cancel() {} };
  const id = ++promptSeq;
  const promise = new Promise((resolve) => prompts.set(id, resolve));
  w.webContents.send('ssh:prompt', { ...data, id, kind });
  if (!w.isVisible()) w.show();
  if (!w.isFocused()) w.flashFrame(true);
  return {
    promise,
    cancel() {
      if (!prompts.has(id)) return;
      prompts.get(id)(null);
      prompts.delete(id);
      if (!w.isDestroyed()) w.webContents.send('ssh:prompt-cancel', id);
    },
  };
}

ipcMain.on('ssh:answer', (_e, id, answer) => {
  const resolve = prompts.get(id);
  if (!resolve) return;
  prompts.delete(id);
  resolve(answer);
});

// ---------- host keys: trust on first use ----------

const knownFile = () => file('ssh-known-hosts.json');

async function verifyHostKey(h, key, cancelled) {
  const fingerprint = `SHA256:${crypto.createHash('sha256').update(key).digest('base64').replace(/=+$/, '')}`;
  let type = 'unknown';
  try { type = sshUtils.parseKey(key)?.type || type; } catch { /* keep "unknown" */ }
  const id = `${h.host}:${h.port || 22}`;
  const known = readJson(knownFile(), {});
  const entry = known[id];
  if (entry?.fingerprint === fingerprint) return true;
  if (cancelled()) return false;
  const changed = entry && entry.type === type;
  const p = ask(changed ? 'hostkey-changed' : 'hostkey', { host: label(h), address: id, type, fingerprint, previous: changed ? entry.fingerprint : null });
  const answer = await p.promise;
  if (answer !== 'trust') return false;
  known[id] = { type, fingerprint, at: Date.now() };
  writeJson(knownFile(), known);
  return true;
}

// ---------- connections ----------

const pool = new Map(); // host id -> { client, ready: Promise<Client>, timer }

function dropConn(id) {
  const c = pool.get(id);
  if (!c) return;
  pool.delete(id);
  clearTimeout(c.timer);
  try { c.client.end(); } catch { /* closed */ }
  notifyHosts();
}

function touch(id) {
  const c = pool.get(id);
  if (!c) return;
  clearTimeout(c.timer);
  if (c.busy > 0) return; // an open terminal keeps the connection
  c.timer = setTimeout(() => dropConn(id), IDLE_MS);
}

function hold(id, delta) {
  const c = pool.get(id);
  if (!c) return;
  c.busy = Math.max(0, (c.busy || 0) + delta);
  touch(id);
}

const DEFAULT_KEYS = ['id_ed25519', 'id_ecdsa', 'id_rsa'].map((k) => path.join(os.homedir(), '.ssh', k));
const agentSocket = () => (process.platform === 'win32' ? '\\\\.\\pipe\\openssh-ssh-agent' : process.env.SSH_AUTH_SOCK);

async function authConfig(h) {
  const cfg = {};
  const secret = h.secret || {};
  const keyFile = h.auth === 'key' || h.auth === 'auto' || !h.auth
    ? expandHome(h.keyPath) || (h.auth === 'key' ? null : DEFAULT_KEYS.find((k) => fs.existsSync(k)))
    : null;
  if (h.auth === 'key' && !keyFile) throw new Error(`No key file is set for ${h.name}.`);
  if (keyFile) {
    let key;
    try { key = fs.readFileSync(keyFile); } catch { if (h.auth === 'key') throw new Error(`Can't read the key file ${keyFile}.`); }
    if (key) {
      let passphrase = unseal(secret.passphrase);
      const parsed = sshUtils.parseKey(key, passphrase || undefined);
      if (parsed instanceof Error && /encrypted|passphrase/i.test(parsed.message)) {
        const a = await ask('passphrase', { host: label(h), keyFile }).promise;
        if (!a?.value) throw new Error('No passphrase was given for the key.');
        passphrase = a.value;
        if (a.remember) { h.secret = { ...h.secret, passphrase: seal(passphrase) }; saveHosts(); }
      }
      cfg.privateKey = key;
      if (passphrase) cfg.passphrase = passphrase;
    }
  }
  if ((h.auth === 'agent' || h.auth === 'auto' || !h.auth) && agentSocket() && (process.platform === 'win32' || fs.existsSync(agentSocket()))) {
    cfg.agent = agentSocket();
  }
  if (h.auth === 'password' || h.auth === 'auto' || !h.auth) {
    let password = unseal(secret.password);
    if (!password && h.auth === 'password') {
      const a = await ask('password', { host: label(h) }).promise;
      if (!a?.value) throw new Error('No password was given.');
      password = a.value;
      if (a.remember) { h.secret = { ...h.secret, password: seal(password) }; saveHosts(); }
    }
    if (password) cfg.password = password;
  }
  return cfg;
}

function connect(h) {
  const existing = pool.get(h.id);
  if (existing) { touch(h.id); return existing.ready; }
  const client = new Client();
  let cancelledFlag = false;
  const entry = { client, timer: null };
  entry.ready = (async () => {
    const auth = await authConfig(h);
    return new Promise((resolve, reject) => {
      client.once('ready', () => { touch(h.id); notifyHosts(); resolve(client); });
      client.once('error', (e) => {
        pool.delete(h.id);
        reject(new Error(/authentication methods failed/i.test(e.message) ? `Couldn't sign in to ${h.name}: check the user, key or password in Crew.`
          : /verification failed/i.test(e.message) ? `Crew didn't connect to ${h.name}: its host key wasn't trusted (or has changed since it was). The user has to confirm it in Crew.`
            : e.level === 'client-timeout' ? `Timed out signing in to ${h.host}.` : e.message));
      });
      client.on('close', () => { if (pool.get(h.id) === entry) { pool.delete(h.id); notifyHosts(); } });
      client.on('keyboard-interactive', (_name, instructions, _lang, list, finish) => {
        if (!list.length) { finish([]); return; }
        ask('keyboard', { host: label(h), instructions, prompts: list.map((p) => ({ prompt: p.prompt, echo: p.echo })) })
          .promise.then((answers) => finish(Array.isArray(answers) ? answers : []));
      });
      // Only reaching the host times out quickly: the sign-in after it can wait on the user
      // (host-key trust, a password, 2FA), so it gets ten minutes.
      const sock = net.connect({ host: h.host, port: h.port || 22 });
      sock.setTimeout(20000, () => { sock.destroy(); pool.delete(h.id); reject(new Error(`Timed out connecting to ${h.host}:${h.port || 22}.`)); });
      sock.once('error', (e) => { pool.delete(h.id); reject(new Error(`Can't reach ${h.host}:${h.port || 22}: ${e.code || e.message}.`)); });
      sock.once('connect', () => {
        sock.setTimeout(0);
        client.connect({
          sock,
          username: h.user || os.userInfo().username,
          readyTimeout: 10 * 60e3,
          keepaliveInterval: 15000,
          tryKeyboard: true,
          hostVerifier: (key, verify) => { verifyHostKey(h, key, () => cancelledFlag).then(verify, () => verify(false)); },
          ...auth,
        });
      });
    });
  })();
  entry.ready.catch(() => { cancelledFlag = true; if (pool.get(h.id) === entry) pool.delete(h.id); });
  pool.set(h.id, entry);
  return entry.ready;
}

const shq = (s) => `'${String(s).replace(/'/g, "'\\''")}'`;

// Runs a command on the host's exec channel (no PTY, empty stdin). Resolves to its exit code.
function execOn(h, command, { cwd, onOut, onErr, abort }) {
  return connect(h).then((client) => new Promise((resolve, reject) => {
    const line = cwd ? `cd ${shq(cwd)} && ${command}` : command;
    client.exec(line, (err, stream) => {
      if (err) { dropConn(h.id); reject(err); return; }
      let code = null;
      stream.on('data', (d) => onOut(d));
      stream.stderr.on('data', (d) => onErr(d));
      stream.on('exit', (c, signal) => { code = c ?? (signal ? 255 : 0); });
      stream.on('close', () => {
        if (code === null && !abort.aborted) { dropConn(h.id); reject(new Error(`The connection to ${h.name} closed before the command finished.`)); return; }
        touch(h.id);
        resolve(code ?? 255);
      });
      stream.end();
      abort.onAbort = () => { try { stream.signal('KILL'); } catch { /* not supported */ } try { stream.close(); } catch { /* closed */ } };
    });
  }));
}

function copyFile(h, direction, local, remote) {
  return connect(h).then((client) => new Promise((resolve, reject) => {
    client.sftp((err, sftp) => {
      if (err) { reject(err); return; }
      const target = remote.replace(/^~\//, ''); // SFTP starts in the home folder
      const done = (e) => { sftp.end(); touch(h.id); if (e) reject(e); else resolve(); };
      if (direction === 'up') sftp.fastPut(local, target, done);
      else sftp.fastGet(target, local, done);
    });
  }));
}

// ---------- terminals on a host ----------

// The user's terminal on a host is an SSH shell with a PTY, shown in a Crew pane. A headless
// xterm mirrors it, so an agent can read what's on screen (crew-ssh screen <host>).
const shells = new Map(); // host id -> Set of { stream, mirror }

function openShell(hostId, cols, rows, onData, onClose) {
  const found = findHost(hostId);
  if (found.error) return Promise.reject(new Error(found.error));
  const h = found.host;
  return connect(h).then((client) => new Promise((resolve, reject) => {
    client.shell({ term: 'xterm-256color', cols, rows }, (err, stream) => {
      if (err) { reject(err); return; }
      const { Terminal } = require('@xterm/headless');
      const { StringDecoder } = require('string_decoder');
      const mirror = new Terminal({ cols, rows, scrollback: 2000, allowProposedApi: true });
      const decoder = new StringDecoder('utf8'); // a character can be split across chunks
      const rec = { stream, mirror };
      if (!shells.has(h.id)) shells.set(h.id, new Set());
      shells.get(h.id).add(rec);
      hold(h.id, 1);
      notifyHosts();
      const out = (d) => { const t = decoder.write(d); if (t) { mirror.write(t); onData(t); } };
      stream.on('data', out);
      stream.stderr?.on('data', out);
      let code = 0;
      stream.on('exit', (c) => { code = c ?? 0; });
      stream.on('close', () => {
        shells.get(h.id)?.delete(rec);
        mirror.dispose();
        hold(h.id, -1);
        notifyHosts();
        onClose(code);
      });
      resolve({
        write: (d) => { try { stream.write(d); } catch { /* closed */ } },
        resize: (c, r) => { try { stream.setWindow(r, c, 0, 0); mirror.resize(c, r); } catch { /* closed */ } },
        kill: () => { try { stream.close(); } catch { /* closed */ } },
      });
    });
  }));
}

// The last `count` lines of the most recently opened terminal on the host, as plain text.
function screenText(h, count = 60) {
  const recs = [...(shells.get(h.id) || [])];
  const rec = recs[recs.length - 1];
  if (!rec) return null;
  const b = rec.mirror.buffer.active;
  const lines = [];
  for (let i = 0; i < b.length; i++) lines.push(b.getLine(i)?.translateToString(true) || '');
  while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
  return lines.slice(-count).join('\n');
}

// ---------- which commands only read ----------

// Commands that run without approval in the default mode: every part of the line must be one of
// these (pipes and && / || / ; chains are split), with no redirection into files, no command
// substitution, no sudo and no environment assignments. Anything unsure needs approval.
const READ_COMMANDS = new Set(('ls ll cat head tail grep egrep fgrep rg stat file wc du df free uptime uname hostname whoami id groups who w last date pwd echo printf printenv which type ps pgrep vmstat iostat mpstat lscpu lsblk lsof lsmem lspci lsusb ss netstat ip ifconfig route ping traceroute tracepath dig nslookup host journalctl dmesg sort uniq cut tr tree realpath readlink basename dirname md5sum sha1sum sha256sum cksum getent nproc arch sensors zcat zgrep jq column nl diff cmp comm test true').split(' '));
const READ_SUBCOMMANDS = {
  systemctl: /^(status|is-active|is-enabled|is-failed|list-units|list-unit-files|list-timers|show|cat)\b/,
  docker: /^((ps|images|logs|inspect|version|info|top|port|stats\s.*--no-stream|history)\b|(container|image|network|volume)\s+(ls|list|inspect)\b|compose\s+(ps|logs|config|ls|images)\b)/,
  git: /^(status|log|diff|show|branch|remote|rev-parse|describe|blame|ls-files|ls-remote|shortlog|reflog)\b/,
  kubectl: /^(get|describe|logs|top|version|explain|api-resources|cluster-info|config\s+(view|get-contexts|current-context))\b/,
  apt: /^(list|show|policy|search)\b/, 'apt-cache': /^/, dpkg: /^(-l|-L|-s|-S|--list|--status|--listfiles|--search)\b/,
  rpm: /^-q/, yum: /^(list|info|search|repolist)\b/, dnf: /^(list|info|search|repolist)\b/,
  npm: /^(ls|list|view|outdated|config\s+get)\b/, pip: /^(list|show|freeze)\b/, pip3: /^(list|show|freeze)\b/,
  pm2: /^(list|ls|status|show|describe|logs\b.*--nostream)/, service: /^\S+\s+status\b|^--status-all\b/,
  nginx: /^(-t|-T|-v|-V)\s*$/, crontab: /^-l\b/, ufw: /^status\b/, timedatectl: /^(status)?\s*$/, hostnamectl: /^(status)?\s*$/,
  'firewall-cmd': /^(--state|--list-\S+|--get-\S+)(\s|$)/, iptables: /^(-L|-S)\b/, find: /^/, sed: /^/, curl: /^/, top: /^-b\b/, awk: null,
};

function onlyReads(command) {
  const cmd = command.trim();
  if (!cmd || /\n/.test(cmd) || /`|\$\(|<\(|>\(/.test(cmd)) return false;
  // Redirections other than to /dev/null or between streams write files.
  const withoutHarmless = cmd.replace(/\d?>>?\s*\/dev\/null|\d?>&\d/g, '');
  if (/[<>]/.test(withoutHarmless)) return false;
  for (const part of cmd.split(/\|\|?|&&|;/)) {
    const words = part.trim().split(/\s+/);
    const [name, ...args] = words;
    if (!name) return false;
    if (/=/.test(name) || name === 'sudo' || name === 'su' || name === 'xargs' || name === 'tee' || name === 'env' && args.length) return false;
    let rest = args.join(' ');
    if (name === 'git') rest = rest.replace(/^((-C|-c)\s+\S+\s+|--no-pager\s+)+/, ''); // options before the subcommand
    if (name === 'find' && /-(exec|execdir|ok|okdir|delete|fprint|fprintf|fls)\b/.test(rest)) return false;
    if (name === 'sed' && /(^|\s)-[a-zA-Z]*i|--in-place/.test(rest)) return false;
    if (name === 'curl' && /(^|\s)(-[a-zA-Z]*[oOTdFX]|--(output|remote-name|upload-file|data\S*|form|request))/.test(rest)) return false;
    if (READ_COMMANDS.has(name)) continue;
    const sub = READ_SUBCOMMANDS[name];
    if (sub && sub.test(rest)) continue;
    return false;
  }
  return true;
}

// ---------- approvals and the activity log ----------

const activity = new Map(); // host id -> recent agent commands
const allowedUntil = new Map(); // host id -> time until which everything is allowed

function logActivity(h, { kind, command, cwd, session, agent, decision, code, error }) {
  const list = activity.get(h.id) || [];
  list.unshift({ at: Date.now(), kind, command, cwd, session, agent, decision, code, error }); // plain data: it goes to the window
  activity.set(h.id, list.slice(0, LOG_SIZE));
  const w = getWin();
  if (w && !w.isDestroyed()) w.webContents.send('ssh:activity', h.id);
}

// Resolves to 'auto' | 'approved' (ran without or with the user) or null when denied.
async function approve(h, req, waiting) {
  const mode = h.approval || 'changes';
  const reads = req.kind === 'exec' ? onlyReads(req.command) : req.kind === 'download';
  if (mode === 'auto' || (allowedUntil.get(h.id) || 0) > Date.now() || (mode === 'changes' && reads)) return 'auto';
  waiting('waiting for you to approve it in Crew');
  const p = ask('approve', { hostId: h.id, host: label(h), action: req.kind, command: req.command, cwd: req.cwd || '', session: req.session, agent: req.agent });
  req.cancelPrompt = p.cancel;
  const answer = await p.promise;
  if (answer === 'host') { allowedUntil.set(h.id, Date.now() + ALLOW_FOR_MS); return 'approved'; }
  return answer === 'once' ? 'approved' : null;
}

// ---------- the bridge the `crew-ssh` command talks to ----------

const extraOps = {}; // other commands on the same pipe, e.g. `crew` (agents.js)
const registerOps = (ops) => Object.assign(extraOps, ops);

const bridge = {
  pipe: process.platform === 'win32' ? `\\\\.\\pipe\\crew-ssh-${crypto.randomBytes(8).toString('hex')}` : path.join(dataDir(), 'crew-ssh.sock'),
  token: crypto.randomBytes(24).toString('hex'),
};

function sameToken(t) {
  const a = Buffer.from(String(t || ''));
  const b = Buffer.from(bridge.token);
  return a.length === b.length && crypto.timingSafeEqual(a, b);
}

function handleClient(sock) {
  let buf = '';
  let started = false;
  const abort = { onAbort: null, aborted: false };
  let req = null;
  const send = (m) => { if (!sock.destroyed) sock.write(`${JSON.stringify(m)}\n`); };
  const fail = (message) => { send({ type: 'fail', message }); sock.end(); };
  sock.on('close', () => {
    abort.aborted = true;
    abort.onAbort?.();
    req?.cancelPrompt?.();
  });
  sock.on('error', () => { /* the client went away */ });
  sock.on('data', async (d) => {
    buf += d;
    const i = buf.indexOf('\n');
    if (i < 0 || started) return;
    started = true;
    let m;
    try { m = JSON.parse(buf.slice(0, i)); } catch { fail('Bad request.'); return; }
    if (!sameToken(m.token)) { fail('Not authorized.'); return; }
    try {
      if (extraOps[m.op]) { await extraOps[m.op](m, { send, fail, end: () => sock.end() }); return; } // the crew command (agents.js)
      if (m.op === 'hosts') {
        send({ type: 'result', hosts: allHosts().map((h) => ({ id: h.id, name: h.name, host: h.host, port: h.port || 22, user: h.user || '', group: h.group || '', notes: h.notes || '', approval: h.approval || 'changes' })) });
        sock.end();
        return;
      }
      const found = findHost(m.host);
      if (found.error) { fail(found.error); return; }
      const h = found.host;
      const waiting = (text) => send({ type: 'status', message: text });
      if (m.op === 'screen') {
        const text = screenText(h, Math.min(500, Math.max(1, +m.lines || 60)));
        if (text == null) { fail(`No terminal on ${h.name} is open in Crew. Use crew-ssh exec to run commands instead.`); return; }
        send({ type: 'out', d: Buffer.from(`${text}\n`).toString('base64') });
        send({ type: 'exit', code: 0 });
        sock.end();
        return;
      }
      if (m.op === 'exec') {
        if (!String(m.command || '').trim()) { fail('No command given.'); return; }
        req = { kind: 'exec', command: m.command, cwd: m.cwd || '', session: m.session || '', agent: m.agent || '' };
        const decision = await approve(h, req, waiting);
        if (abort.aborted) return;
        if (!decision) { logActivity(h, { ...req, decision: 'denied' }); fail('The user denied this command in Crew. Ask them before trying again.'); return; }
        if (!pool.has(h.id)) waiting(`connecting to ${h.name}`);
        let code;
        try {
          code = await execOn(h, m.command, {
            cwd: m.cwd,
            abort,
            onOut: (b) => send({ type: 'out', d: b.toString('base64') }),
            onErr: (b) => send({ type: 'err', d: b.toString('base64') }),
          });
        } catch (e) {
          logActivity(h, { ...req, decision, error: e.message });
          fail(e.message);
          return;
        }
        logActivity(h, { ...req, decision, code });
        send({ type: 'exit', code });
        sock.end();
      } else if (m.op === 'cp') {
        const direction = m.direction === 'up' ? 'up' : 'down';
        req = { kind: direction === 'up' ? 'upload' : 'download', command: `${direction === 'up' ? `${m.local} → ${h.name}:${m.remote}` : `${h.name}:${m.remote} → ${m.local}`}`, session: m.session || '', agent: m.agent || '' };
        const decision = await approve(h, req, waiting);
        if (abort.aborted) return;
        if (!decision) { logActivity(h, { ...req, decision: 'denied' }); fail('The user denied this copy in Crew.'); return; }
        try {
          await copyFile(h, direction, m.local, m.remote);
        } catch (e) {
          logActivity(h, { ...req, decision, error: e.message });
          fail(e.message);
          return;
        }
        logActivity(h, { ...req, decision, code: 0 });
        send({ type: 'exit', code: 0 });
        sock.end();
      } else {
        fail(`Unknown operation "${m.op}".`);
      }
    } catch (e) {
      fail(e.message || String(e));
    }
  });
}

function startBridge() {
  if (process.platform !== 'win32') { try { fs.unlinkSync(bridge.pipe); } catch { /* none */ } }
  const server = net.createServer(handleClient);
  server.on('error', () => { /* the command reports that Crew can't be reached */ });
  server.listen(bridge.pipe, () => {
    if (process.platform !== 'win32') { try { fs.chmodSync(bridge.pipe, 0o600); } catch { /* best effort */ } }
  });
  // For crew-ssh run outside Crew's sessions (e.g. in the user's own terminal).
  try { fs.writeFileSync(file('ssh-bridge.json'), JSON.stringify(bridge), { mode: 0o600 }); } catch { /* sessions still get the env */ }
}

// ---------- the crew-ssh command ----------

const binDir = () => file('bin');

// Writes crew-ssh and crew into Crew's data folder (a .cmd for cmd/PowerShell and a script for
// Git Bash each), running their .js with Crew's own executable in Node mode. Sessions get them
// on their PATH.
function installCli() {
  try {
    fs.mkdirSync(binDir(), { recursive: true });
    const fwd = (p) => p.replace(/\\/g, '/');
    for (const name of ['crew-ssh', 'crew']) {
      const js = path.join(binDir(), `${name}.js`);
      fs.writeFileSync(js, fs.readFileSync(path.join(__dirname, `${name}.js`)));
      if (process.platform === 'win32') {
        fs.writeFileSync(path.join(binDir(), `${name}.cmd`), `@echo off\r\nset ELECTRON_RUN_AS_NODE=1\r\n"${process.execPath}" "${js}" %*\r\n`);
      }
      fs.writeFileSync(path.join(binDir(), name), `#!/bin/sh\nELECTRON_RUN_AS_NODE=1 exec "${fwd(process.execPath)}" "${fwd(js)}" "$@"\n`, { mode: 0o755 });
    }
    return true;
  } catch {
    return false;
  }
}

// What a session needs to use crew-ssh: the command on its PATH and the way to reach Crew.
function sessionEnv(env, sessionId, agent) {
  const pathKey = Object.keys(env).find((k) => k.toUpperCase() === 'PATH') || 'PATH';
  return {
    ...env,
    [pathKey]: `${binDir()}${path.delimiter}${env[pathKey] || ''}`,
    CREW_SSH_PIPE: bridge.pipe,
    CREW_SSH_TOKEN: bridge.token,
    CREW_SESSION_ID: sessionId || '',
    CREW_AGENT: agent || '',
  };
}

// The skill that tells Claude Code and Codex how to use crew-ssh.
const SKILL = `---
name: crew-ssh
description: Run commands on, and copy files to or from, the user's saved SSH hosts (servers, VPSes, other machines) through Crew. Use whenever the user asks to check, manage, deploy to, debug or change a remote machine.
---

# Remote hosts through Crew

The user keeps their SSH hosts and credentials in Crew. Reach them only with the \`crew-ssh\` command: never ask for, read or handle passwords or keys, and don't fall back to \`ssh\`, \`scp\` or \`sftp\`.

- \`crew-ssh hosts --json\` lists the saved hosts: name, address, user, group, \`notes\` and \`approval\`.
- \`crew-ssh exec <host> [--cwd DIR] -- <command>\` runs a command on the host with its login shell and prints its output. It exits with the command's exit status, or 255 when Crew couldn't run it (the reason starts with \`crew-ssh:\`).
- \`crew-ssh cp <local> <host>:<remote>\` uploads one file; \`crew-ssh cp <host>:<remote> <local>\` downloads one.
- \`crew-ssh screen <host>\` prints what's on the user's terminal on that host in Crew (read it when they refer to something on their screen).

## Choosing a host

A host whose name matches the user's word exactly comes first; otherwise match addresses and groups. Ask only when one machine is meant and several match. For a group, work on each of its hosts. Each host's \`notes\` are the user's instructions for that machine: follow them.

If the machine isn't saved in Crew, say so and ask the user to add it (Hosts in Crew's sidebar).

## Approval

Commands that only read (\`df -h\`, \`systemctl status nginx\`, \`journalctl -u app -n 200 --no-pager\`, \`docker ps\` …) run at once. Anything that may change something, and every upload, waits until the user approves it in Crew. Before such a command, say in one line what you'll run and why. Give \`crew-ssh\` a long timeout (10 minutes) so the user has time to approve, or to answer a password or host-key prompt on a first connection. If the user denies a command, don't run it again or work around it; ask them.

## Commands

There's no terminal and no input: use non-interactive forms (\`--no-pager\`, \`-y\`, \`DEBIAN_FRONTEND=noninteractive\`, \`systemctl --no-pager\`). Keep output short (\`| tail -n 100\`). If \`sudo\` needs a password, ask the user to run that step. After a timeout or a \`crew-ssh:\` error, check what the command already did before running it again.

If \`crew-ssh\` isn't found or can't reach Crew, the session wasn't started from Crew or Crew isn't running: tell the user.
`;

function installSkill() {
  const targets = [
    path.join(os.homedir(), '.claude', 'skills', 'crew-ssh', 'SKILL.md'),
    path.join(process.env.CODEX_HOME || path.join(os.homedir(), '.codex'), 'skills', 'crew-ssh', 'SKILL.md'),
  ];
  const done = [];
  for (const t of targets) {
    try { fs.mkdirSync(path.dirname(t), { recursive: true }); fs.writeFileSync(t, SKILL); done.push(t); } catch { /* report the others */ }
  }
  return done;
}

// ---------- window side ----------

function notifyHosts() {
  const w = getWin();
  if (w && !w.isDestroyed()) w.webContents.send('ssh:hosts-changed');
}

ipcMain.handle('hosts:list', () => allHosts().map(publicHost));

ipcMain.handle('hosts:save', (_e, input) => {
  const list = allHosts();
  const h = list.find((x) => x.id === input.id) || { id: crypto.randomUUID() };
  const isNew = !list.includes(h);
  for (const k of ['name', 'host', 'user', 'auth', 'keyPath', 'group', 'notes', 'approval']) h[k] = (input[k] ?? '').toString().trim();
  h.port = +input.port || 22;
  if (!h.name) h.name = h.host;
  h.secret = { ...(h.secret || {}) };
  if (input.password) h.secret.password = seal(input.password);
  if (input.clearPassword) delete h.secret.password;
  if (input.passphrase) h.secret.passphrase = seal(input.passphrase);
  if (input.clearPassphrase) delete h.secret.passphrase;
  if (isNew) list.push(h);
  dropConn(h.id); // the next use signs in with the new settings
  saveHosts();
  notifyHosts();
  return publicHost(h);
});

ipcMain.handle('hosts:delete', (_e, id) => {
  dropConn(id);
  hosts = allHosts().filter((h) => h.id !== id);
  activity.delete(id);
  saveHosts();
  notifyHosts();
});

ipcMain.handle('hosts:test', async (_e, id) => {
  const found = findHost(id);
  if (found.error) return { ok: false, message: found.error };
  let out = '';
  try {
    const code = await execOn(found.host, 'echo "$(whoami)@$(hostname) · $(uname -sr 2>/dev/null || ver)"', { abort: {}, onOut: (b) => { out += b; }, onErr: () => {} });
    return { ok: code === 0, message: out.trim() || `Exit code ${code}` };
  } catch (e) {
    return { ok: false, message: e.message };
  }
});

ipcMain.handle('hosts:disconnect', (_e, id) => dropConn(id));
ipcMain.handle('hosts:workspace', (_e, id) => {
  const found = findHost(id);
  return found.error ? null : writeWorkspace(found.host);
});
ipcMain.handle('hosts:activity', (_e, id) => activity.get(id) || []);

ipcMain.handle('hosts:import', () => {
  let text;
  try { text = fs.readFileSync(path.join(os.homedir(), '.ssh', 'config'), 'utf8'); } catch { return { added: 0, missing: true }; }
  const list = allHosts();
  let added = 0;
  for (const c of parseSshConfig(text)) {
    if (list.some((h) => h.name.toLowerCase() === c.name.toLowerCase())) continue;
    list.push({ id: crypto.randomUUID(), auth: c.keyPath ? 'key' : 'auto', approval: 'changes', port: 22, user: '', keyPath: '', group: 'ssh config', notes: '', ...c });
    added++;
  }
  if (added) { saveHosts(); notifyHosts(); }
  return { added };
});

ipcMain.handle('hosts:pick-key', async () => {
  const r = await dialog.showOpenDialog(getWin(), { title: 'Choose a private key', defaultPath: path.join(os.homedir(), '.ssh'), properties: ['openFile', 'showHiddenFiles'] });
  return r.canceled ? null : r.filePaths[0];
});

ipcMain.handle('hosts:install-skill', () => installSkill());
ipcMain.handle('hosts:show-skill', () => shell.openPath(path.join(os.homedir(), '.claude', 'skills', 'crew-ssh')));
ipcMain.handle('hosts:can-seal', () => canSeal());

// A reloaded page has lost its prompts: answer them "no" so the waiting commands fail instead
// of hanging.
function dropPrompts() {
  for (const [id, resolve] of prompts) { prompts.delete(id); resolve(null); }
}

function init(windowGetter) {
  getWin = windowGetter;
  getWin()?.webContents.on('did-start-loading', dropPrompts);
  installCli();
  startBridge();
}

function shutdown() {
  for (const id of [...pool.keys()]) dropConn(id);
}

module.exports = { init, shutdown, sessionEnv, openShell, registerOps, onlyReads, parseSshConfig };
