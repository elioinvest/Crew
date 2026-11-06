🌐 [中文](README.md) · **English**

<p align="center">
  <img src="rust/crates/crew-app/assets/icon.svg" alt="Crew icon" width="128" height="128">
</p>

Crew is a Windows desktop app for Claude Code and Codex. Run the official CLIs side by side in split panes, organized by project, with a real prompt box, one-click model and mode switching, live status for each agent, a built-in web browser and tray mode. Resume any past session with a single click.

Crew doesn't replace either tool. Every session is the official `claude` or `codex` command running in a real terminal, with your own account and history. Crew adds the workspace around them, and collects nothing itself: your code and conversations go only to the agents you run.

## Install

1. Download `Crew-Rust-<version>-win-x64.zip` from the [Releases](https://github.com/elioinvest/Crew/releases) page.
2. Unzip it anywhere and keep `crew.exe` and `crew-cli.exe` together.
3. Run `crew.exe`. It isn't code-signed, so Windows SmartScreen may warn about an unknown publisher: click **More info**, then **Run anyway**. Windows may also ask to allow it through the firewall if you use phone access.

### The agents

Crew runs the agents you already have. Install the ones you want to use:

| Agent | Install command | Also needs |
| --- | --- | --- |
| Claude Code | `irm https://claude.ai/install.ps1 \| iex` (in PowerShell) | [Git for Windows](https://git-scm.com/download/win) |
| Codex | `npm install -g @openai/codex` | [Node.js](https://nodejs.org/), which provides npm |

You don't have to do this in advance. If you start a session for an agent that isn't installed, Crew shows the command and an **Install** button that runs it for you, then starts the session. Each agent asks you to sign in the first time it runs: Claude Code with a Claude account or API key, Codex with a ChatGPT account or OpenAI API key.

## Use 

### Projects and sessions

- The sidebar lists every folder where you've used Claude Code or Codex. Click **+** next to **Projects** to add another folder.
- Inside a project, Claude and Codex sessions are listed separately, newest first. Click one to resume it.
- Start a new session with **New session** at the top, the **+** on a project or agent group, or the start screen.
- Double-click a session to rename it. Right-click it to restart, close, delete or copy its ID.
- Drag projects to reorder them, or right-click a project and choose **Move up**, **Move down** or **Move to top**. The order stays as you set it.
- **Ctrl+K** searches projects and sessions.

### Writing prompts

- Type in the box under the terminal and press **Enter** to send. **Shift+Enter** adds a new line.
- Paste a screenshot with **Ctrl+V**, drop files onto the box, or click **+** to attach files.
- The chips below the box switch the model, reasoning effort and permission mode. **Shift+Tab** cycles the permission mode, as in the terminal.
- While the box is empty, **Esc** and the arrow keys go to the agent, so you can answer its menus and prompts.
- You can also click into the terminal and type there directly.

### Split panes

- Split the main area with the buttons at the top right, or **Ctrl+\\** (right) and **Ctrl+Shift+\\** (down). Each pane's header has the same buttons and a **×** to close it.
- Click a pane to make it active. It gets an orange frame and an **Active** badge, and the prompt box and chips act on it.
- Drag a session from the sidebar onto a pane to show it there. Right-click a session for **Open in split right** or **Open in split down**.
- Drag the gaps between panes to resize them. Closing a pane doesn't stop its session; reopen it from the sidebar.
- A spinner next to a session means its agent is working. A green dot means it's finished and waiting for you.

### Browser and files

- Links in the conversation open in the built-in browser beside it. **Ctrl+click** opens them in your default browser instead.
- The globe button at the top right shows or hides the browser. It has tabs (**Ctrl+T**, **Ctrl+W**, **Ctrl+Tab**) and keeps you signed in to websites.
- Image paths in the conversation open in a viewer. Other file paths open in their default program.
- **Agents' browser:** Crew integrates [ego lite](https://github.com/citrolabs/ego-lite), a free browser built for agents, the way ego intends: with ego's official `ego-browser` skill, agents use ego lite for anything that needs a browser (sites, forms, sign-ins, screenshots, testing web apps), each in its own Space without taking over your screen. When a step needs you (a sign-in, a code), the agent pauses and tells you which Space to open. Settings shows whether ego lite and its official skill are installed, with buttons to download ego lite and to run its official skill installer.

### Files

- The files button at the top right, beside Git, opens the active session's folder as a tree in a panel on the right. Folders open one at a time; files Git ignores are shown dimmed. The panel follows the active session, and shares the right side with Git.
- **Name** finds files by name, **Content** finds lines inside files. Click a file to open it in a pane of its own in the main area, beside the terminal, with its whole content (images as images), with line numbers and syntax highlighting for its type (by [highlight.js](https://highlightjs.org/)); a content match opens at its line. Edit the file right there: **Ctrl+S** or **Save** saves it (keeping its line endings), Tab / Shift+Tab indent, and switching away asks first about unsaved changes.

### Token usage

- Beside the Claude and GPT limits in the status bar, Crew shows today's estimated cost and tokens. Click it for the **Usage** page: cost or tokens for the past 24 hours, 7, 30 or 90 days, per provider, with a chart, totals (processed, cached and uncached input, output, cache savings), cost by type, and a breakdown by model or by hour or day. **Limits** shows the plan limits.
- It's read from Claude Code's and Codex's session files on this computer, including custom providers' sessions. Costs are **estimates at API list prices**, so on a subscription they show what your use would cost, not what you paid. Prices come from [LiteLLM's list](https://github.com/BerriAI/litellm), fetched daily, with a built-in copy as a fallback. Click a model to see its details or set your own price, for example for a custom provider's model that isn't on the list; unpriced models are left out of the cost and the page says so.

### Git

- When the active session's folder is a Git repository, a branch button appears at the top right. Click it to open Git in a panel on the right, beside the terminal; drag the panel's left edge to resize it, and click the button again or the panel's **×** to close it. It follows the active session to its repository. To open any project, right-click it and choose **Open in Git**. A folder that isn't a repository yet can be initialized from there.
- The panel is organized like [Zed](https://github.com/zed-industries/zed)'s Git features. Its behavior was studied there; Crew's code is its own.
- **Worktrees** let a repository have several checkouts on disk, so sessions can work side by side without touching each other's files. Open the worktree picker at the top of the panel. Type a name or let Crew pick one. New worktrees start on a detached HEAD, based on the remote's default branch or the current one; use the branch picker to create or check out a branch, which can't be one in use by another worktree. Picking a worktree shows its session or starts one. Its sessions are listed under the same project, marked with its name.
- **Archive** a worktree Crew created to save its staged, unstaged and untracked changes and remove the folder; files Git ignores are deleted with it. Picking it again, or opening one of its sessions, brings everything back. **Delete** asks before removing a folder with changes (hold **Alt** to force it). Crew won't archive or delete a worktree while a session runs in it, and won't archive one it didn't create.
- **Settings → Git** sets where worktrees go (`../worktrees` by default) and a setup command that runs in each new one, with `CREW_WORKTREE_ROOT` and `CREW_MAIN_GIT_WORKTREE` set.
- **Changes** is one column of files in Staged and Unstaged sections, with checkboxes for a file or a whole section; the view options switch to Tracked and Untracked, or to a tree. At the bottom are the branch picker, the sync button and the commit editor. It commits what's staged, or all tracked files when nothing is (**Ctrl+Enter**), with Amend, Signoff and Skip hooks in its menu; **Uncommit** undoes the last commit. One button does what's next: Push, Pull, Publish or Fetch, and its menu has the rest, including Force push and each remote.
- **View Diff**, or clicking a file, opens the **Project Diff** beside the list: every change in one scrolling view, inline or side by side, with changed words highlighted. Stage, unstage or restore single hunks. A file with conflicts shows **Use** buttons for each side and **Use both**.
- **History** shows the commit graph. Select a commit or stash to see all its changes; right-click a commit or a branch or tag label for more. **View file history** on a file shows just its commits. The menu beside **Stage All** has stashing (all, tracked or staged), the stash picker, merging and tags.
- Crew runs your own `git`, so it needs [Git for Windows](https://git-scm.com/download/win). Pushing and pulling use your configured credentials (Git Credential Manager). The view refreshes on its own while agents change files.

### Remote hosts (SSH)

Agents can manage your servers over SSH, without ever seeing your passwords or keys. The idea comes from [Hatoba](https://github.com/scarletkc/Hatoba).

- Add servers under **Hosts** at the bottom of the sidebar, or choose **⋯ → Import from ~/.ssh/config**. Each host has a name, address, user, sign-in method (key file, ssh-agent or password), an optional group, and **notes for agents**, e.g. "Production: never restart the database."
- **Click a host** to open its workspace: your terminal on the server on one side, and an agent session (Claude, or Codex or one of your custom model providers from the **+** on the host) on the other. Chat with the agent, e.g. "why is nginx failing?" or "free up some disk space", and it works on that server. The agent shares the terminal with you, as in Hatoba: it reads what's on screen ("look at this error") and types into it (commands, answers to prompts, keys like Ctrl+C, programs like `top` or an editor), and you watch everything it types and can step in at any time. When the terminal asks for a password or a code, you type it, and the agent carries on. With the terminal's pane active, the prompt box below is its command line: what you type there runs in the terminal.
- A host unfolds in the sidebar like a project: **Terminal**, then its agent sessions, newest first. Click one to resume it beside the terminal.
- The agent knows which server it's on from the workspace's instructions, including your notes for that host. It uses `crew-ssh`: `send` and `screen` for the shared terminal, `exec` for commands run on their own, and `cp` for files. Agents in other sessions can use it too, by host name.
- Codex runs commands in a sandbox that can't reach Crew, so Crew adds a Codex rule (`~/.codex/rules/crew.rules`) that lets just `crew-ssh` and `crew` run outside it. Crew still asks you before anything that changes a host.
- Choose **⋯ → Teach Claude Code and Codex to use crew-ssh** once. It installs a skill so the agents know the command without being told.
- Crew keeps the credentials and signs in itself. Passwords are saved encrypted with Windows data protection, and the window and the agents never receive them.
- Commands that only read (`df`, `ps`, `systemctl status`, `journalctl`, `docker ps`…) run at once, whether run on their own or typed at the terminal's prompt. Anything that could change something, input into a full-screen program, and every upload, shows a card where you **Allow**, **Allow 15 min** (everything on that host) or **Deny**. Each host can instead ask before every command or run everything.
- The first connection to a host shows its key fingerprint to trust. If the key later changes, Crew blocks the connection.
- Each host's editor lists its recent agent commands and what happened to them.

### Agents (@-mentions)

Hand parts of a job to other agents from your main session, one after another or several at once.

- Add agents under **Agents** in the sidebar. **+** offers a code reviewer, a tester, a publisher or a blank agent. Each has a name, runs on Claude, Codex (Crew checks which are installed) or one of your custom model providers, with a model and reasoning effort picked from what it offers, has instructions, and has a **Can** setting: only read, edit and run commands in the project, or anything without asking. Give the last only to agents you trust with it.
- In a session, type `@` to pick agents, and say what should happen: "Build the signup flow. Then have @CodeReviewerAgent review it and @TesterAgent test it at the same time. If both pass, have @CodePublisherAgent publish it."
- The main agent hands each one a task with `crew ask` (`--bg` for several at once, then `crew wait`) and gets their answers back. Agents work in the same folder, in Claude's or Codex's non-interactive mode.
- A card at the top right shows each running agent, what it's doing and for how long, with **Stop**, **Result** and **Open** (continue a Claude agent's run as a normal session).
- Each agent uses its own CLI's quota, shown in the usage bar.

### Custom model providers

Add your own APIs as agents beside Claude and Codex: relays, self-hosted services and other OpenAI- or Anthropic-compatible endpoints.

- In **Settings → Model providers**, choose **+ Add provider** and fill in a provider ID, a display name, an icon (an image or an emoji; otherwise the name's first letter), the API URL, the API protocol, the API key and its models. **Fetch available models** reads the provider's list; you can also add models by hand, with a display name, and star the default.
- Each provider runs on the official CLI that speaks its protocol, with an environment of its own: a home folder Crew keeps for it, holding its settings, sessions and history apart from your own Claude and Codex. **OpenAI Responses** providers run on Codex with that folder as `CODEX_HOME`, whose `config.toml` makes the provider the default (your Windows sandbox setting is carried over); **Anthropic Messages** providers run on Claude Code with it as `CLAUDE_CONFIG_DIR`, through the official `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN` and `ANTHROPIC_MODEL` settings. Those are the protocols the two CLIs support (Codex no longer supports Chat Completions). Crew's `crew-ssh` and ego lite skills are linked into each home.
- Each provider appears with its icon in the **New session** menus, on hosts and as an option for @-agents, and has its own group in the sidebar. The header, the prompt box and the chips show the provider, its models and its reasoning effort. Picking another model or effort switches it in the running session, without a restart (Claude Code: `/model`, `/effort`; Codex: `/model … --reasoning=…`, with the provider's models given to Codex as a model catalog), and a session keeps what you picked. The catalog carries Codex's own base prompt, `codex-prompt.md` from [openai/codex](https://github.com/openai/codex) (Apache-2.0).
- The API key is saved encrypted with Windows data protection, and only the sessions you start on that provider receive it. Your own Claude and Codex settings files aren't changed.

### Phone access

Use Crew on your computer from your phone's browser (an iPhone too) or from the Crew app (Android; the source is in the separate CrewApp project).

- In **Settings → Phone access**, check **Let paired phones connect**, then scan the code in Settings with the phone camera to open Crew in the browser (or tap **Scan QR code** in the app). Or open the address shown in the phone browser and enter the code.
- The app lists the sessions and SSH terminals open in Crew and whether each is working or idle, shows their output live and lets you type into them. **Stop** interrupts what a session is doing, **＋** sends a photo or file from the phone to the agent (uploaded to the computer, its path put into the message), and **/** runs commands like `/model` and `/compact`. When a session finishes while you aren't watching it, the phone tells you. You can also resume or start sessions in your projects, follow agent runs, and answer `crew-ssh` approvals (Allow, Allow 15 min or Deny).
- The phone and the computer need to be on the same Wi-Fi, or anywhere if both have [Tailscale](https://tailscale.com). Crew listens on port 7788 and accepts only devices that have the pairing code. **New pairing code** cuts off the phones already paired until they scan again.

### Window

- The button at the top left hides or shows the sidebar.
- Closing the window keeps Crew running in the system tray, so sessions continue. Click the tray icon to reopen it, or right-click it and choose **Quit Crew** to exit. You can turn this off in Settings.
- The bar at the bottom shows your plan usage for each agent: how much of the 5-hour limit is used and when it resets, then the weekly limit. Hover for details. Crew asks each agent for its current numbers in the background, with your existing sign-in: Codex through its `app-server`, and Claude (Pro and Max plans) by running `/usage` in a hidden Claude Code window that saves no transcript. Claude sessions Crew starts also get a status line hook that updates the numbers after each reply, then runs your own status line as usual; your Claude settings files aren't changed. Turn the bar off in Settings.
- Settings (gear icon) set the language (Automatic follows Windows, English or 中文), the light or dark theme, the font size, and the command and arguments used to start each agent.

## Build from source

Requires Rust (the version is pinned in `rust/rust-toolchain.toml`) and the Visual Studio C++ build tools.

```
cd rust
cargo build --release -p crew-app -p crew-cli
```

The programs are `crew.exe` and `crew-cli.exe` in `target/release`.

## License

[MIT](LICENSE)

The Claude model list under the prompt box, with each model's reasoning effort levels, comes from the model list [T3 Code](https://github.com/pingdotgg/t3code) publishes (MIT License, Copyright (c) 2026 T3 Tools Inc.): Crew ships a copy and refreshes it once a day.

The Usage page's calculations and layout, and how it reads session files, follow [T3 Code](https://github.com/pingdotgg/t3code) (MIT license, Copyright (c) T3 Tools Inc.).
