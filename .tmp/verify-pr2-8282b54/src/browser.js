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

// ego lite's README, "Let your agent set it up": paste this into your agent.
const SETUP_PROMPT = 'Set up ego lite for me: https://github.com/citrolabs/ego-lite\n\nRead `skills/ego-browser/references/install.md` and follow the steps to install ego lite.';
let deps = null; // { commandExists, cleanEnv }

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

ipcMain.handle('ego:status', () => status());

function init(d) {
  deps = d;
  removeOldSkill();
}

module.exports = { init };
