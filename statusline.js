// Status line for the Claude sessions Crew starts (passed with `claude --settings`). Claude Code
// sends the session's data on stdin, including the plan's rate limits; this saves those for
// Crew's usage bar, then runs the status line the user configured, so theirs still shows.
// Run by Crew's own executable in Node mode: node statusline.js <usage file>

const fs = require('fs');
const path = require('path');
const os = require('os');
const { spawnSync } = require('child_process');

const usageFile = process.argv[2];
const chunks = [];
process.stdin.on('data', (d) => chunks.push(d));
process.stdin.on('end', () => {
  const input = Buffer.concat(chunks);
  let data = {};
  try { data = JSON.parse(input.toString('utf8')); } catch { /* not JSON: nothing to save */ }

  const limits = data.rate_limits;
  if (usageFile && limits && (limits.five_hour || limits.seven_day)) {
    try {
      const tmp = `${usageFile}.${process.pid}.tmp`;
      fs.writeFileSync(tmp, JSON.stringify({ at: Date.now(), rate_limits: limits }));
      fs.renameSync(tmp, usageFile); // the bar never reads a half-written file
    } catch { /* the bar just keeps the previous numbers */ }
  }

  // The user's own status line, from the same places Claude Code reads it (project first).
  const cwd = data.workspace?.project_dir || data.workspace?.current_dir || data.cwd || process.cwd();
  const files = [
    path.join(cwd, '.claude', 'settings.local.json'),
    path.join(cwd, '.claude', 'settings.json'),
    path.join(os.homedir(), '.claude', 'settings.json'),
  ];
  let command = null;
  for (const f of files) {
    try {
      const c = JSON.parse(fs.readFileSync(f, 'utf8')).statusLine?.command;
      if (c) { command = c; break; }
    } catch { /* missing or unreadable */ }
  }
  if (!command || command.includes('crew-statusline')) return;

  // Claude Code runs commands with Git Bash on Windows; do the same so bash-style lines work.
  const bash = process.platform === 'win32'
    ? [process.env.CLAUDE_CODE_GIT_BASH_PATH, 'C:\\Program Files\\Git\\bin\\bash.exe', 'C:\\Program Files (x86)\\Git\\bin\\bash.exe'].find((p) => p && fs.existsSync(p))
    : '/bin/sh';
  const opts = { input, cwd: fs.existsSync(cwd) ? cwd : undefined, timeout: 5000, windowsHide: true, env: { ...process.env, ELECTRON_RUN_AS_NODE: undefined } };
  const r = bash ? spawnSync(bash, ['-c', command], opts) : spawnSync(command, { ...opts, shell: true });
  if (r.stdout && r.stdout.length) process.stdout.write(r.stdout);
});
