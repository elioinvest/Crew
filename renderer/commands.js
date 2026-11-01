/* global api, $, el, state, inputEl, autosizeInput */
// Slash-command completion for the prompt box, like the @ agents list beside it. Typing a lone
// "/" opens the agent's own commands; ↑/↓ move, Enter or Tab completes the name (it never sends
// the message — a second Enter, or a space then Enter, does), Esc closes. The built-ins below
// are listed here; commands written on disk are scanned in the main process (commands:list).

const SLASH_COMMANDS = {
  // Claude Code 2.1.x. Most used first: with just "/" typed, the box shows the top of this list.
  claude: [
    ['clear', 'Start a new conversation with empty context'],
    ['compact', 'Summarize the conversation to free context'],
    ['model', 'Switch the model'],
    ['effort', 'Set the effort level'],
    ['config', 'Open settings'],
    ['context', 'Show what fills the context window'],
    ['usage', 'Show cost, plan usage and activity'],
    ['cost', 'Alias for /usage'],
    ['resume', 'Resume a conversation'],
    ['rename', 'Rename this session'],
    ['rewind', 'Rewind the conversation or code'],
    ['status', 'Show version, model and connections'],
    ['help', 'Show help and commands'],
    ['init', 'Create a CLAUDE.md for this project'],
    ['memory', 'Edit CLAUDE.md and auto memory'],
    ['permissions', 'Manage tool permission rules'],
    ['plan', 'Enter plan mode'],
    ['review', 'Alias of /code-review'],
    ['code-review', 'Review the current diff or a PR for bugs'],
    ['security-review', 'Review the branch for vulnerabilities'],
    ['simplify', 'Clean up changed code'],
    ['mcp', 'Manage MCP servers'],
    ['agents', 'Create and manage subagents'],
    ['skills', 'List available skills'],
    ['hooks', 'View hook configurations'],
    ['diff', 'Review changes in the working tree'],
    ['export', 'Export the conversation as text'],
    ['copy', 'Copy the last response'],
    ['btw', 'Ask a side question without adding to the conversation'],
    ['branch', 'Branch the conversation here'],
    ['fork', 'Copy the conversation into a background session'],
    ['tasks', 'Manage background work'],
    ['subtask', 'Spawn a forked subagent'],
    ['background', 'Detach this session as a background agent'],
    ['goal', 'Set a goal to keep working until it is met'],
    ['loop', 'Run a prompt repeatedly'],
    ['run', 'Build, launch and drive the app'],
    ['verify', 'Confirm a change works in the app'],
    ['theme', 'Change the color theme'],
    ['statusline', 'Configure the status line'],
    ['keybindings', 'Open the keyboard shortcuts file'],
    ['terminal-setup', 'Install a Shift+Enter keybinding'],
    ['focus', 'Toggle the focus view'],
    ['color', 'Set the prompt bar color'],
    ['login', 'Sign in'],
    ['logout', 'Sign out'],
    ['plugin', 'Manage plugins'],
    ['reload-plugins', 'Reload plugins'],
    ['reload-skills', 'Re-scan skills and commands on disk'],
    ['doctor', 'Check the installation and settings'],
    ['update-config', 'Change settings by describing them'],
    ['add-dir', 'Add a working directory to this session'],
    ['cd', 'Move this session to another directory'],
    ['sandbox', 'Toggle sandbox mode'],
    ['fast', 'Toggle fast mode'],
    ['voice', 'Toggle voice dictation'],
    ['exit', 'Exit the session'],
    ['bug', 'Report a bug or share the conversation'],
    ['feedback', 'Send feedback about Claude Code'],
    ['release-notes', 'View the changelog'],
    ['restart', 'Restart and resume this session'],
    ['upgrade', 'Open the upgrade page'],
    ['insights', 'Report on your recent sessions'],
    ['stats', 'Alias for /usage'],
    ['recap', 'Summarize this session in one line'],
    ['advisor', 'Enable or disable the advisor model'],
    ['autocompact', 'Set when context compacting starts'],
    ['output-style', 'List or switch output styles'],
    ['scroll-speed', 'Adjust mouse wheel scroll speed'],
    ['tui', 'Set the terminal UI renderer'],
    ['ide', 'Manage IDE integrations'],
    ['desktop', 'Continue in the Claude Code Desktop app'],
    ['mobile', 'Get the Claude mobile app'],
    ['teleport', 'Pull a cloud session here'],
    ['remote-control', 'Make this session controllable from claude.ai'],
    ['remote-env', 'Choose the cloud environment'],
    ['schedule', 'Manage cloud scheduled tasks'],
    ['ultrareview', 'Deep multi-agent review in the cloud'],
    ['stop', 'Stop the attached background session'],
    ['list-agents', 'List the agents this session can message'],
    ['import', 'Import setup from Codex, Gemini CLI or Cursor'],
    ['plugin-authoring', 'Reference for writing a mod'],
    ['workflows', 'Watch and manage workflows'],
    ['workflow-authoring', 'Reference for workflow scripts'],
    ['privacy-settings', 'View privacy settings'],
    ['usage-credits', 'Configure usage credits'],
    ['rate-limit-options', 'Ways to keep working at a usage limit'],
    ['team-onboarding', 'Generate a team onboarding guide'],
    ['skill-doctor', 'See what skills cost and how often they run'],
    ['dataviz', 'Design guidance for charts and dashboards'],
    ['deep-research', 'Research a question across the web'],
    ['design', 'Draft UI mockups as artboards'],
    ['design-login', 'Authorize design-system access'],
    ['design-sync', 'Upload the repo design system'],
    ['slides', 'Make a presentation artifact'],
    ['artifacts', 'List and attach artifacts'],
    ['chrome', 'Claude in Chrome settings'],
    ['claude-api', 'Claude API reference for this project'],
    ['claude-in-chrome', 'Do a task in the browser'],
    ['install-github-app', 'Install the Claude GitHub App'],
    ['install-slack-app', 'Install the Claude Slack app'],
    ['web-setup', 'Connect GitHub for cloud sessions'],
    ['setup-bedrock', 'Configure Amazon Bedrock'],
    ['setup-vertex', 'Configure Google Cloud'],
    ['autofix-pr', 'Watch the branch PR and fix CI failures'],
    ['batch', 'Plan large-scale parallel changes'],
    ['fewer-permission-prompts', 'Add allow rules for common read-only calls'],
    ['powerup', 'Learn Claude Code with quick lessons'],
  ],
  // Codex, in its own popup order (most used first).
  codex: [
    ['model', 'Choose what model and reasoning effort to use'],
    ['permissions', 'Choose what Codex is allowed to do'],
    ['review', 'Review my current changes and find issues'],
    ['rename', 'Rename the current thread'],
    ['new', 'Start a new chat during a conversation'],
    ['resume', 'Resume a saved chat'],
    ['init', 'Create an AGENTS.md for this project'],
    ['compact', 'Summarize the conversation'],
    ['recap', 'Summarize the current conversation now'],
    ['plan', 'Switch to Plan mode'],
    ['goal', 'Set the goal for a long-running task'],
    ['agents', 'Open the agent command center'],
    ['skills', 'Use skills'],
    ['diff', 'Show the git diff'],
    ['status', 'Show session config and token usage'],
    ['usage', 'View account usage'],
    ['theme', 'Choose a syntax highlighting theme'],
    ['keymap', 'Remap TUI shortcuts'],
    ['vim', 'Toggle Vim mode for the composer'],
    ['voice', 'Start or stop voice'],
    ['copy', 'Copy the last response'],
    ['export', 'Export the conversation as markdown'],
    ['raw', 'Toggle raw scrollback mode'],
    ['tui', 'Choose the TUI mode for the next launch'],
    ['mention', 'Mention a file'],
    ['cd', 'Change the working directory'],
    ['pwd', 'Show the working directory'],
    ['clear', 'Clear the terminal and start a new chat'],
    ['archive', 'Archive this session'],
    ['delete', 'Permanently delete this session'],
    ['fork', 'Fork the current chat'],
    ['worktree', 'Start or continue in a new worktree'],
    ['app', 'Continue in the Desktop app'],
    ['side', 'Start or resume a side conversation'],
    ['btw', 'Ask a side question without adding to the conversation'],
    ['presets', 'Run or save reusable prompts'],
    ['config', 'Configure Codex preferences'],
    ['memories', 'Configure memory use and generation'],
    ['hooks', 'View and manage lifecycle hooks'],
    ['mcp', 'List MCP tools'],
    ['apps', 'Manage apps'],
    ['plugins', 'Browse plugins'],
    ['import', 'Import setup from Claude Code'],
    ['experimental', 'Toggle experimental features'],
    ['daemon', 'Manage the local background server'],
    ['ps', 'List background terminals'],
    ['stop', 'Stop all background terminals'],
    ['warnings', 'View warnings'],
    ['title', 'Configure the terminal title'],
    ['statusline', 'Configure the status line'],
    ['pets', 'Choose or hide the terminal pet'],
    ['daybreak', 'Turn Daybreak on or off'],
    ['feedback', 'Send logs to maintainers'],
    ['logout', 'Log out of Codex'],
    ['quit', 'Exit Codex'],
    ['exit', 'Exit Codex'],
  ],
};

