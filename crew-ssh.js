// crew-ssh: lets the agents in Crew's sessions use the SSH hosts saved in Crew, by name, without
// ever seeing their credentials. It forwards each request to the running Crew over a private
// pipe; Crew connects, asks the user where needed, and streams the output back.
// Run by Crew's own executable in Node mode (see installCli in ssh.js).

const fs = require('fs');
const net = require('net');
const path = require('path');

const HELP = `crew-ssh: run commands on the SSH hosts saved in Crew.

Usage:
  crew-ssh hosts [--json]                         list the saved hosts (no credentials)
  crew-ssh exec <host> [--cwd DIR] -- <command>   run a command with the host's login shell
  crew-ssh cp <local> <host>:<remote>             upload one file
  crew-ssh cp <host>:<remote> <local>             download one file
  crew-ssh screen <host> [--lines N]              what's on the user's terminal on the host in Crew

<host> is a saved host's name (or id). Crew holds the credentials, so never pass passwords
or keys. Commands that only read run at once; others wait until the user approves them in
Crew, so allow a long timeout. There's no terminal and no input: use non-interactive flags.

Exit status: the remote command's, or 255 when Crew couldn't run it (the reason is printed
on stderr, starting with "crew-ssh:").`;

const die = (msg) => { process.stderr.write(`crew-ssh: ${msg}\n`); process.exit(255); };

// Git Bash passes local paths as /c/Users/...; Crew needs C:\Users\...
function localPath(p) {
  const m = /^\/([a-zA-Z])(\/.*)?$/.exec(p);
  const win = process.platform === 'win32' && m ? `${m[1].toUpperCase()}:${(m[2] || '/').replace(/\//g, '\\')}` : p;
  return path.resolve(win);
}

function bridge() {
  if (process.env.CREW_SSH_PIPE && process.env.CREW_SSH_TOKEN) return { pipe: process.env.CREW_SSH_PIPE, token: process.env.CREW_SSH_TOKEN };
  try { return JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'ssh-bridge.json'), 'utf8')); } catch { return null; }
}

function request(msg, { onResult } = {}) {
  const b = bridge();
  if (!b) die("can't find Crew. Start this from a Crew session, or open Crew first.");
  const sock = net.connect(b.pipe);
  let buf = '';
  let finished = false;
  sock.on('connect', () => {
    sock.write(`${JSON.stringify({ ...msg, token: b.token, session: process.env.CREW_SESSION_ID || '', agent: process.env.CREW_AGENT || '' })}\n`);
  });
  sock.on('error', () => die("can't reach Crew. Is it running? (Restart the session if Crew was restarted.)"));
  sock.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      let m;
      try { m = JSON.parse(line); } catch { continue; }
      if (m.type === 'out') process.stdout.write(Buffer.from(m.d, 'base64'));
      else if (m.type === 'err') process.stderr.write(Buffer.from(m.d, 'base64'));
      else if (m.type === 'status') process.stderr.write(`crew-ssh: ${m.message}…\n`);
      else if (m.type === 'result') { finished = true; onResult(m); }
      else if (m.type === 'exit') { finished = true; process.exitCode = m.code; }
      else if (m.type === 'fail') { finished = true; process.stderr.write(`crew-ssh: ${m.message}\n`); process.exitCode = 255; }
    }
  });
  sock.on('close', () => {
    if (!finished) die('Crew closed the connection before the command finished.');
  });
  // Ctrl+C (or the agent's timeout) stops the remote command too: Crew sees the pipe close.
  process.on('SIGINT', () => { sock.destroy(); process.exit(130); });
}

const args = process.argv.slice(2);
const cmd = args.shift();

if (!cmd || cmd === 'help' || cmd === '--help' || cmd === '-h') {
  process.stdout.write(`${HELP}\n`);
} else if (cmd === 'hosts' || cmd === 'ls') {
  const json = args.includes('--json');
  request({ op: 'hosts' }, {
    onResult: ({ hosts }) => {
      if (json) { process.stdout.write(`${JSON.stringify(hosts, null, 2)}\n`); return; }
      if (!hosts.length) { process.stdout.write('No hosts are saved in Crew yet.\n'); return; }
      for (const h of hosts) {
        const addr = `${h.user ? `${h.user}@` : ''}${h.host}${h.port !== 22 ? `:${h.port}` : ''}`;
        process.stdout.write(`${h.name}\t${addr}${h.group ? `\t[${h.group}]` : ''}${h.notes ? `\n    notes: ${h.notes.replace(/\n/g, '\n           ')}` : ''}\n`);
      }
    },
  });
} else if (cmd === 'exec' || cmd === 'run') {
  const host = args.shift();
  if (!host) die('which host? Usage: crew-ssh exec <host> -- <command>');
  let cwd = '';
  while (args.length && args[0] !== '--' && args[0].startsWith('--')) {
    const opt = args.shift();
    if (opt === '--cwd') cwd = args.shift() || '';
    else if (opt.startsWith('--cwd=')) cwd = opt.slice(6);
    else die(`unknown option ${opt}`);
  }
  if (args[0] === '--') args.shift();
  const command = args.length === 1 ? args[0] : args.join(' ');
  if (!command.trim()) die('no command given. Usage: crew-ssh exec <host> -- <command>');
  request({ op: 'exec', host, command, cwd });
} else if (cmd === 'screen') {
  const host = args.shift();
  if (!host) die('which host? Usage: crew-ssh screen <host> [--lines N]');
  const i = args.indexOf('--lines');
  request({ op: 'screen', host, lines: i >= 0 ? +args[i + 1] || 60 : 60 });
} else if (cmd === 'cp' || cmd === 'copy') {
  if (args.length !== 2) die('usage: crew-ssh cp <local> <host>:<remote>  or  crew-ssh cp <host>:<remote> <local>');
  // host:path, but not a Windows drive letter (C:\...).
  const remote = (s) => { const m = /^([^:\\/]{2,}|[^:\\/]*[^:\\/A-Za-z][^:\\/]*):(.+)$/.exec(s); return m ? { host: m[1], path: m[2] } : null; };
  const [a, b] = args.map(remote);
  if (a && b) die('copy between two hosts isn\'t supported: copy to this machine first.');
  if (!a && !b) die('one side must be <host>:<path>.');
  if (b) request({ op: 'cp', direction: 'up', host: b.host, remote: b.path, local: localPath(args[0]) });
  else request({ op: 'cp', direction: 'down', host: a.host, remote: a.path, local: localPath(args[1]) });
} else {
  die(`unknown command "${cmd}". Run crew-ssh --help.`);
}
