// ego lite, the browser for agents (github.com/citrolabs/ego-lite), integrated the way ego intends:
// agents get ego's own official `ego-browser` skill, which sends anything that needs a browser to
// ego lite and defines how they work there (their own Space, pausing for the user when a step
// needs them). Crew adds no skill or rules of its own; it shows whether ego lite and the official
// skill are installed, and sets them up the way ego's README says: by giving an agent ego's own
// setup prompt (the renderer opens a session with it).

const { ipcMain } = require('electron');
const fs = require('fs');
const path = require('path');
const os = require('os');
const { spawn } = require('child_process');

// ego lite's README, "Let your agent set it up": paste this into your agent.
const SETUP_PROMPT = 'Set up ego lite for me: https://github.com/citrolabs/ego-lite\n\nRead `skills/ego-browser/references/install.md` and follow the steps to install ego lite.';
let deps = null; // { commandExists, cleanEnv, getConfig, setConfig }

// Where the official skill lives for each agent (ego's setup and `npx skills` put it there).
const skillHomes = () => ({
  claude: [path.join(os.homedir(), '.claude', 'skills', 'ego-browser', 'SKILL.md')],
  codex: [
    path.join(os.homedir(), '.agents', 'skills', 'ego-browser', 'SKILL.md'),
    path.join(process.env.CODEX_HOME || path.join(os.homedir(), '.codex'), 'skills', 'ego-browser', 'SKILL.md'),
  ],
});

function status() {
  const homes = skillHomes();
  return {
    installed: deps.commandExists('ego-browser', deps.cleanEnv()),
    skill: { claude: homes.claude.some((p) => fs.existsSync(p)), codex: homes.codex.some((p) => fs.existsSync(p)) },
    setupPrompt: SETUP_PROMPT,
    home: os.homedir(),
  };
}

// Crew 2.5.13 wrote a skill of its own, crew-browser, with its own rules. Ego's official skill
// replaces it, so remove it, but only a file Crew wrote.
function removeOldSkill() {
  const dirs = [
    path.join(os.homedir(), '.claude', 'skills', 'crew-browser'),
    path.join(process.env.CODEX_HOME || path.join(os.homedir(), '.codex'), 'skills', 'crew-browser'),
  ];
  for (const dir of dirs) {
    try {
      if (fs.readFileSync(path.join(dir, 'SKILL.md'), 'utf8').includes('The user set Crew so that agents use ego lite')) fs.rmSync(dir, { recursive: true, force: true });
    } catch { /* not there */ }
  }
}

// ---------- ego lite out of the way while agents work ----------

// ego lite opens a window to hold an agent's Space when none is open. While an agent is driving
// it (an `ego-browser` command is running), a small watcher minimizes such windows without
// activating them, so they don't come in front of the user. Windows the user already had open,
// and ones they restore after the watcher minimized them, are left alone. Windows only.
const WATCH_PS = String.raw`
param([int]$CrewPid)
Add-Type @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class EgoMin {
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  public static List<IntPtr> Windows(HashSet<uint> pids) {
    var list = new List<IntPtr>();
    EnumWindows((h, l) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pids.Contains(pid) && IsWindowVisible(h) && GetWindow(h, 4) == IntPtr.Zero && GetWindowTextLength(h) > 0) list.Add(h);
      return true;
    }, IntPtr.Zero);
    return list;
  }
}
'@
$users = @{}   # windows open while no agent was working: the user's
$done = @{}    # windows minimized once: if the user restores one, leave it
$tick = 0
while ($true) {
  if (($tick++ % 16) -eq 0) { try { [void][Diagnostics.Process]::GetProcessById($CrewPid) } catch { exit } }
  $ego = [Diagnostics.Process]::GetProcessesByName('ego')
  if ($ego.Length -gt 0) {
    $pids = New-Object 'System.Collections.Generic.HashSet[uint32]'
    foreach ($p in $ego) { [void]$pids.Add([uint32]$p.Id) }
    $wins = [EgoMin]::Windows($pids)
    if ([Diagnostics.Process]::GetProcessesByName('ego-browser').Length -eq 0) {
      $users = @{}; $done = @{}
      foreach ($h in $wins) { if (-not [EgoMin]::IsIconic($h)) { $users["$h"] = 1 } }
    } else {
      foreach ($h in $wins) {
        if (-not $users.ContainsKey("$h") -and -not $done.ContainsKey("$h") -and -not [EgoMin]::IsIconic($h)) {
          [void][EgoMin]::ShowWindow($h, 6)   # SW_MINIMIZE: minimize and give focus back to the window below
          $done["$h"] = 1
        }
      }
    }
  }
  Start-Sleep -Milliseconds 120
}
`;

let watcher = null;
function setWatcher(on) {
  if (process.platform !== 'win32') return;
  if (on && !watcher) {
    const script = path.join(os.tmpdir(), `crew-ego-watch-${process.pid}.ps1`);
    fs.writeFileSync(script, WATCH_PS);
    watcher = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script, '-CrewPid', String(process.pid)], { windowsHide: true, stdio: 'ignore' });
    watcher.on('exit', () => { watcher = null; fs.rm(script, { force: true }, () => {}); });
  } else if (!on && watcher) {
    try { watcher.kill(); } catch { /* gone */ }
    watcher = null;
  }
}

ipcMain.handle('ego:status', () => ({ ...status(), keepMinimized: deps.getConfig().egoKeepMinimized !== false }));
ipcMain.handle('ego:keep-minimized', (_e, on) => {
  deps.setConfig({ egoKeepMinimized: !!on });
  setWatcher(!!on);
  return !!on;
});

function init(d) {
  deps = d;
  removeOldSkill();
  setWatcher(deps.getConfig().egoKeepMinimized !== false);
}

function shutdown() { setWatcher(false); }

module.exports = { init, shutdown };