const commandPop = $('#command-pop');
const commandCache = new Map(); // "agent:project" -> the commands scanned on disk
const builtinCache = {}; // agent -> the built-ins above, as { name, description }
let commandState = null; // { token, matches, index }
let commandDismissed = null; // the token Esc closed, until the token changes

// The "/name" being typed, or null: it must be the whole draft, so a command never completes
// from the middle of a sentence (and a pasted multiline message can't trigger it).
const commandToken = () => {
  const m = /^\/([\w:-]*)$/.exec(inputEl.value);
  return m && inputEl.selectionStart === inputEl.selectionEnd ? m[1] : null;
};

function commandsFor(entry) {
  if (!entry || (entry.agent !== 'claude' && entry.agent !== 'codex')) return [];
  builtinCache[entry.agent] ||= SLASH_COMMANDS[entry.agent].map(([name, description]) => ({ name, description }));
  const key = `${entry.agent}:${entry.projectPath || ''}`;
  if (!commandCache.has(key)) {
    commandCache.set(key, []); // one scan per session folder; rows fill in when it lands
    api.listCommands(entry.agent, entry.projectPath || '').then((list) => {
      commandCache.set(key, Array.isArray(list) ? list : []);
      if (commandState) updateCommands();
    }).catch(() => {});
  }
  return [...commandCache.get(key), ...builtinCache[entry.agent]];
}

