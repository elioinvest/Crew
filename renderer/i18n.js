// Chinese for Crew's interface. The interface is written in English; in Chinese, this
// translates its text, tooltips and placeholders as they appear (menus, dialogs and cards are
// built on the fly, so a MutationObserver watches the page). Exact strings come from ZH, ones
// with names or numbers in them from ZH_PATTERNS. User content is never touched: terminals,
// diffs, commit messages, file, session, host and agent names, agent output and anything typed.
// The language is Settings → Language (Automatic follows Windows); changing it reloads the page.

const ZH = {
  // ---------- sidebar, header, panes ----------
  'New session': '新建会话', 'Projects': '项目', 'Add project folder': '添加项目文件夹', 'Search (Ctrl+K)': '搜索 (Ctrl+K)',
  'Search projects and sessions': '搜索项目和会话', 'Settings': '设置', 'Hide sidebar': '隐藏侧栏', 'Show sidebar': '显示侧栏',
  'Restart session': '重启会话', 'Close session (stops the process)': '关闭会话（会结束进程）', 'Split right (Ctrl+\\)': '向右分屏 (Ctrl+\\)',
  'Split down (Ctrl+Shift+\\)': '向下分屏 (Ctrl+Shift+\\)', 'Split right': '向右分屏', 'Split down': '向下分屏', 'Toggle browser': '显示/隐藏浏览器',
  'Close pane (the session keeps running)': '关闭窗格（会话继续运行）', 'Empty pane': '空窗格', 'Pick a session on the left, or drag one here.': '从左侧选择一个会话，或拖到这里。',
  'New Claude session here': '在此新建 Claude 会话', 'New Codex session here': '在此新建 Codex 会话', 'New Claude session': '新建 Claude 会话', 'New Codex session': '新建 Codex 会话',
  'Where should we start?': '从哪里开始？', 'Start Claude': '启动 Claude', 'Start Codex': '启动 Codex', 'Add a project folder…': '添加项目文件夹…',
  'Pick a session on the left to resume it, or start a new one in a project.': '从左侧选择会话以继续，或在某个项目中新建会话。',
  'No projects yet — add a folder with +': '还没有项目——用 + 添加文件夹', 'No sessions yet': '还没有会话', 'No matches': '没有匹配项',
  'Untitled': '未命名', 'Move up': '上移', 'Move down': '下移', 'Move to top': '移到顶部', 'Open folder': '打开文件夹', 'Copy path': '复制路径',
  'Hide from list': '从列表中隐藏', 'Open in Git': '在 Git 中打开', 'Open': '打开', 'Open in split right': '在右侧分屏打开', 'Open in split down': '在下方分屏打开',
  'Restart': '重启', 'Close': '关闭', 'Rename': '重命名', 'Copy session ID': '复制会话 ID', 'Delete': '删除', 'More': '更多', 'Working': '运行中', 'Idle': '空闲',
  'Fold': '折叠', 'Unfold': '展开',
  'now': '刚刚', 'Active': '当前', 'Double-click to check out': '双击检出', 'Double-click to check out as a local branch': '双击检出为本地分支', 'Click to open its terminal and agent': '点击打开它的终端和智能体',
  // ---------- prompt box and chips ----------
  'How can I help you today?': '今天需要我做什么？', 'Ask Codex to do anything': '让 Codex 做任何事', 'Attach files': '添加文件', 'Send (Enter)': '发送 (Enter)',
  'Model': '模型', 'Effort': '推理强度', 'Permission mode': '权限模式', 'Manual': '手动', 'Accept edits': '自动接受编辑', 'Plan': '计划', 'Auto': '自动',
  'Bypass': '跳过权限', "Don't ask": '不询问', 'Permissions': '权限', 'Untrusted': '不受信任', 'On failure': '失败时', 'On request': '按需', 'Never ask': '从不询问',
  'Default': '默认', 'Extra high': '超高', 'Low': '低', 'Medium': '中', 'High': '高', 'Max': '最高', 'Ultra': '极限', 'Minimal': '最低',
  'Refresh model list': '刷新模型列表', "Reading Claude's model list…": '正在读取 Claude 的模型列表…', "Reading Codex's models and effort levels…": '正在读取 Codex 的模型和推理强度…',
  "Couldn't read the model list — try again when Claude is idle": '无法读取模型列表——请在 Claude 空闲时重试',
  "Couldn't read Codex's model list — try again when Codex is idle": '无法读取 Codex 的模型列表——请在 Codex 空闲时重试',
  "Claude's model list changed — click the model again": 'Claude 的模型列表已变化——请再次点击模型',
  "Codex's model list changed — click again": 'Codex 的模型列表已变化——请再次点击',
  'Current Codex model not found — use Refresh model list': '未找到当前 Codex 模型——请刷新模型列表',
  'Install the agent above to start this session': '先安装上方的智能体才能启动此会话', 'Installing…': '正在安装…', 'Session stopped — press Enter to resume it': '会话已停止——按 Enter 继续',
  // ---------- missing agent ----------
  'It can be installed with:': '可以这样安装：', 'Check again': '重新检查', 'Install guide': '安装指南', 'Get Node.js': '获取 Node.js', 'Show installer output': '显示安装输出',
  'You sign in once, the first time the session starts. Installed somewhere else? Set the command in Settings.': '首次启动会话时登录一次即可。装在其他位置？请在设置中指定命令。',
  // ---------- settings ----------
  'Claude command': 'Claude 命令', 'Claude arguments': 'Claude 参数', 'Codex command': 'Codex 命令', 'Codex arguments': 'Codex 参数', 'Theme': '主题', 'Light': '浅色', 'Dark': '深色',
  'Font size': '字体大小', 'Language': '语言', 'Automatic': '自动', 'Keep running in the tray when the window is closed': '关闭窗口时在托盘中继续运行',
  'Show plan usage at the bottom (Claude sessions Crew starts get a status line hook that records it)': '在底部显示套餐用量（Crew 启动的 Claude 会话会加载一个记录用量的状态栏钩子）',
  'ego lite: the browser for agents': 'ego lite：智能体的浏览器', '✓ ego lite is installed': '✓ 已安装 ego lite', "✗ ego lite isn't installed": '✗ 未安装 ego lite',
  '✓ Official skill installed for Claude Code and Codex': '✓ 已为 Claude Code 和 Codex 安装官方技能', '✓ Official skill installed for Claude Code only': '✓ 仅为 Claude Code 安装了官方技能',
  '✓ Official skill installed for Codex only': '✓ 仅为 Codex 安装了官方技能', '✗ Official skill not installed': '✗ 未安装官方技能',
  "With ego's official skill, agents use ego lite for anything that needs a browser, each in its own Space. When a step needs you (a sign-in, a code), the agent pauses and tells you which Space to open.": '有了 ego 的官方技能，凡是需要浏览器的任务，智能体都会在 ego lite 中各自的空间里完成。遇到需要你本人操作的步骤（登录、验证码）时，智能体会暂停并告诉你打开哪个空间。',
  'Set up ego lite': '设置 ego lite', "Opens an agent session with ego's own setup prompt": '用 ego 官方的设置提示词打开一个智能体会话',
  'Set up ego lite?': '设置 ego lite？', 'Not now': '暂不', 'Set up': '设置',
  "Agents use ego lite, a free browser built for agents, for anything that needs a browser. It isn't installed yet.": '智能体会用 ego lite（一款为智能体打造的免费浏览器）处理所有需要浏览器的任务。目前尚未安装。',
  "Set up opens an agent session with ego's official setup prompt. The agent guides you through installing ego lite and its skill. You can also do this later in Settings.": '点击“设置”会用 ego 官方的设置提示词打开一个智能体会话，由智能体引导你安装 ego lite 及其技能。你也可以稍后在设置中进行。',
  'Command and arguments apply to sessions started after saving.': '命令和参数对保存后新启动的会话生效。', 'Cancel': '取消', 'Save': '保存',
  'e.g. --model opus': '例如 --model opus', 'e.g. -m gpt-5.5': '例如 -m gpt-5.5',
  // ---------- model providers ----------
  'Model providers': '模型提供商', '+ Add provider': '+ 添加提供商', 'Add a model provider': '添加模型提供商', 'Create provider': '创建提供商',
  'Your own APIs: relays, self-hosted services and other OpenAI- or Anthropic-compatible endpoints. Each one appears in the New session menus.': '你自己的 API：中转站、自部署服务及其他兼容 OpenAI 或 Anthropic 的接口。每个提供商都会出现在“新建会话”菜单中。',
  'None yet. Add a relay, a self-hosted service or another API compatible with OpenAI or Anthropic.': '还没有。可添加中转站、自部署服务或其他兼容 OpenAI 或 Anthropic 的接口。',
  'Connect a relay, a self-hosted service or another API compatible with OpenAI or Anthropic. Crew runs it with Codex or Claude Code, set up through their official options.': '连接中转站、自部署服务或其他兼容 OpenAI / Anthropic 协议的接口。Crew 通过 Codex 或 Claude Code 的官方配置项来运行它。',
  'Provider ID': 'Provider ID', 'Starts with a lowercase letter; a-z, 0-9 and -. It identifies the provider in Crew.': '以小写字母开头，只含 a-z、0-9 和 -，在 Crew 中唯一标识该提供商。',
  'Display name': '显示名称', 'Icon': '图标', 'Choose image…': '选择图片…', 'Emoji': 'Emoji', 'Reset': '重置',
  'An image (PNG, SVG, ICO… under 512 KB) or an emoji. Without one, the first letter of the name is shown.': '图片（PNG、SVG、ICO 等，小于 512 KB）或一个 Emoji。都不设置时显示名称的首字母。',
  'API URL': 'API 地址', 'API protocol': 'API 协议', 'API key': 'API 密钥', 'Enter the API key': '输入 API 密钥', 'Saved. Leave empty to keep it': '已保存，留空则保持不变',
  'Runs on Codex. For APIs that serve /v1/responses.': '由 Codex 运行，适用于提供 /v1/responses 的接口。',
  'Runs on Claude Code. For APIs that serve /v1/messages.': '由 Claude Code 运行，适用于提供 /v1/messages 的接口。',
  'For APIs that serve /v1/chat/completions, as most relays do.': '适用于提供 /v1/chat/completions 的接口，多数中转站都是如此。',
  'For APIs that serve /v1/responses.': '适用于提供 /v1/responses 的接口。', 'For APIs that serve /v1/messages.': '适用于提供 /v1/messages 的接口。',
  'Connect a relay, a self-hosted service or another API compatible with OpenAI or Anthropic. Crew runs it as an agent of its own, talking to the API directly.': '连接中转站、自部署服务或其他兼容 OpenAI / Anthropic 协议的接口。Crew 会把它作为独立的智能体运行，直接调用该 API。',
  // ---------- built-in agent (builtin.js) ----------
  'Custom provider': '自定义提供商', 'Thinking': '思考过程', 'running…': '运行中…', 'waiting for you': '等待你确认', 'failed': '失败', 'denied': '已拒绝',
  'Deny': '拒绝', 'Allow': '允许', 'Allow for this session': '本会话内都允许', 'Allow everything the agent does in this session from now on': '从现在起允许智能体在本会话中的所有操作',
  'A custom model provider, run on Codex': '自定义模型提供商，由 Codex 运行', 'A custom model provider, run on Claude Code': '自定义模型提供商，由 Claude Code 运行', 'It edits files inside the project folder and runs commands, without asking.': '它会在项目文件夹内编辑文件并运行命令，不经询问。',
  'Add models to this provider in Settings → Model providers.': '请在“设置 → 模型提供商”中为该提供商添加模型。',
  'Stopped.': '已停止。',
  'Save (Ctrl+S)': '保存 (Ctrl+S)', 'Unsaved changes': '未保存的更改', "Your changes are lost if you don't save them.": '如果不保存，你的更改将会丢失。', "Don't save": '不保存',
  'Staged changes': '已暂存的更改', 'Changes': '更改', 'Message': '信息', 'Commit options': '提交选项', 'Commit (Ctrl+Enter)': '提交 (Ctrl+Enter)', 'Clear and go to History': '清空并转到历史',
  // ---------- file browser (files.js) ----------
  'Files of this folder': '此文件夹的文件', 'Close files': '关闭文件', 'Name': '名称', 'Content': '内容', 'Find files': '查找文件', 'Find in files': '在文件中查找',
  'Click a file to see its content.': '点击文件查看其内容。', 'No matches': '没有匹配项', 'Empty': '空', 'Loading…': '加载中…', 'Open in its default app': '用默认程序打开',
  'This is a binary file.': '这是二进制文件。', 'The image is too large to show.': '图片太大，无法显示。',
  'Ask for approval': '请求批准', 'Approve for me': '替我批准', 'Read Only': '只读',
  'Read and edit workspace files and run commands; asks for internet access or edits outside the workspace': '读取和编辑工作区文件、运行命令；联网或修改工作区以外的文件时会询问',
  'Only asks for actions detected as potentially unsafe': '只在检测到可能不安全的操作时询问',
  'Use with caution: edits files outside this workspace and uses the internet without asking': '谨慎使用：可修改工作区以外的文件并联网，不经询问',
  'Reads workspace files; asks for edits or internet access': '读取工作区文件；编辑或联网时会询问',
  "Codex didn't open its permissions — try again when it's idle": 'Codex 没有打开权限设置——请在它空闲时重试', 'Request approval': '请求批准', 'Auto-approve': '自动批准', 'Full access': '完全访问', 'Stop (Esc)': '停止 (Esc)',
  'Edits and commands that change something wait for you': '会改动内容的编辑和命令都要等你确认', 'Edits inside the project folder and commands run; anything else asks': '项目文件夹内的编辑和命令直接运行，其他操作会先询问', 'Everything runs without asking, also outside the project': '一切操作都不经询问直接运行，包括项目之外',
  'It reads, searches and edits files and runs commands here, and reaches your saved hosts with crew-ssh. In Request approval mode, changes wait for you; the permission chip below changes that.': '它可以在这里读取、搜索和编辑文件、运行命令，并通过 crew-ssh 访问你保存的主机。在“请求批准”模式下，改动都会先等你确认；可在下方的权限选项中更改。',
  'list dir': '列出目录', 'read file': '读取文件', 'search': '搜索', 'write file': '写入文件', 'edit file': '编辑文件', 'run command': '运行命令',
  'The agent is still working: wait, or press Esc to stop it': '智能体仍在工作：请等待，或按 Esc 停止',
  'It reads, searches and edits files and runs commands here, and reaches your saved hosts with crew-ssh. Changes wait for your approval unless you allow them in the permission chip.': '它可以在这里读取、搜索和编辑文件、运行命令，并通过 crew-ssh 访问你保存的主机。除非你在权限选项中放行，否则改动都会先等你确认。',
  'Saved encrypted with Windows data protection. Agents get it only in the sessions you start on this provider.': '使用 Windows 数据保护加密保存，只在你用该提供商启动的会话中提供给智能体。',
  'Models': '模型目录', 'Fetch available models': '获取可用模型', '+ Add model': '+ 添加模型', 'Model ID, e.g. claude-sonnet-4-5 or gpt-5': '模型 ID，例如 claude-sonnet-4-5 或 gpt-5',
  "No models yet. Fetch the provider's list or add model IDs.": '还没有模型。可获取提供商的模型列表，或手动添加模型 ID。', 'New sessions use the default model': '新会话使用默认模型',
  'Default': '默认', 'Make this the default model for new sessions': '设为新会话的默认模型', 'That model is already in the list.': '该模型已在列表中。',
  'Model ID': '模型 ID', 'Default model for new sessions': '新会话的默认模型', 'The provider listed no models. Add them by hand.': '提供商没有列出模型，请手动添加。',
  "No models yet. Fetch the provider's list, or add a model and type its ID.": '还没有模型。可获取提供商的模型列表，或添加模型并填写其 ID。',
  "Each model has its ID (sent to the API) and the name shown in the session's model menu. ★ marks the default for new sessions.": '每个模型包含 ID（发送给 API）和在会话模型菜单中显示的名称。★ 标记新会话的默认模型。',
  'Type a model ID in the box first, then click + Add model. Models already in the list are included; the selected one is the default.': '请先在输入框中填写模型 ID，再点击“+ 添加模型”。列表中的模型都已包含，选中的是默认模型。',
  "All models in the list appear in the model menu of this provider's sessions. The selected one is the default for new sessions; × removes a model.": '列表中的所有模型都会出现在该提供商会话的模型菜单中。选中的是新会话的默认模型；× 可移除模型。',
  'Fetching the models…': '正在获取模型…', 'The provider listed no models. Add model IDs by hand.': '提供商没有列出模型，请手动添加模型 ID。', 'no models': '无模型',
  "Sessions started on it are kept, but resume on the agent's own account.": '用它启动的会话会保留，但恢复时将使用智能体自己的账号。',
  // ---------- browser and viewer ----------
  'New tab (Ctrl+T)': '新建标签页 (Ctrl+T)', 'Close browser': '关闭浏览器', 'Back': '后退', 'Forward': '前进', 'Reload': '重新加载', 'Enter a URL or search': '输入网址或搜索内容',
  'Open in default browser': '在默认浏览器中打开', 'Close tab': '关闭标签页', 'Show in folder': '在文件夹中显示', 'Close (Esc)': '关闭 (Esc)', 'Copy': '复制', 'Copied': '已复制',
  "Couldn't load this image. It may have been moved or deleted.": '无法加载此图片，它可能已被移动或删除。',
  // ---------- usage bar ----------
  'Usage': '用量', 'Refresh usage': '刷新用量', 'Checking plan usage…': '正在检查套餐用量…', 'Updated just now': '刚刚更新',
  // ---------- git ----------
  'Git (this project is a Git repository)': 'Git（此项目是 Git 仓库）', 'Close Git': '关闭 Git', 'Commit': '提交', 'Pull': '拉取', 'Push': '推送', 'Fetch': '获取',
  'Branch': '分支', 'Merge': '合并', 'Stash': '贮藏', 'Discard': '丢弃', 'Tag': '标签', 'Explorer': '资源管理器', 'Refresh': '刷新',
  'Commit the staged changes': '提交已暂存的更改', 'Pull from the upstream branch': '从上游分支拉取', 'Push to the upstream branch': '推送到上游分支', 'Fetch from all remotes': '从所有远程获取',
  'Create a branch': '创建分支', 'Merge a branch into the current branch': '把一个分支合并到当前分支', 'Stash the uncommitted changes': '贮藏未提交的更改',
  'Discard uncommitted changes': '丢弃未提交的更改', 'Create a tag': '创建标签', 'Open the repository folder': '打开仓库文件夹',
  'Workspace': '工作区', 'File status': '文件状态', 'History': '历史', 'Branches': '分支', 'Tags': '标签', 'Remotes': '远程', 'Stashes': '贮藏',
  'Graph': '图谱', 'Description': '描述', 'Date': '日期', 'Author': '作者', 'Committer': '提交者', 'Parent': '父提交', 'Parents': '父提交',
  'Uncommitted changes': '未提交的更改', 'No commits yet': '还没有提交', 'Staged files': '已暂存文件', 'Unstaged files': '未暂存文件', 'Nothing staged': '没有已暂存的文件', 'No changes': '没有更改',
  'Unstage all': '全部取消暂存', 'Unstage selected': '取消暂存所选', 'Stage all': '全部暂存', 'Stage selected': '暂存所选', 'Stage': '暂存', 'Unstage': '取消暂存',
  'Stage file': '暂存文件', 'Unstage file': '取消暂存文件', 'Discard file': '丢弃文件', 'Stage hunk': '暂存块', 'Unstage hunk': '取消暂存块', 'Discard hunk': '丢弃块',
  'Mark resolved': '标记为已解决', 'Select a file to view the diff': '选择文件以查看差异', 'Binary file: no text diff.': '二进制文件：没有文本差异。', 'No content changes.': '内容没有变化。',
  'No changes.': '没有更改。', "Couldn't load the diff.": '无法加载差异。', "Couldn't load this commit.": '无法加载此提交。', 'Changes compared with the first parent.': '与第一个父提交比较的更改。',
  'Large diff: the rest is hidden. Show the full diff': '差异较大，其余部分已隐藏。显示完整差异', 'Loading…': '正在加载…', 'Loading repository…': '正在加载仓库…', 'Show this commit': '显示此提交',
  'Modified': '已修改', 'Added': '已添加', 'Deleted': '已删除', 'Renamed': '已重命名', 'Copied ': '已复制', 'Type changed': '类型已变', 'Conflicted': '有冲突', 'Untracked': '未跟踪',
  'Commit message': '提交信息', 'Amend last commit': '修改上一次提交', 'Push changes immediately': '立即推送更改', 'Write a commit message first.': '请先填写提交信息。', 'There is nothing to commit.': '没有可提交的内容。',
  'Nothing staged ': '没有已暂存的文件', 'No files are staged. Stage all changes and commit them?': '没有已暂存的文件。暂存所有更改并提交吗？', 'Stage all and commit': '全部暂存并提交',
  'Amend commit': '修改提交', 'Open file': '打开文件', 'Open current version': '打开当前版本', 'Discard changes': '丢弃更改', 'Ignore this file': '忽略此文件',
  'Restore this version to the working copy': '把此版本恢复到工作副本', 'Restore file': '恢复文件', 'Restore': '恢复', "Couldn't update .gitignore.": '无法更新 .gitignore。',
  'Discard this hunk from the working copy? This can\'t be undone.': '要从工作副本中丢弃此块吗？此操作无法撤销。', 'It is untracked and will be deleted.': '它未被跟踪，将被删除。',
  'Not a Git repository': '不是 Git 仓库', 'Initialize': '初始化', 'Branch name': '分支名称', 'Check out the new branch': '检出新分支', 'Create branch': '创建分支', 'New branch': '新建分支',
  'Tag name': '标签名称', 'Message': '信息', 'Optional': '可选', 'Optional. With a message the tag is annotated.': '可选。填写信息即为附注标签。', 'Create tag': '创建标签', 'New tag': '新建标签',
  'Make a commit before adding a tag.': '请先提交，再添加标签。', 'Branch to merge': '要合并的分支', 'Always create a merge commit (--no-ff)': '总是创建合并提交 (--no-ff)', 'There is no other branch to merge.': '没有其他可合并的分支。',
  'Stash changes': '贮藏更改', 'Set the uncommitted changes aside and restore a clean working copy. Apply the stash later from the sidebar.': '把未提交的更改暂存起来并恢复干净的工作副本。稍后可在侧栏中应用该贮藏。',
  'Include untracked files': '包括未跟踪的文件', 'Keep staged changes': '保留已暂存的更改', 'There are no changes to stash.': '没有可贮藏的更改。', 'There are no changes to discard.': '没有可丢弃的更改。',
  'Discard all changes': '丢弃所有更改', 'Reset every tracked file to the last commit, staged and unstaged changes alike. This can\'t be undone.': '把所有已跟踪的文件重置到上一次提交，已暂存和未暂存的更改都会丢失。此操作无法撤销。',
  'Discard all': '全部丢弃', 'Check out': '检出', 'Check out commit': '检出提交', 'Check out remote branch': '检出远程分支', 'Local branch name': '本地分支名称', 'Check out as local branch…': '检出为本地分支…',
  'New branch from here…': '从这里新建分支…', 'Rename…': '重命名…', 'Copy name': '复制名称', 'Delete…': '删除…', 'Push to remote…': '推送到远程…', 'Rename branch': '重命名分支', 'New name': '新名称',
  'Delete branch': '删除分支', 'Delete even if it has commits that aren\'t merged anywhere': '即使有未合并的提交也删除', 'Delete remote branch': '删除远程分支', 'Delete tag': '删除标签', 'Delete stash': '删除贮藏',
  'Apply': '应用', 'Pop (apply and delete)': '弹出（应用并删除）', 'Check out this commit…': '检出此提交…', 'New branch here…': '在此新建分支…', 'New tag here…': '在此新建标签…',
  'Revert this commit…': '还原此提交…', 'Revert': '还原', 'Cherry-pick': '拣选', 'Rebase': '变基', 'Copy commit hash': '复制提交哈希', 'Copy message': '复制信息',
  'Mode': '模式', 'Soft: keep all changes staged': '软重置：保留所有更改为已暂存', 'Mixed: keep changes in the working copy': '混合重置：在工作副本中保留更改', 'Hard: discard all changes': '硬重置：丢弃所有更改',
  'Reset': '重置', 'Hard reset': '硬重置', 'A hard reset deletes every uncommitted change. Continue?': '硬重置会删除所有未提交的更改。要继续吗？', 'Continue': '继续', 'Abort': '中止',
  'HEAD is detached: check out a branch to push.': 'HEAD 处于分离状态：请先检出分支再推送。', 'This repository has no remote to push to. Add one with git remote add.': '此仓库没有可推送的远程。请用 git remote add 添加。',
  'This repository has no remotes to fetch from.': '此仓库没有可获取的远程。', 'Check out a branch before rebasing.': '请先检出分支再变基。', 'Remote': '远程',
  'That commit is older than the commits loaded. Use "Load more" at the end of the history.': '该提交早于已加载的提交。请使用历史末尾的"加载更多"。', 'Dismiss': '关闭',
  // ---------- hosts ----------
  'Hosts': '主机', 'SSH hosts the agents can use through crew-ssh': '智能体可通过 crew-ssh 使用的 SSH 主机', 'Add SSH host': '添加 SSH 主机', 'Add host…': '添加主机…',
  'Import from ~/.ssh/config': '从 ~/.ssh/config 导入', 'Teach Claude Code and Codex to use crew-ssh': '教 Claude Code 和 Codex 使用 crew-ssh', 'How agents use these hosts': '智能体如何使用这些主机',
  'Add a server here and the agents can manage it with crew-ssh. ': '在这里添加服务器，智能体就能用 crew-ssh 管理它。', 'Add a host': '添加主机', 'import ~/.ssh/config': '导入 ~/.ssh/config', 'Terminal': '终端',
  'Name': '名称', 'Group': '分组', 'Address': '地址', 'Port': '端口', 'User': '用户', 'Sign in with': '登录方式', 'Automatic (key, ssh-agent or saved password)': '自动（密钥、ssh-agent 或已保存的密码）',
  'Private key file': '私钥文件', 'ssh-agent': 'ssh-agent', 'Password': '密码', 'Key file': '密钥文件', 'Key passphrase': '密钥口令', 'Browse…': '浏览…', 'Agents': '智能体',
  'Ask before anything that could change the host (recommended)': '可能改动主机的操作需先询问（推荐）', 'Ask before every command': '每条命令都询问', 'Run everything without asking': '全部直接运行，不询问',
  'Notes for agents': '给智能体的备注', 'What you and the agents call it': '你和智能体对它的称呼', 'Optional, e.g. production': '可选，例如 production', 'e.g. web-1': '例如 web-1',
  'example.com or 203.0.113.10': 'example.com 或 203.0.113.10', 'e.g. root or ubuntu': '例如 root 或 ubuntu', 'Default: ~/.ssh/id_ed25519, id_ecdsa or id_rsa': '默认：~/.ssh/id_ed25519、id_ecdsa 或 id_rsa',
  'Only if the key has one': '仅当密钥设有口令时', 'Saved (type to replace)': '已保存（输入以替换）', 'Leave empty to be asked when needed': '留空则在需要时询问',
  'Forget the saved password': '忘记已保存的密码', 'Forget the saved passphrase': '忘记已保存的口令', 'Will be forgotten when you save.': '保存后将被忘记。',
  'Saved encrypted with Windows data protection.': '使用 Windows 数据保护加密保存。', "This system can't encrypt it, so it isn't saved.": '此系统无法加密，因此不会保存。',
  'Instructions for agents on this host, e.g. "Production: never restart the database. The app lives in /srv/app and runs under pm2."': '给智能体的关于此主机的说明，例如"生产环境：不要重启数据库。应用位于 /srv/app，由 pm2 运行。"',
  'Recent agent commands': '最近的智能体命令', 'Test connection': '测试连接', 'Edit': '编辑', 'Disconnect': '断开连接', 'Open terminal and agent': '打开终端和智能体',
  'New agent session on this host': '在此主机上新建智能体会话', 'Connected': '已连接', 'denied': '已拒绝', 'failed': '失败', 'SSH terminal': 'SSH 终端',
  'Trust and connect': '信任并连接', 'Keep blocking': '继续阻止', 'Replace the key and connect': '替换密钥并连接', 'Trusted before': '之前信任的', 'Now': '现在',
  'On the server: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub': '在服务器上运行：ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub', 'Remember it (saved encrypted)': '记住（加密保存）',
  'Connect': '连接', 'Key passphrase ': '密钥口令', 'An agent is connecting to this host. The password stays in Crew.': '有智能体正在连接此主机。密码只保存在 Crew 中。',
  'Deny': '拒绝', 'Allow 15 min': '允许 15 分钟', 'Allow': '允许', 'Run everything the agents send to this host for the next 15 minutes': '接下来 15 分钟内放行智能体发往此主机的所有操作', 'An agent': '一个智能体',
  'Its saved password and settings are removed from Crew. The server itself isn\'t touched.': '它保存的密码和设置会从 Crew 中移除，服务器本身不受影响。',
  'No ~/.ssh/config file was found.': '没有找到 ~/.ssh/config 文件。', 'Every host in ~/.ssh/config is already here.': '~/.ssh/config 中的主机都已在这里。', "Couldn't write the skill files.": '无法写入技能文件。', 'OK': '确定',
  'Sessions started in Crew have a crew-ssh command. Ask the agent things like "check the disk on web-1" or "restart nginx on the staging servers" and it runs crew-ssh with the host\'s name.': '在 Crew 中启动的会话可以使用 crew-ssh 命令。对智能体说"看看 web-1 的磁盘"或"重启 staging 服务器上的 nginx"，它就会按主机名运行 crew-ssh。',
  "Crew signs in with the host's saved key, ssh-agent or password. The agent never sees credentials.": 'Crew 使用主机保存的密钥、ssh-agent 或密码登录，智能体永远看不到凭据。',
  'Commands that only read (df, ps, systemctl status, journalctl…) run at once. Anything that could change something waits for you to approve it here. Change this per host.': '只读命令（df、ps、systemctl status、journalctl 等）直接运行。任何可能改动的操作都会等你在这里批准。可按主机修改。',
  'The first connection to a host shows its key fingerprint to trust. If the key later changes, Crew blocks the connection.': '首次连接主机时会显示其密钥指纹供你确认；之后若密钥变化，Crew 会阻止连接。',
  'Use "Teach Claude Code and Codex to use crew-ssh" once so the agents know the command without being told.': '使用一次"教 Claude Code 和 Codex 使用 crew-ssh"，智能体无需提示就知道该命令。',
  // ---------- agents ----------
  'Agents your sessions can @-mention to hand them work': '会话中可以 @ 提及并交派任务的智能体', 'Add agent': '添加智能体', 'Add an agent': '添加智能体',
  'Add agents, then @-mention them in a session: "build this, then have @CodeReviewerAgent check it". ': '添加智能体后，在会话中 @ 提及它们："完成这个，然后让 @CodeReviewerAgent 检查"。',
  'Code reviewer': '代码审查', 'Tester': '测试', 'Publisher': '发布', 'Blank agent': '空白智能体', 'Runs on': '运行于', 'Claude Code': 'Claude Code', 'Codex': 'Codex', 'Can': '权限',
  'Reasoning effort': '推理强度', 'Instructions': '说明', 'What you @-mention': '你 @ 提及的名称', 'e.g. CodeReviewerAgent (no spaces)': '例如 CodeReviewerAgent（不含空格）',
  'Only read': '只读', 'Edit files and run commands in the project': '可在项目中编辑文件并运行命令', 'Anything, without asking': '可执行任何操作，不询问',
  'It can look but not change anything.': '它只能查看，不能改动任何内容。', "Codex's sandbox keeps it to the project folder.": 'Codex 的沙箱会把它限制在项目文件夹内。',
  "Claude's auto mode lets it act, and blocks risky actions.": 'Claude 的自动模式允许它操作，并会拦截危险操作。',
  'It runs any command and changes anything without asking you, including outside the project. Give this only to agents you trust with that, like a publisher.': '它可以不经询问运行任何命令、改动任何内容，包括项目以外。只把这个权限给你信任的智能体，例如发布者。',
  'What this agent does and how, e.g. "Review the code for bugs and security problems. Don\'t change files; report findings."': '这个智能体做什么、怎么做，例如"审查代码中的错误和安全问题。不要修改文件，报告发现。"',
  'Installed': '已安装', 'Checking…': '正在检查…', 'Loading models…': '正在加载模型…', 'Each one is the latest version of that model.': '每一项都是该模型的最新版本。',
  "Couldn't load Codex's models; it uses its default.": '无法加载 Codex 的模型，将使用其默认模型。', 'Default (your Claude Code model)': '默认（你的 Claude Code 模型）',
  'Default (your Codex model)': '默认（你的 Codex 模型）', 'Default (your Claude Code setting)': '默认（你的 Claude Code 设置）', "Default (the model's)": '默认（该模型的默认值）',
  'Sessions can no longer @-mention it. Its past runs are kept as sessions.': '会话将无法再 @ 提及它。它过去的运行仍保留为会话。', 'Stop': '停止', 'Result': '结果', 'Details': '详情',
  'Continue this run as a normal session': '把这次运行作为普通会话继续', '(no answer)': '（没有回答）', 'Done': '完成', 'Failed': '失败', 'Stopped': '已停止', 'Starting…': '正在启动…', 'Thinking…': '思考中…',
  'reads': '只读', 'edits': '可编辑', 'full access': '完全权限',
  'Review the code for bugs, security problems, missing error handling and unclear code. Don\'t change files. Report findings by severity, each with the file, line and a suggested fix. Say clearly if you found nothing serious.': '审查代码中的错误、安全问题、缺失的错误处理和不清晰的代码。不要修改文件。按严重程度报告发现，每条附上文件、行号和修改建议。如果没有发现严重问题，请明确说明。',
  "Run the project's tests and linters. Add tests for anything important that has none. Report what passed, what failed and why.": '运行项目的测试和代码检查。为没有测试的重要部分补充测试。报告哪些通过、哪些失败以及原因。',
  'Publish the project: build it, commit and push the changes, tag a release or deploy as the project does it (read its README and scripts first). Stop and report instead of guessing when credentials or a step are missing.': '发布项目：按项目的方式构建、提交并推送更改、打标签发布或部署（先阅读其 README 和脚本）。缺少凭据或步骤时停止并报告，不要猜测。',
  ' review it." The main agent hands it the work in the same folder, one after another or several at once, and gets its answer back.': ' 审查。"主智能体会在同一文件夹中把任务交给它，可依次或同时执行，并拿回它的回答。',
  'Mention it in a session, e.g. "Add a pricing page, then have @': '在会话中提及它，例如"添加一个价格页面，然后让 @',
};

