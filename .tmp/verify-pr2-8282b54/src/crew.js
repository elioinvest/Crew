// crew: lets an agent in a Crew session hand work to the user's other agents (saved in Crew
// under Agents), one after another or several at once. Talks to the running Crew over the same
// private pipe as crew-ssh. Run by Crew's own executable in Node mode (see installCli in ssh.js).

const fs = require('fs');
const net = require('net');
const path = require('path');

const HELP = `crew: hand tasks to the agents the user saved in Crew.

Usage:
  crew agents [--json]                 list the agents (name, Claude or Codex, what each may do)
  crew ask <agent> "<task>"            run an agent on a task and wait for its answer
  crew ask <agent> "<task>" --bg       start it and print its run id at once (to run several together)
  crew wait <id> [<id>...]             wait for runs and print their answers
  crew result <id>                     print a run's answer, or that it's still running
  crew runs                            list this session's runs
  crew stop <id>                       stop a run

Agents work in this folder (the current directory) and can't see this conversation: give each
a complete task, with the files and context it needs. The task can also come on stdin
(crew ask <agent> - < task.md).

One after another: crew ask Reviewer "..."   (waits)
At once:           crew ask Tester "..." --bg; crew ask Reviewer "..." --bg; crew wait r1 r2

Waiting stops after 9 minutes (exit status 3) while the run goes on: call crew wait again.
Give these commands a long timeout. Exit status: 0 done, 1 failed or stopped, 3 still running,
255 Crew couldn't do it (the reason starts with "crew:").`;

const die = (msg) => { process.stderr.write(`crew: ${msg}\n`); process.exit(255); };

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
    sock.write(`${JSON.stringify({ ...msg, token: b.token, session: process.env.CREW_SESSION_ID || '', depth: +process.env.CREW_DEPTH || 0 })}\n`);
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
      else if (m.type === 'status') process.stderr.write(`crew: ${m.message}…\n`);
      else if (m.type === 'result') { finished = true; onResult(m); }
      else if (m.type === 'exit') { finished = true; process.exitCode = m.code; }
      else if (m.type === 'fail') { finished = true; process.stderr.write(`crew: ${m.message}\n`); process.exitCode = 255; }
    }
  });
  sock.on('close', () => { if (!finished) die('Crew closed the connection before answering.'); });
}

const args = process.argv.slice(2);
const cmd = args.shift();
const flag = (name) => { const i = args.indexOf(name); if (i < 0) return false; args.splice(i, 1); return true; };

if (!cmd || cmd === 'help' || cmd === '--help' || cmd === '-h') {
  process.stdout.write(`${HELP}\n`);
} else if (cmd === 'agents') {
  const json = flag('--json');
  request({ op: 'agents' }, {
    onResult: ({ agents }) => {
      if (json) { process.stdout.write(`${JSON.stringify(agents, null, 2)}\n`); return; }
      if (!agents.length) { process.stdout.write('No agents are saved in Crew yet (Agents in the sidebar).\n'); return; }
      const can = { read: 'only reads', edit: 'edits and runs commands in the folder', full: 'may do anything' };
      for (const a of agents) process.stdout.write(`${a.name}\t${a.cli}${a.model ? ` (${a.model})` : ''}\t${can[a.access] || a.access}${a.instructions ? `\n    ${a.instructions.split('\n')[0].slice(0, 120)}` : ''}\n`);
    },
  });
} else if (cmd === 'ask') {
  const bg = flag('--bg') || flag('--background');
  const agent = args.shift();
  if (!agent) die('which agent? Usage: crew ask <agent> "<task>"');
  let task = args.join(' ');
  if (task === '-' || (!task && !process.stdin.isTTY)) task = fs.readFileSync(0, 'utf8');
  if (!task.trim()) die('no task given. Usage: crew ask <agent> "<task>"');
  request({ op: 'ask', agent, task, bg, cwd: process.cwd() });
} else if (cmd === 'wait' || cmd === 'result') {
  if (!args.length) die(`usage: crew ${cmd} <id> [<id>...]`);
  request({ op: cmd, ids: args });
} else if (cmd === 'stop') {
  if (!args[0]) die('usage: crew stop <id>');
  request({ op: 'stop', id: args[0] });
} else if (cmd === 'runs') {
  request({ op: 'runs' });
} else {
  die(`unknown command "${cmd}". Run crew --help.`);
}