function hideCommands() {
  commandPop.classList.add('hidden');
  commandState = null;
}

// Prefix matches first, then names that merely contain it; the list's own order is kept within
// each group, so the common commands stay on top.
function matchCommands(list, query) {
  if (!query) return list;
  const prefix = [];
  const other = [];
  for (const c of list) {
    const name = c.name.toLowerCase();
    if (name.startsWith(query)) prefix.push(c);
    else if (name.includes(query)) other.push(c);
  }
  return [...prefix, ...other];
}

function updateCommands() {
  const entry = state.terms.get(state.active);
  const token = commandToken();
  if (token == null || !entry || entry.exited) { hideCommands(); return; }
  if (commandDismissed === token) { hideCommands(); return; }
  const matches = matchCommands(commandsFor(entry), token.toLowerCase()).slice(0, 8);
  if (!matches.length) { hideCommands(); return; }
  const index = commandState && commandState.token === token
    ? Math.min(commandState.index, matches.length - 1) : 0;
  commandState = { token, matches, index };
  commandPop.replaceChildren(...matches.map((c, i) => el('div', {
    class: `mention-item${i === index ? ' on' : ''}`,
    onmousedown: (e) => { e.preventDefault(); pickCommand(i); },
  },
  el('b', {}, `/${c.name}`),
  c.description ? el('span', { class: 'cmd-desc' }, c.description) : null,
  c.from === 'project' ? el('span', { class: 'cmd-from' }, 'project') : null)));
  commandPop.classList.remove('hidden');
}

function pickCommand(i) {
  const c = commandState?.matches[i];
  if (!c) return;
  inputEl.value = `/${c.name} `;
  inputEl.selectionStart = inputEl.selectionEnd = inputEl.value.length;
  commandDismissed = null;
  hideCommands();
  autosizeInput();
  inputEl.focus();
}

inputEl.addEventListener('input', updateCommands);
inputEl.addEventListener('click', updateCommands);
inputEl.addEventListener('blur', () => setTimeout(hideCommands, 150));
$('#composer-send').addEventListener('click', hideCommands);

// Capture phase on the box, so these keys reach the list before the prompt box sends or moves.
$('#composer').addEventListener('keydown', (e) => {
  if (!commandState || commandPop.classList.contains('hidden') || e.target !== inputEl) return;
  if (e.isComposing || e.keyCode === 229) return; // IME still composing
  const n = commandState.matches.length;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    commandState.index = (commandState.index + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
    updateCommands();
  } else if (e.key === 'Tab' && !e.shiftKey) {
    pickCommand(commandState.index);
  } else if (e.key === 'Enter') {
    if (e.shiftKey) return; // Shift+Enter is a line break: let the box have it
    // A fully typed command sends as usual; anything shorter completes first.
    const picked = commandState.matches[commandState.index];
    if (picked && picked.name.toLowerCase() === commandState.token.toLowerCase()) {
      hideCommands();
      return;
    }
    pickCommand(commandState.index);
  } else if (e.key === 'Escape') {
    commandDismissed = commandState.token;
    hideCommands();
  } else {
    return;
  }
  e.preventDefault();
  e.stopPropagation();
}, true);
