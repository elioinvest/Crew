🌐 **English** · [中文](README.zh-CN.md)

<p align="center">
  <img src="build/icon.svg" alt="Crew icon" width="128" height="128">
</p>

Crew is a Windows desktop app for Claude Code and Codex. Run the official CLIs side by side in split panes, organized by project, with a real prompt box, one-click model and mode switching, live status for each agent, a built-in web browser and tray mode. Resume any past session with a single click.

Crew doesn't replace either tool. Every session is the official `claude` or `codex` command running in a real terminal, with your own account and history. Crew adds the workspace around them, and collects nothing itself: your code and conversations go only to the agents you run.

## Install

1. Download `Crew Setup <version>.exe` from the [Releases](https://github.com/elioinvest/Crew/releases) page.
2. Run it. You can choose the install folder, and it adds Start menu and desktop shortcuts.
3. The installer isn't code-signed, so Windows SmartScreen may warn about an unknown publisher. Click **More info**, then **Run anyway**.

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

### Git

- When the active session's folder is a Git repository, a branch button appears at the top right. Click it to open Git in a panel on the right, beside the terminal; drag the panel's left edge to resize it, and click the button again or the panel's **×** to close it. It follows the active session to its repository. To open any project, right-click it and choose **Open in Git**. A folder that isn't a repository yet can be initialized from there.
- The toolbar has **Commit**, **Pull**, **Push**, **Fetch**, **Branch**, **Merge**, **Stash**, **Discard** and **Tag**. Badges show the number of changed files and how many commits you're ahead of or behind the upstream branch.
- **History** shows the commit graph of all branches, tags and remotes. Select a commit to see its details and changed files, then a file to see its diff. Right-click a commit to check it out, branch or tag from it, merge, rebase, cherry-pick, revert or reset to it.
- **File status** lists staged and unstaged files with a commit box. Stage or unstage whole files (double-click, the **+** / **−** button, or **Ctrl**/**Shift**-click several) or single hunks from the diff. **Ctrl+Enter** commits.
- The sidebar lists branches, tags, remotes and stashes. Double-click a branch to check it out; right-click for more. Merges, rebases and cherry-picks that stop on a conflict show a banner with **Abort** (and **Continue**).
- Crew runs your own `git`, so it needs [Git for Windows](https://git-scm.com/download/win). Pushing and pulling use your configured credentials (Git Credential Manager). The view refreshes on its own while agents change files.

### Window

- The button at the top left hides or shows the sidebar.
- Closing the window keeps Crew running in the system tray, so sessions continue. Click the tray icon to reopen it, or right-click it and choose **Quit Crew** to exit. You can turn this off in Settings.
- Settings (gear icon) set the light or dark theme, the font size, and the command and arguments used to start each agent.

## Build from source

Requires Node.js 20 or later.

```
npm install
npm start          # run in development
npm run dist       # build the Windows installer into release/
```

## License

[MIT](LICENSE)