// Strings with names or numbers in them. Each returns the translation, or null to keep the text.
const T = (s) => ZH[s] ?? ZH[s.trim()] ?? zhPattern(s.trim()) ?? s;
const ZH_PATTERNS = [
  [/^New (Claude|Codex) session$/, (m) => `新建 ${m[1]} 会话`],
  [/^New (Claude|Codex) session on (.+)$/, (m) => `在 ${m[2]} 上新建 ${m[1]} 会话`],
  [/^New (.+) session$/, (m) => `新建 ${m[1]} 会话`],
  [/^(OpenAI Chat Completions|OpenAI Responses|Anthropic Messages) · (\d+) models?$/, (m) => `${m[1]} · ${m[2]} 个模型`],
  [/^(OpenAI Chat Completions|OpenAI Responses|Anthropic Messages) · no models$/, (m) => `${m[1]} · 无模型`],
  [/^Found (\d+) models \((\d+) new\)\.$/, (m) => `找到 ${m[1]} 个模型（${m[2]} 个新模型）。`],
  [/^(\d+) images? attached$/, (m) => `已附加 ${m[1]} 张图片`],
  [/^Couldn't fetch the models: (.*)$/, (m) => `无法获取模型：${m[1]}`],
  [/^Added (.+)\.$/, (m) => `已添加 ${m[1]}。`],
  [/^Save changes to (.+)\?$/, (m) => `保存对 ${m[1]} 的更改？`],
  [/^Saved (.+)$/, (m) => `已保存 ${m[1]}`],
  [/^Couldn't save (.+?): (.*)$/, (m) => `无法保存 ${m[1]}：${m[2]}`],
  [/^Codex doesn't offer "(.+)" here$/, (m) => `Codex 在这里不提供“${m[1]}”`],
  [/^Ask (.+) to do anything$/, (m) => `让 ${m[1]} 做任何事`],
  [/^Model (\d+): the model ID can't be empty\.$/, (m) => `模型 ${m[1]}：模型 ID 不能为空。`],
  [/^(.+) is in the list twice\.$/, (m) => `${m[1]} 在列表中出现了两次。`],
  [/^No models are listed for (.+): add them in Settings → Model providers$/, (m) => `${m[1]} 没有模型：请在“设置 → 模型提供商”中添加`],
  [/^Start (Claude|Codex)$/, (m) => `启动 ${m[1]}`],
  [/^(Claude|Codex) · (.+)$/, (m) => `${m[1]} · ${m[2]}`],
  [/^Edit (.+)$/, (m) => `编辑 ${m[1]}`],
  [/^Notes for agents: (.*)$/s, (m) => `给智能体的备注：${m[1]}`],
  [/^(@\S+ · (?:Claude|Codex)(?: \(.+?\))?) · (Only read|Edit files and run commands in the project|Anything, without asking)$/, (m) => `${m[1]} · ${ZH[m[2]]}`],
  [/^Delete (.+)\?$/, (m) => `删除 ${m[1]}？`],
  [/^Copy @(.+)$/, (m) => `复制 @${m[1]}`],
  [/^(.+) · terminal$/, (m) => `${m[1]} · 终端`],
  [/^Your terminal on (.+)$/, (m) => `你在 ${m[1]} 上的终端`],
  [/^Connecting to (.+)…$/, (m) => `正在连接 ${m[1]}…`],
  [/^Connected: (.+)$/, (m) => `已连接：${m[1]}`],
  [/^(.+): connected · (.+)$/, (m) => `${m[1]}：已连接 · ${m[2]}`],
  [/^Trust (.+)\?$/, (m) => `信任 ${m[1]}？`],
  [/^(.+)'s key has changed$/, (m) => `${m[1]} 的密钥已变化`],
  [/^Password for (.+)$/, (m) => `${m[1]} 的密码`],
  [/^Sign in to (.+)$/, (m) => `登录 ${m[1]}`],
  [/^For (.+), to connect to (.+)\.$/, (m) => `用于 ${m[1]}，以连接 ${m[2]}。`],
  [/^This is the first connection to (.+)\. Check that its (\S+) key fingerprint matches the server's:$/, (m) => `这是首次连接 ${m[1]}。请确认其 ${m[2]} 密钥指纹与服务器一致：`],
  [/^The (\S+) key of (.+) isn't the one you trusted before\. (.*)$/, (m) => `${m[2]} 的 ${m[1]} 密钥与之前信任的不同。服务器重装时会这样，但也可能有人在拦截连接。Crew 已阻止连接。`],
  [/^While a host's terminal is open, the agent shares it with you: (.*)$/, () => '主机的终端打开时，智能体与你共用这个终端：它能读取屏幕内容，也能在其中输入（命令、对提示的回答、Ctrl+C 等按键），它输入的一切你都能看到。终端里要求输入的密码和验证码由你自己输入。'],
  [/^(.+) wants to (run on|upload a file to|download a file from|type into your terminal on) $/, (m) => `${m[1]} 想要${{ 'run on': '在以下主机上运行', 'upload a file to': '上传文件到', 'download a file from': '从以下主机下载文件', 'type into your terminal on': '在你的终端中输入，主机' }[m[2]]} `],
  [/^ wants to (run on|upload a file to|download a file from|type into your terminal on) $/, (m) => ` 想要${{ 'run on': '在以下主机上运行：', 'upload a file to': '上传文件到：', 'download a file from': '从以下主机下载文件：', 'type into your terminal on': '在你的终端中输入，主机：' }[m[1]]} `],
  [/^in (.+)$/, (m) => `位于 ${m[1]}`],
  [/^exit (\d+)( · approved)?$/, (m) => `退出码 ${m[1]}${m[2] ? ' · 已批准' : ''}`],
  [/^Imported (\d+) hosts? from ~\/\.ssh\/config\.$/, (m) => `已从 ~/.ssh/config 导入 ${m[1]} 台主机。`],
  [/^Installed the crew-ssh skill for (.+)\. New sessions pick it up\.$/, (m) => `已为 ${m[1].replace(' and ', ' 和 ')} 安装 crew-ssh 技能。新会话会自动加载。`],
  [/^Not installed\. Install it with: (.+)$/, (m) => `未安装。安装命令：${m[1]}`],
  [/^(Claude Code|Codex) \(not installed\)$/, (m) => `${m[1]}（未安装）`],
  [/^Default \((.+)\)$/, (m) => `默认（${T(m[1])}）`],
  [/^(.+) \(saved\)$/, (m) => `${m[1]}（已保存）`],
  [/^(.+) \(loading…\)$/, (m) => `${m[1]}（加载中…）`],
  [/^for (.+)$/, (m) => `为 ${m[1]}`],
  [/^(.+): (result|done|failed|stopped)$/, (m) => `${m[1]}：${{ result: '结果', done: '完成', failed: '失败', stopped: '已停止' }[m[2]]}`],
  [/^ (Working|Done|Failed|Stopped) · $/, (m) => ` ${T(m[1])} · `],
  [/^Reading (.+)$/, (m) => `正在读取 ${m[1]}`],
  [/^Editing (.+)$/, (m) => `正在编辑 ${m[1]}`],
  [/^Running (.+)$/, (m) => `正在运行 ${m[1]}`],
  [/^Searching (.*)$/, (m) => `正在搜索 ${m[1]}`],
  [/^Looking up (.*)$/, (m) => `正在查找 ${m[1]}`],
  [/^Using (.+)$/, (m) => `正在使用 ${m[1]}`],
  // usage bar
  [/^(5-hour|Weekly|(\d+)-day|(\d+)-hour) limit: (\d+%) used(?:, resets in (.+?) \((.+)\))?$/, (m) => `${{ '5-hour': '5 小时', Weekly: '每周' }[m[1]] || (m[2] ? `${m[2]} 天` : `${m[3]} 小时`)}额度：已用 ${m[4]}${m[5] ? `，${m[5]} 后重置（${m[6]}）` : ''}`],
  [/^Updated (.+) ago$/, (m) => `${m[1]}前更新`],
  [/^(Claude|Codex) \((\w+) plan\)$/, (m) => `${m[1]}（${m[2]} 套餐）`],
  [/^· week (\d+%)$/, (m) => `· 本周 ${m[1]}`],
  // git
  [/^(.+) \((\d+)\)$/, (m) => (ZH[m[1]] ? `${ZH[m[1]]} (${m[2]})` : null)],
  [/^Files \((\d+)\)$/, (m) => `文件 (${m[1]})`],
  [/^(Modified|Added|Deleted|Renamed|Copied|Type changed|Conflicted|Untracked): (.+)$/, (m) => `${ZH[m[1]] || ZH[`${m[1]} `]}：${m[2]}`],
  [/^ · (\d+) files?$/, (m) => ` · ${m[1]} 个文件`],
  [/^ · no commits yet$/, () => ' · 还没有提交'],
  [/^(Stage|Unstage|Discard) (\d+) files$/, (m) => `${T(m[1])} ${m[2]} 个文件`],
  [/^Showing the latest (\d+) commits\. Load more$/, (m) => `显示最近 ${m[1]} 个提交。加载更多`],
  [/^Check out (.+)$/, (m) => `检出 ${m[1]}`],
  [/^Merge (.+) into (.+)$/, (m) => `把 ${m[1]} 合并到 ${m[2]}`],
  [/^Rebase (.+) onto (.+)$/, (m) => `把 ${m[1]} 变基到 ${m[2]}`],
  [/^Merge into (.+)$/, (m) => `合并到 ${m[1]}`],
  [/^Cherry-pick onto (.+)$/, (m) => `拣选到 ${m[1]}`],
  [/^Reset (.+) to this commit…$/, (m) => `把 ${m[1]} 重置到此提交…`],
  [/^Push to (.+)$/, (m) => `推送到 ${m[1]}`],
  [/^Fetch (.+)$/, (m) => `获取 ${m[1]}`],
  [/^Abort (merge|rebase|cherry-pick|revert)$/, (m) => `中止${{ merge: '合并', rebase: '变基', 'cherry-pick': '拣选', revert: '还原' }[m[1]]}`],
  [/^A (merge|rebase|cherry-pick|revert) is in progress\. .*$/, (m) => `${{ merge: '合并', rebase: '变基', 'cherry-pick': '拣选', revert: '还原' }[m[1]]}正在进行。请解决冲突并暂存文件，然后${m[1] === 'merge' ? '提交' : '继续'}。`],
  [/^Ignore everything in (.+)$/, (m) => `忽略 ${m[1]} 中的所有内容`],
  [/^Added (.+) to \.gitignore\.$/, (m) => `已把 ${m[1]} 加入 .gitignore。`],
  [/^(.+) isn't managed by Git yet\. Create a repository here with git init\?$/, (m) => `${m[1]} 还没有使用 Git。要在这里用 git init 创建仓库吗？`],
  [/^Delete the local branch (.+)\?$/, (m) => `删除本地分支 ${m[1]}？`],
  [/^Delete the local tag (.+)\?$/, (m) => `删除本地标签 ${m[1]}？`],
  [/^Tag commit (.+)\.$/, (m) => `为提交 ${m[1]} 打标签。`],
  [/^Start the branch at commit (.+)\.$/, (m) => `从提交 ${m[1]} 开始新分支。`],
  [/^Start the branch at the current (.+)\.$/, (m) => `从当前${m[1].replace(/^branch \((.+)\)$/, '分支（$1）').replace(/^commit$/, '提交')}开始新分支。`],
  [/^Merge a branch into (.+)\.$/, (m) => `把一个分支合并到 ${m[1]}。`],
  [/^Push the tag to (.+)$/, (m) => `把标签推送到 ${m[1]}`],
  [/^Also delete (\d+) untracked files?$/, (m) => `同时删除 ${m[1]} 个未跟踪的文件`],
  [/^Discard the unstaged changes to (.+)\?(.*)$/, (m) => `要丢弃 ${m[1]} 的未暂存更改吗？此操作无法撤销。`],
  [/^Rebase (.+) onto (.+)\? .*$/, (m) => `把 ${m[1]} 变基到 ${m[2]}？它的提交会在 ${m[2]} 之上重放并被改写。`],
  [/^Reset (.+)$/, (m) => `重置 ${m[1]}`],
  // the git status bar: "Push: done. …", "Pushing…", "Push failed. …"
  [/^(.+): done\.(.*)$/s, (m) => `${T(m[1])}：完成。${m[2]}`],
  [/^(.+?) failed\. (.*)$/s, (m) => `${T(m[1])}失败。${m[2]}`],
  [/^(Committing|Pushing|Pulling|Fetching|Merging|Stashing|Staging|Unstaging|Discarding|Rebasing|Resetting|Reverting|Applying|Popping|Restoring|Cherry-picking|Checking out|Creating|Deleting|Renaming|Amending|Continuing|Aborting|Connecting)(.*)…$/, (m) => `正在${{ Committing: '提交', Pushing: '推送', Pulling: '拉取', Fetching: '获取', Merging: '合并', Stashing: '贮藏', Staging: '暂存', Unstaging: '取消暂存', Discarding: '丢弃', Rebasing: '变基', Resetting: '重置', Reverting: '还原', Applying: '应用', Popping: '弹出', Restoring: '恢复', 'Cherry-picking': '拣选', 'Checking out': '检出', Creating: '创建', Deleting: '删除', Renaming: '重命名', Amending: '修改', Continuing: '继续', Aborting: '中止', Connecting: '连接' }[m[1]]}${m[2]}…`],
  [/^(Push|Pull|Fetch|Stage|Unstage|Commit|Merge|Stash|Discard all|Discard|Rebase|Revert|Cherry-pick|Amend commit|Create branch|Create tag|Delete|Rename branch|Apply|Pop|Restore file|Stage hunk|Unstage hunk|Discard hunk)(.*)…$/, (m) => `${T(m[1])}${m[2]}…`],
];

function zhPattern(s) {
  for (const [re, fn] of ZH_PATTERNS) {
    const m = re.exec(s);
    if (m) { const r = fn(m); if (r != null) return r; }
  }
  return null;
}

// Where nothing is translated: what the user and the agents wrote, and terminals.
const I18N_SKIP = [
  '.xterm', '.term', 'pre', 'code', 'textarea', 'input', '#head-title', '.git-line', '.git-hunk-head .mono', '.subject', '.git-meta-subject',
  '.git-meta-body', '.git-fname', '.git-fdir', '.git-ref', '.c-author', '.c-hash', '.run-task', '.run-activity', '.run-result', '.ssh-cmd', '.fingerprint',
  '.row-item.session:not(.host-term) .label', '.row-item.project .label', '.host-row .label', '.host-row .meta', '.agent-row .label', '.br-tab .title', '#br-url',
  '.git-nav-item:not(#git-nav > .git-nav-item) .label', '#git-title', '#git-sub', '.mention-item b', '.host-act code', '.prompt-text.muted',
].join(',');

const i18n = {
  lang: 'en',
  // The language Settings asked for; "auto" follows Windows.
  resolve(setting) {
    if (setting === 'zh' || setting === 'en') return setting;
    return /^zh\b/i.test(navigator.language || '') ? 'zh' : 'en';
  },
  t(s) { return this.lang === 'zh' ? T(s) : s; },
};

function i18nText(node) {
  const v = node.nodeValue;
  if (!v || !/[A-Za-z]/.test(v)) return;
  const p = node.parentElement;
  if (!p || p.closest(I18N_SKIP)) return;
  const lead = v.match(/^\s*/)[0];
  const trail = v.match(/\s*$/)[0];
  const core = v.trim();
  let out = ZH[v] ?? ZH[core] ?? zhPattern(core);
  if (out == null && core.includes('\n')) out = core.split('\n').map((l) => T(l)).join('\n');
  if (out != null && out !== core) node.nodeValue = ZH[v] != null ? out : `${lead}${out}${trail}`;
}

function i18nAttrs(el) {
  if (el.closest?.('.xterm, .term')) return;
  for (const a of ['title', 'placeholder', 'aria-label']) {
    const v = el.getAttribute?.(a);
    if (!v || !/[A-Za-z]/.test(v)) continue;
    const out = v.includes('\n') ? v.split('\n').map((l) => T(l)).join('\n') : T(v);
    if (out !== v) el.setAttribute(a, out);
  }
}

function i18nWalk(root) {
  if (root.nodeType === 3) { i18nText(root); return; }
  if (root.nodeType !== 1) return;
  if (root.matches?.('.xterm, .term')) return;
  i18nAttrs(root);
  const w = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.nodeType === 1 && n.matches('.xterm, .term') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  for (let n = w.nextNode(); n; n = w.nextNode()) { if (n.nodeType === 3) i18nText(n); else i18nAttrs(n); }
}

// Until the config arrives, use the language from last time, so the page doesn't flash English.
(() => {
  let saved = 'auto';
  try { saved = localStorage.getItem('language') || 'auto'; } catch { /* default */ }
  i18n.lang = i18n.resolve(saved);
  document.documentElement.lang = i18n.lang === 'zh' ? 'zh-CN' : 'en';
  if (i18n.lang !== 'zh') return;
  const start = () => {
    i18nWalk(document.body);
    new MutationObserver((list) => {
      for (const m of list) {
        if (m.type === 'childList') m.addedNodes.forEach(i18nWalk);
        else if (m.type === 'characterData') i18nText(m.target);
        else if (m.type === 'attributes') i18nAttrs(m.target);
      }
    }).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ['title', 'placeholder', 'aria-label'] });
  };
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
})();

// Called once the config is loaded: a different language than the page started with reloads it.
function applyLanguage(setting) {
  try { localStorage.setItem('language', setting || 'auto'); } catch { /* ignore */ }
  if (i18n.resolve(setting || 'auto') !== i18n.lang) location.reload();
}
