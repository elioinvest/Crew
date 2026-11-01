// Crew on your phone: pairs with Crew desktop (its Settings → Phone access shows a QR code with
// the computer's addresses and a pairing code), then shows the sessions open there as chats,
// streams a terminal's output, types into it, starts or resumes sessions in Crew, and answers
// crew-ssh's approvals. Everything goes over one WebSocket to Crew's remote.js.

const $ = (q) => document.querySelector(q);
function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') e.className = v;
    else if (k === 'html') e.innerHTML = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (v != null) e.setAttribute(k, v);
  }
  for (const c of children) if (c != null && c !== false) e.append(c);
  return e;
}
const Cap = window.Capacitor?.Plugins || {};

// ---------- language ----------

const ZH = {
  'Use Crew from your phone': '在手机上使用 Crew',
  'In Crew on your computer, open Settings → Phone access, turn it on, then scan its QR code.': '在电脑上的 Crew 中打开 设置 → 手机访问，开启后扫描其中的二维码。',
  'Scan QR code': '扫描二维码', 'Connect': '连接', 'Address, like 192.168.1.20:7788': '地址，例如 192.168.1.20:7788', 'Pairing code': '配对码',
  'What are we doing today?': '今天做点什么？', 'New chat': '新会话', 'Agent runs': '智能体运行', 'Computers': '电脑', 'Chats': '会话',
  'Search chats': '搜索会话', 'Working': '工作中', 'Open in Crew': '在 Crew 中打开', 'Recent': '最近', 'open': '打开',
  'Describe the task to start a new chat, / for commands': '描述要做的事来开始新会话，/ 调用指令',
  'Message, or / for commands': '发消息，/ 调用指令', 'Command': '命令',
  'Choose a project': '选择项目', 'Agent': '智能体', 'Commands': '指令', 'No projects yet.': '还没有项目。', 'Nothing matches.': '没有匹配的会话。',
  'No chats yet. Start one with New chat.': '还没有会话。点击“新会话”开始。',
  'Needs your OK': '需要确认', 'Deny': '拒绝', 'Allow 15 min': '允许 15 分钟', 'Allow': '允许',
  'is waiting for you on the computer': '正在电脑上等你处理',
  'Running': '运行中', 'Done': '已完成', 'Failed': '失败', 'Stop': '停止', 'No agent runs yet.': '还没有智能体运行。',
  '{0} runs · {1} running': '{0} 次运行 · {1} 个进行中',
  'Pair another computer': '配对另一台电脑', 'Disconnect': '断开连接', 'Forget': '忘记', 'Connected': '已连接',
  'Connecting to {0}…': '正在连接 {0}…', 'Not connected': '未连接',
  "Couldn't reach {0}. Is Crew running with phone access on, and is this phone on the same Wi-Fi (or Tailscale)?": '无法连接 {0}。请确认 Crew 正在运行并已开启手机访问，且手机与电脑在同一 Wi-Fi（或 Tailscale）下。',
  'Enter the address and the pairing code shown in Crew.': '请输入 Crew 中显示的地址和配对码。',
  'Scan the QR code in Crew with your phone camera, or enter the address and code here.': '用手机相机扫描 Crew 中的二维码，或在这里输入地址和配对码。',
  'Crew needs the camera to scan the code.': 'Crew 需要相机权限来扫码。', "That isn't Crew's pairing code.": '这不是 Crew 的配对码。',
  'Starting a {0} session in Crew…': '正在 Crew 中启动 {0} 会话…', 'Opening it in Crew…': '正在 Crew 中打开…',
  'Finished: {0}': '已完成：{0}', 'Finished': '已完成', 'Sending…': '正在发送…', 'Sending {0} files…': '正在发送 {0} 个文件…',
  'Added. Say what to do with it, then send.': '已添加。说明要怎么处理，然后发送。',
  '{0} is over 20 MB.': '{0} 超过 20 MB。', "Couldn't read {0}.": '无法读取 {0}。', "The computer didn't answer.": '电脑没有响应。',
  '[The session ended in Crew]': '[会话已在 Crew 中结束]', 'Loading…': '加载中…',
  'Switch model': '切换模型', 'Summarize to free context': '压缩上下文', 'Start over': '清空重来', 'Context used': '上下文用量', 'Plan usage': '套餐用量',
  'Resume a session': '恢复会话', 'All commands': '全部指令', 'Model and reasoning': '模型与推理强度', 'Approval mode': '审批模式',
  'New conversation': '新对话', 'Changes so far': '目前的改动', 'Review changes': '审查改动', 'Session status': '会话状态',
  'just now': '刚刚', '{0} min': '{0}分钟', '{0} h': '{0}小时', '{0} d': '{0}天',
  'upload a file to': '上传文件到', 'download a file from': '从其下载文件', 'type into your terminal on': '在你的终端中输入，主机', 'run on': '运行命令，主机',
  'An agent wants to {0} {1}': '智能体想要{0} {1}', 'a host key to trust': '需要信任主机密钥', 'a password or code': '需要密码或验证码',
  'Untitled': '未命名', 'Model': '模型', 'Effort': '推理强度', 'Mode': '模式', 'Permissions': '权限', 'Manual': '手动', 'Accept edits': '自动接受编辑',
  'Plan': '计划', 'Auto': '自动', 'Bypass': '跳过权限', 'Low': '低', 'Medium': '中', 'High': '高', 'Extra high': '超高', 'Max': '最高',
  'Ask for approval': '请求批准', 'Approve for me': '替我批准', 'Full Access': '完全访问', 'Read Only': '只读', 'Refresh model list': '刷新模型列表', 'Older models': '较早的模型', 'Default': '默认',
  "Default (the model's)": '默认（模型自身）', 'Try again when the agent is idle.': '请在智能体空闲时再试。', 'Switching…': '正在切换…', 'now': '刚刚', '{0}m': '{0}分钟', '{0}h': '{0}小时', '{0}d': '{0}天', 'Show {0} more': '再显示 {0} 个',
  'No sessions yet': '还没有会话', 'No matches': '没有匹配项', 'No projects yet': '还没有项目', 'Hosts': '主机', 'Agents': '智能体', 'Terminal': '终端', 'reads': '只读', 'edits': '可编辑', 'full access': '完全访问',
  'Add hosts in Crew on your computer.': '在电脑上的 Crew 中添加主机。', 'Add agents in Crew on your computer, then @-mention them in a chat.': '在电脑上的 Crew 中添加智能体，然后在会话中 @ 它们。', 'Search projects and sessions': '搜索项目和会话', 'New session': '新会话', 'Projects': '项目',
};
let lang = (() => { try { return localStorage.getItem('lang'); } catch { return null; } })() || (/^zh/i.test(navigator.language) ? 'zh' : 'en');
function T(s, ...args) {
  const out = lang === 'zh' ? ZH[s] ?? s : s;
  return out.replace(/\{(\d)\}/g, (_, i) => args[+i] ?? '');
}
function applyStatic() {
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
  for (const e of document.querySelectorAll('[data-t]')) e.textContent = T(e.dataset.t);
  for (const e of document.querySelectorAll('[data-tp]')) e.placeholder = T(e.dataset.tp);
  $('#d-lang').textContent = lang === 'zh' ? 'English' : '中文';
}

// ---------- icons ----------

const ICON = {
  caret: '<svg class="caret" viewBox="0 0 24 24"><path d="M6 9l6 6 6-6"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/></svg>',
  slash: '<svg viewBox="0 0 24 24"><path d="M15 4L9 20"/></svg>',
  shell: '<svg viewBox="0 0 24 24"><path d="M5 8l4 4-4 4M12 17h7"/></svg>',
  check: '<svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  up: '<svg viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6"/></svg>',
  stop: '<svg viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="14" rx="2.5"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
  monitor: '<svg viewBox="0 0 24 24"><rect x="3.5" y="4.5" width="17" height="11.5" rx="2"/><path d="M8.5 20h7M12 16v4"/></svg>',
};
const AGENT_NAME = { claude: 'Claude', codex: 'Codex', ssh: 'SSH' };
const MARK = { claude: ['C', ''], codex: ['G', 'codex'], ssh: ['>_', 'ssh'] };
function mark(agent, provider) {
  const [t, cls] = provider ? ['P', 'provider'] : MARK[agent] || ['?', ''];
  return el('span', { class: `mark ${cls}` }, t);
}
function ago(ms) {
  if (!ms) return '';
  if (ms < 1e12) ms *= 1000;
  const m = Math.max(0, Math.round((Date.now() - ms) / 60000));
  if (m < 1) return T('just now');
  if (m < 60) return T('{0} min', m);
  if (m < 60 * 24) return T('{0} h', Math.round(m / 60));
  return T('{0} d', Math.round(m / 1440));
}

// ---------- saved computers ----------

// { name, hosts: [address], port, token }, the last used first.
const store = {
  get: () => { try { return JSON.parse(localStorage.getItem('computers') || '[]'); } catch { return []; } },
  set: (list) => { try { localStorage.setItem('computers', JSON.stringify(list)); } catch { /* private mode */ } },
};
function remember(c) {
  const list = store.get().filter((x) => x.token !== c.token);
  store.set([c, ...list].slice(0, 6));
}

// crew://pair?d=… or http://<computer>:7788/?d=… with base64url JSON { v, name, port, token, hosts },
// as Crew's QR code carries it.
function parsePairing(text) {
  const m = /[?&]d=([A-Za-z0-9_-]+)/.exec(String(text));
  if (!m) return null;
  try {
    const json = JSON.parse(atob(m[1].replace(/-/g, '+').replace(/_/g, '/')));
    if (!json.token || !json.hosts?.length) return null;
    return { name: json.name || 'Computer', hosts: json.hosts, port: json.port || 7788, token: json.token };
  } catch { return null; }
}

// ---------- the connection ----------

const conn = { ws: null, computer: null, host: null, ready: false, retry: 0, timer: null, wanted: false, name: '' };
const app = {
  terminals: [], projects: [], prompts: new Map(), runs: new Map(),
  view: 'welcome', current: null, // the open chat's terminal
  project: null, agent: 'claude', // a new chat's choices
  pending: null, // a new chat's first message, sent once Crew has started it
  chips: new Map(), // session id -> { model, effort, mode }, as Crew's composer shows them
};

function setStatus(text, error = false) {
  const s = $('#connect-status');
  s.textContent = text;
  s.classList.toggle('error', error);
}

// Tries the computer's addresses in turn (the Wi-Fi one first, then Tailscale's) until one answers.
async function connect(computer) {
  conn.computer = computer;
  conn.wanted = true;
  clearTimeout(conn.timer);
  setDot('wait');
  for (const host of computer.hosts) {
    setStatus(T('Connecting to {0}…', `${computer.name} (${host})`));
    const ws = await open(`ws://${host}:${computer.port}`);
    if (!ws) continue;
    conn.host = host;
    attachSocket(ws);
    return;
  }
  setDot('');
  setStatus(T("Couldn't reach {0}. Is Crew running with phone access on, and is this phone on the same Wi-Fi (or Tailscale)?", computer.name), true);
  scheduleRetry();
}

function open(url) {
  return new Promise((resolve) => {
    let ws;
    try { ws = new WebSocket(url); } catch { resolve(null); return; }
    const t = setTimeout(() => { try { ws.close(); } catch { /* */ } resolve(null); }, 3500);
    ws.onopen = () => { clearTimeout(t); resolve(ws); };
    ws.onerror = () => { clearTimeout(t); resolve(null); };
  });
}

function attachSocket(ws) {
  conn.ws = ws;
  ws.onmessage = (e) => { let m; try { m = JSON.parse(e.data); } catch { return; } onMessage(m); };
  ws.onclose = () => {
    conn.ready = false;
    conn.ws = null;
    setDot('');
    if (conn.wanted) scheduleRetry();
  };
  send({ t: 'hello', token: conn.computer.token, device: deviceName() });
}

function scheduleRetry() {
  if (!conn.wanted) return;
  clearTimeout(conn.timer);
  const delay = Math.min(15000, 1000 * 2 ** conn.retry++);
  conn.timer = setTimeout(() => connect(conn.computer), delay);
}

function send(m) { if (conn.ws?.readyState === 1) conn.ws.send(JSON.stringify(m)); }
const deviceName = () => (/iPhone|iPad/i.test(navigator.userAgent) ? 'iPhone' : /Android/i.test(navigator.userAgent) ? 'Android phone' : 'Phone');
function setDot(kind) {
  $('#conn-dot').className = `dot${kind === 'on' ? ' on' : kind === 'wait' ? ' wait' : ''}`;
  $('#conn-name').textContent = kind === 'on' ? conn.name : T('Not connected');
}

function onMessage(m) {
  switch (m.t) {
    case 'denied':
      conn.wanted = false;
      setStatus(m.reason, true);
      showScreen('connect');
      break;
    case 'ready':
      conn.ready = true;
      conn.retry = 0;
      conn.name = m.computer || conn.computer.name;
      setDot('on');
      remember({ ...conn.computer, name: conn.name, hosts: [conn.host, ...conn.computer.hosts.filter((h) => h !== conn.host)] });
      $('#computer-tag').textContent = conn.name;
      showScreen('main');
      if (app.current) send({ t: 'attach', termId: app.current.termId }); // back after a dropped connection
      break;
    case 'state':
      app.terminals = m.terminals;
      app.projects = m.projects;
      app.providers = m.providers || [];
      app.hosts = m.hosts || [];
      app.agents = m.agents || [];
      app.prompts = new Map(m.prompts.map((p) => [p.id, p]));
      app.runs = new Map(m.runs.map((r) => [r.id, r]));
      if (app.current) app.current = app.terminals.find((t) => t.termId === app.current.termId) || app.current;
      if (!app.project || !app.projects.some((p) => p.path === app.project)) app.project = app.projects[0]?.path || null;
      checkPending();
      renderAll();
      break;
    case 'snapshot': onSnapshot(m); break;
    case 'out': if (app.current?.termId === m.termId) term.write(m.data); break;
    case 'exit':
      if (app.current?.termId === m.termId) term.write(`\r\n\x1b[90m${T('[The session ended in Crew]')}\x1b[0m\r\n`);
      break;
    case 'prompt':
      app.prompts.set(m.prompt.id, m.prompt);
      renderPrompts();
      notify(T('Needs your OK'), m.prompt.kind === 'approve' ? `${m.prompt.host}: ${m.prompt.command}` : m.prompt.host);
      break;
    case 'prompt-done': app.prompts.delete(m.id); renderPrompts(); break;
    case 'run': app.runs.set(m.run.id, m.run); renderRuns(); renderDrawer(); break;
    case 'activity': onActivity(m); break;
    case 'chips':
      if (m.chips) app.chips.set(m.sessionId, m.chips); else app.chips.delete(m.sessionId);
      if (app.current?.sessionId === m.sessionId) renderComposer();
      break;
    case 'chip-menu': chipMenus.get(m.reqId)?.(m.menu); chipMenus.delete(m.reqId); break;
    case 'uploaded': uploads.get(m.reqId)?.(m); uploads.delete(m.reqId); break;
    default: break;
  }
}

// ---------- screens and views ----------

function showScreen(name) {
  $('#screen-connect').classList.toggle('hidden', name !== 'connect');
  $('#screen-main').classList.toggle('hidden', name !== 'main');
  if (name === 'connect') { closeDrawer(); renderSaved(); }
  if (name === 'main') renderAll();
}

// welcome: a new chat · term: an open chat · runs: the agent runs
function setView(view) {
  if (view !== 'term' && app.current) { send({ t: 'detach', termId: app.current.termId }); app.current = null; }
  app.view = view;
  $('#welcome').classList.toggle('hidden', view !== 'welcome');
  $('#term-view').classList.toggle('hidden', view !== 'term');
  $('#runs-view').classList.toggle('hidden', view !== 'runs');
  $('#screen-main').classList.toggle('welcome-mode', view === 'welcome');
  $('#composer').classList.toggle('hidden', view === 'runs');
  $('#btn-keys').classList.toggle('hidden', view !== 'term');
  if (view !== 'term') showKeys(false);
  renderAll();
}

function renderAll() {
  renderHeader();
  renderComposer();
  renderPrompts();
  renderDrawer();
  if (app.view === 'runs') renderRuns();
}

function renderHeader() {
  $('#title').textContent = app.view === 'term' ? app.current?.title || '' : app.view === 'runs' ? T('Agent runs') : '';
}

// The chips under the message box: for a new chat, the project and the agent; in a chat, the
// agent's commands and its name (tap it to switch model).
function chip(btn, html, cls = '') {
  btn.className = `chip ${cls}`.trim();
  btn.innerHTML = html;
}
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
// The chips under the box, as under Crew's: for a new chat, the project and the agent; in a
// chat, the session's model, reasoning effort and mode, each opening Crew's own menu for it.
function chipBtn(html, onclick, cls = '') {
  return el('button', { class: `chip ${cls}`.trim(), html, onclick });
}
function renderComposer() {
  const t = app.current;
  const box = $('#in-text');
  const chips = [];
  if (app.view === 'welcome') {
    const p = app.projects.find((x) => x.path === app.project);
    box.placeholder = T('Describe the task to start a new chat, / for commands');
    chips.push(chipBtn(`${ICON.folder}<span class="label">${esc(p?.name || T('Choose a project'))}</span>${ICON.caret}`, pickProject));
    chips.push(chipBtn(`<span class="label">${AGENT_NAME[app.agent]}</span>${ICON.caret}`, pickAgent, 'model'));
    $('#btn-attach').classList.remove('hidden');
  } else if (t) {
    const ssh = t.agent === 'ssh';
    box.placeholder = ssh ? T('Command') : T('Message, or / for commands');
    if (ssh) chips.push(chipBtn(`${ICON.shell}<span class="label">${esc(t.project || 'SSH')}</span>`, () => {}));
    else {
      const c = app.chips.get(t.sessionId);
      if (c) {
        chips.push(chipBtn(`<span class="label">${esc(T(c.model))}</span>${ICON.caret}`, () => openChip('model'), 'model'));
        if (c.effort) chips.push(chipBtn(`<span class="label">${esc(T(c.effort))}</span>${ICON.caret}`, () => openChip('effort'), 'sub'));
        chips.push(chipBtn(`<span class="label">${esc(T(c.mode))}</span>${ICON.caret}`, () => openChip('mode'), 'sub'));
      } else {
        chips.push(chipBtn(`<span class="label">${AGENT_NAME[t.agent] || t.agent}</span>`, () => send({ t: 'chips', sessionId: t.sessionId }), 'model'));
      }
      if (COMMANDS[t.agent]) chips.push(chipBtn(ICON.slash, pickCommand, 'icon'));
    }
    $('#btn-attach').classList.toggle('hidden', ssh);
  }
  $('#chips').replaceChildren(...chips);
  $('#busy').classList.toggle('hidden', !(app.view === 'term' && t?.busy));
  renderSend();
}
function renderSend() {
  const b = $('#btn-send');
  const typed = !!$('#in-text').value.trim();
  const stop = !typed && app.view === 'term' && app.current?.busy;
  b.className = `send${stop ? ' stop' : typed ? ' ready' : ''}`;
  b.innerHTML = stop ? ICON.stop : ICON.up;
  b.setAttribute('aria-label', stop ? T('Stop') : 'Send');
}

// ---------- the drawer: new chat, runs, computers, and the chats ----------

function openDrawer() { $('#drawer').classList.add('open'); $('#scrim').classList.remove('hidden'); renderDrawer(); }
function closeDrawer() { $('#drawer').classList.remove('open'); $('#scrim').classList.add('hidden'); }

// The chats, laid out like Crew's own sidebar: each project folds open to its sessions, grouped by
// agent (Claude, GPT, then each custom provider), with a dot or a spinner on the ones open in
// Crew and how long ago each was used; the hosts' terminals come after.
const SESSIONS_SHOWN = 8;
const BADGE = {
  claude: '<svg class="agent-badge claude" viewBox="0 0 24 24"><path d="M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6 5.6 18.4"/></svg>',
  codex: '<svg class="agent-badge codex" viewBox="0 0 24 24"><path d="M22.28 9.82a5.98 5.98 0 0 0-.52-4.91 6.05 6.05 0 0 0-6.51-2.9A6.07 6.07 0 0 0 4.98 4.18a5.98 5.98 0 0 0-4 2.9 6.05 6.05 0 0 0 .75 7.1 5.98 5.98 0 0 0 .51 4.91 6.05 6.05 0 0 0 6.51 2.9A5.98 5.98 0 0 0 13.26 24a6.06 6.06 0 0 0 5.77-4.21 5.99 5.99 0 0 0 4-2.9 6.06 6.06 0 0 0-.75-7.07zm-9.02 12.61a4.48 4.48 0 0 1-2.88-1.04l.14-.08 4.78-2.76a.79.79 0 0 0 .39-.68v-6.74l2.02 1.17a.07.07 0 0 1 .04.05v5.58a4.5 4.5 0 0 1-4.49 4.5zm-9.66-4.13a4.47 4.47 0 0 1-.53-3.01l.14.09 4.78 2.76a.77.77 0 0 0 .78 0l5.84-3.37v2.33a.08.08 0 0 1-.03.06L9.74 19.95a4.5 4.5 0 0 1-6.14-1.65zM2.34 7.9a4.49 4.49 0 0 1 2.37-1.97v5.67a.77.77 0 0 0 .39.68l5.81 3.35-2.02 1.17a.08.08 0 0 1-.07 0l-4.83-2.79A4.5 4.5 0 0 1 2.34 7.87zm16.6 3.86-5.84-3.39 2.02-1.16a.08.08 0 0 1 .07 0l4.83 2.79a4.49 4.49 0 0 1-.68 8.1v-5.68a.79.79 0 0 0-.4-.67zm2.01-3.02-.14-.09-4.77-2.78a.78.78 0 0 0-.79 0L9.41 9.23V6.9a.07.07 0 0 1 .03-.06l4.83-2.79a4.5 4.5 0 0 1 6.68 4.66zM8.31 12.86l-2.02-1.16a.08.08 0 0 1-.04-.06V6.07a4.5 4.5 0 0 1 7.38-3.45l-.14.08L8.7 5.46a.79.79 0 0 0-.39.68zm1.1-2.37 2.6-1.5 2.61 1.5v3l-2.6 1.5-2.61-1.5z"/></svg>',
  ssh: '<svg class="agent-badge ssh" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M12 15h5"/></svg>',
};
const TREE_ICON = {
  folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  chev: '<svg class="chev" viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
};
const folded = (() => { try { return new Set(JSON.parse(localStorage.getItem('folded') || '[]')); } catch { return new Set(); } })();
const showAll = new Set();
function toggleFold(key) {
  if (folded.has(key)) folded.delete(key); else folded.add(key);
  try { localStorage.setItem('folded', JSON.stringify([...folded])); } catch { /* */ }
  renderDrawer();
}
// Crew's short times: now, 5m, 3h, 2d, then the date.
function relTime(ms) {
  if (!ms) return '';
  if (ms < 1e12) ms *= 1000;
  const sec = (Date.now() - ms) / 1000;
  if (sec < 60) return T('now');
  if (sec < 3600) return T('{0}m', Math.floor(sec / 60));
  if (sec < 86400) return T('{0}h', Math.floor(sec / 3600));
  if (sec < 86400 * 30) return T('{0}d', Math.floor(sec / 86400));
  return new Date(ms).toLocaleDateString();
}
const statusDot = (t) => (t.busy ? el('span', { class: 'spinner' }) : el('span', { class: 'dot-run' }));
function providerBadge(p) {
  return el('span', { class: 'agent-icon' }, p?.icon ? el('span', { class: 'prov-emoji' }, p.icon) : el('span', { class: 'prov-letter' }, (p?.name || '?').slice(0, 1).toUpperCase()));
}

function renderDrawer() {
  const running = [...app.runs.values()].filter((r) => r.status === 'running').length;
  $('#runs-badge').textContent = String(running);
  $('#runs-badge').classList.toggle('hidden', !running);
  if (!$('#drawer').classList.contains('open')) return; // drawn when it opens
  const q = $('#d-search').value.trim().toLowerCase();
  const has = (x) => String(x || '').toLowerCase().includes(q);
  const live = new Map(app.terminals.filter((t) => t.agent !== 'ssh').map((t) => [t.sessionId, t]));
  const out = [];

  for (const p of app.projects) {
    const nameHit = !q || has(p.name) || has(p.path);
    // Sessions open in Crew but not written to disk yet go on top.
    const fresh = [...live.values()].filter((t) => t.cwd && norm(t.cwd) === norm(p.path) && !p.sessions.some((s) => s.id === t.sessionId))
      .map((t) => ({ id: t.sessionId, title: t.title, agent: t.agent, mtime: Date.now(), pending: true }));
    let sessions = [...fresh, ...p.sessions];
    if (q && !nameHit) sessions = sessions.filter((s) => has(s.title));
    if (q && !nameHit && !sessions.length) continue;

    const isFolded = !q && folded.has(p.path);
    out.push(el('div', { class: `row-item project${isFolded ? ' collapsed' : ''}`, onclick: () => toggleFold(p.path) },
      el('span', { html: TREE_ICON.folder }),
      el('span', { class: 'label' }, p.name),
      el('button', { class: 'icon-btn tiny', 'aria-label': 'New session', html: TREE_ICON.plus, onclick: (e) => { e.stopPropagation(); newChatIn(p.path); } }),
      el('span', { html: TREE_ICON.chev })));
    if (isFolded) continue;

    const list = el('div', { class: 'sessions' });
    const groups = [
      { key: 'claude', label: 'Claude', icon: () => el('span', { class: 'agent-icon', html: BADGE.claude }), agent: 'claude', has: (s) => (s.agent || 'claude') === 'claude' && !s.provider },
      { key: 'codex', label: 'GPT', icon: () => el('span', { class: 'agent-icon', html: BADGE.codex }), agent: 'codex', has: (s) => s.agent === 'codex' && !s.provider },
      ...(app.providers || []).map((pv) => ({ key: `provider:${pv.id}`, label: pv.name, icon: () => providerBadge(pv), has: (s) => s.provider === pv.id })),
    ];
    for (const g of groups) {
      const group = sessions.filter(g.has);
      if (!group.length) continue;
      const key = `${p.path}::${g.key}`;
      const gFolded = !q && folded.has(key);
      list.append(el('div', { class: `row-item agent-group${gFolded ? ' collapsed' : ''}`, onclick: () => toggleFold(key) },
        g.icon(), el('span', { class: 'label' }, g.label), el('span', { class: 'meta' }, String(group.length)),
        g.agent ? el('button', { class: 'icon-btn tiny', 'aria-label': 'New session', html: TREE_ICON.plus, onclick: (e) => { e.stopPropagation(); newChatIn(p.path, g.agent); } }) : null,
        el('span', { html: TREE_ICON.chev })));
      if (gFolded) continue;
      const limit = q || showAll.has(key) ? Infinity : SESSIONS_SHOWN;
      for (const sess of group.slice(0, limit)) list.append(sessionItem(p, sess, live.get(sess.id)));
      if (group.length > limit) {
        list.append(el('div', { class: 'row-item session more', onclick: () => { showAll.add(key); renderDrawer(); } }, T('Show {0} more', group.length - limit)));
      }
    }
    if (!list.children.length) list.append(el('div', { class: 'empty-note' }, T('No sessions yet')));
    out.push(list);
  }
  if (!out.length) out.push(el('div', { class: 'empty-note' }, q ? T('No matches') : T('No projects yet')));

  // Hosts, as in Crew: by group, each host with its address, folding open to its terminal and the
  // agent sessions of its workspace.
  const hosts = (app.hosts || []).filter((h) => !q || has(h.name) || has(h.host) || (h.sessions || []).some((s) => has(s.title)));
  const hostsFolded = !q && folded.has('§hosts');
  out.push(sectionHead(T('Hosts'), '§hosts', hostsFolded));
  if (!hostsFolded) {
    if (!(app.hosts || []).length) out.push(el('div', { class: 'empty-note' }, T('Add hosts in Crew on your computer.')));
    const groups = new Map();
    for (const h of [...hosts].sort((a, b) => a.name.localeCompare(b.name))) {
      if (!groups.has(h.group || '')) groups.set(h.group || '', []);
      groups.get(h.group || '').push(h);
    }
    for (const [g, list] of [...groups].sort(([a], [b]) => (a ? 1 : 0) - (b ? 1 : 0) || a.localeCompare(b))) {
      if (g) out.push(el('div', { class: 'host-group' }, g));
      for (const h of list) out.push(...hostRows(h, live, q));
    }
  }

  // The @-agents: what each runs on, its name and what it may do. Tap one to mention it.
  const agents = (app.agents || []).filter((a) => !q || has(a.name));
  const agentsFolded = !q && folded.has('§agents');
  out.push(sectionHead(T('Agents'), '§agents', agentsFolded));
  if (!agentsFolded) {
    if (!(app.agents || []).length) out.push(el('div', { class: 'empty-note' }, T('Add agents in Crew on your computer, then @-mention them in a chat.')));
    for (const a of [...agents].sort((x, y) => x.name.localeCompare(y.name))) {
      out.push(el('div', { class: 'row-item agent-row', onclick: () => mentionAgent(a) },
        runnerIcon(a.cli), el('span', { class: 'label' }, a.name),
        el('span', { class: 'meta' }, T({ read: 'reads', edit: 'edits', full: 'full access' }[a.access] || a.access || ''))));
    }
  }
  $('#d-list').replaceChildren(...out);
}

function sectionHead(label, key, isFolded) {
  return el('div', { class: `tree-head${isFolded ? ' collapsed' : ''}`, onclick: () => toggleFold(key) },
    el('span', { class: 'hosts-title' }, label));
}

const HOST_ICON = '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="7" rx="1.5"/><rect x="3" y="13" width="18" height="7" rx="1.5"/><path d="M7 7.5h.01M7 16.5h.01"/></svg>';
const hostAddress = (h) => `${h.user ? `${h.user}@` : ''}${h.host}${h.port && h.port !== 22 ? `:${h.port}` : ''}`;

function hostRows(h, live, q) {
  const key = `§host:${h.id}`;
  const open = q || !folded.has(key);
  const shell = app.terminals.find((t) => t.sessionId === `ssh:${h.id}`);
  const row = el('div', { class: `row-item host-row${h.connected ? ' connected' : ''}${open ? '' : ' collapsed'}`, onclick: () => openHostFromPhone(h) },
    el('span', { class: 'host-icon', html: HOST_ICON }),
    el('span', { class: 'label' }, h.name),
    el('span', { class: 'meta' }, hostAddress(h)),
    el('span', { class: 'host-fold', html: TREE_ICON.chev, onclick: (e) => { e.stopPropagation(); toggleFold(key); } }));
  if (!open) return [row];
  const children = [el('div', { class: `row-item session host-child${shell && app.current?.termId === shell.termId ? ' active' : ''}`, onclick: () => openHostFromPhone(h, null, true) },
    shell ? el('span', { class: 'dot-run' }) : null,
    el('span', { class: 'agent-icon', html: BADGE.ssh }),
    el('span', { class: 'label' }, T('Terminal')))];
  const fresh = [...live.values()].filter((t) => t.cwd && norm(t.cwd) === norm(h.workspace) && !(h.sessions || []).some((s) => s.id === t.sessionId))
    .map((t) => ({ id: t.sessionId, title: t.title, agent: t.agent, mtime: Date.now(), pending: true }));
  for (const s of [...fresh, ...(h.sessions || [])]) {
    if (q && !has2(q, h.name, h.host) && !has2(q, s.title)) continue;
    const t = live.get(s.id);
    children.push(el('div', { class: `row-item session host-child${t && app.current?.termId === t.termId ? ' active' : ''}`, onclick: () => { closeDrawer(); if (t) openTerminal(t); else openHostFromPhone(h, s); } },
      t ? statusDot(t) : null,
      sessionIcon(s),
      el('span', { class: 'label' }, s.title || T('Untitled')),
      el('span', { class: 'meta' }, s.pending ? '' : relTime(s.mtime))));
  }
  return [row, el('div', { class: 'host-children' }, ...children)];
}
const has2 = (q, ...xs) => xs.some((x) => String(x || '').toLowerCase().includes(q));

function sessionIcon(s) {
  const pv = s.provider && (app.providers || []).find((x) => x.id === s.provider);
  return pv ? providerBadge(pv) : el('span', { class: 'agent-icon', html: BADGE[s.agent] || BADGE.claude });
}
function runnerIcon(cli) {
  if (String(cli || '').startsWith('provider:')) return providerBadge((app.providers || []).find((x) => x.id === cli.slice(9)) || { name: cli.slice(9) });
  return el('span', { class: 'agent-icon', html: BADGE[cli] || BADGE.claude });
}

// A host, as clicking it in Crew does: Crew opens its terminal beside an agent session. Here the
// terminal (or the session asked for) opens once Crew has it.
function openHostFromPhone(h, session = null, terminalOnly = false) {
  closeDrawer();
  const want = session ? session.id : `ssh:${h.id}`;
  const ready = app.terminals.find((t) => t.sessionId === want);
  if (ready && (terminalOnly || session)) { openTerminal(ready); return; }
  send({ t: 'open-host', hostId: h.id, sessionId: session?.id || null, terminalOnly });
  toast(T('Opening it in Crew…'));
  const until = Date.now() + 20000;
  const wait = setInterval(() => {
    const t = app.terminals.find((x) => x.sessionId === want);
    if (t || Date.now() > until) { clearInterval(wait); if (t) openTerminal(t); }
  }, 500);
}

// An @-agent: put its mention into the message box, as typing @ does in Crew.
function mentionAgent(a) {
  closeDrawer();
  const box = $('#in-text');
  box.value = `${box.value}${box.value && !/\s$/.test(box.value) ? ' ' : ''}@${a.name} `;
  autosize();
  box.focus();
}

function sessionItem(p, s, t) {
  const active = t && app.current?.termId === t.termId;
  return el('div', { class: `row-item session${active ? ' active' : ''}`, onclick: () => { closeDrawer(); if (t) openTerminal(t); else resumeSession(p, s); } },
    t ? statusDot(t) : null,
    el('span', { class: 'label' }, s.title || T('Untitled')),
    el('span', { class: 'meta' }, s.pending ? '' : relTime(s.mtime)));
}

const norm = (x) => String(x || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();

// "+" on a project or an agent group: a new chat there.
function newChatIn(path, agent) {
  closeDrawer();
  app.project = path;
  if (agent) app.agent = agent;
  setView('welcome');
  $('#in-text').focus();
}

// ---------- sheets ----------

function sheet(title, options) {
  const box = $('#sheet');
  box.replaceChildren(...(title ? [el('h3', {}, title)] : []), ...options.map((o) => el('button', { class: `opt${o.divider ? ' divider' : ''}`, onclick: () => { closeSheet(); o.run(); } },
    o.icon ? el('span', { html: o.icon }) : null,
    o.mark || null,
    el('span', { class: 'main' }, el('div', {}, o.label), o.sub ? el('small', {}, o.sub) : null),
    o.on ? el('span', { class: 'check', html: ICON.check }) : null)));
  box.classList.remove('hidden');
  $('#sheet-scrim').classList.remove('hidden');
}
function closeSheet() { $('#sheet').classList.add('hidden'); $('#sheet-scrim').classList.add('hidden'); }

function pickProject() {
  sheet(T('Choose a project'), app.projects.length ? app.projects.map((p) => ({
    icon: ICON.folder, label: p.name, sub: p.path, on: p.path === app.project, run: () => { app.project = p.path; renderComposer(); },
  })) : [{ label: T('No projects yet.'), run: () => {} }]);
}
function pickAgent() {
  sheet(T('Agent'), ['claude', 'codex'].map((a) => ({ mark: mark(a), label: AGENT_NAME[a], on: app.agent === a, run: () => { app.agent = a; renderComposer(); } })));
}
function pickComputer() {
  const list = store.get();
  sheet(T('Computers'), [
    ...list.map((c) => ({
      icon: ICON.monitor, label: c.name, sub: `${c.hosts.join(' · ')}${conn.ready && c.token === conn.computer?.token ? ` · ${T('Connected')}` : ''}`,
      on: conn.ready && c.token === conn.computer?.token, run: () => { if (c.token !== conn.computer?.token || !conn.ready) { disconnect(); connect(c); } },
    })),
    { icon: ICON.plus, label: T('Pair another computer'), run: () => { disconnect(); showScreen('connect'); } },
    ...(conn.ready ? [{ icon: ICON.stop, label: T('Disconnect'), run: () => { disconnect(); showScreen('connect'); } }] : []),
  ]);
}
function disconnect() { conn.wanted = false; clearTimeout(conn.timer); try { conn.ws?.close(); } catch { /* */ } }

// The agents' own slash commands, typed in as you would (pickers like /model then take the keys).
const COMMANDS = {
  claude: [['/model', 'Switch model'], ['/compact', 'Summarize to free context'], ['/clear', 'Start over'], ['/context', 'Context used'], ['/usage', 'Plan usage'], ['/resume', 'Resume a session'], ['/help', 'All commands']],
  codex: [['/model', 'Model and reasoning'], ['/permissions', 'Approval mode'], ['/compact', 'Summarize to free context'], ['/new', 'New conversation'], ['/diff', 'Changes so far'], ['/review', 'Review changes'], ['/status', 'Session status']],
};
function pickCommand() {
  const list = COMMANDS[app.current?.agent];
  if (list) sheet(T('Commands'), list.map(([cmd, what]) => ({ label: cmd, sub: T(what), run: () => runCommand(cmd) })));
}
function runCommand(cmd) {
  const t = app.current;
  if (!t) return;
  send({ t: 'input', termId: t.termId, data: cmd });
  setTimeout(() => send({ t: 'input', termId: t.termId, data: '\r' }), 250);
  if (['/model', '/permissions', '/resume'].includes(cmd)) showKeys(true); // pickers take the arrows
}

// ---------- approvals ----------

// crew-ssh's approvals: Allow / Allow 15 min / Deny. Passwords and host keys stay on the computer.
function renderPrompts() {
  $('#prompts').replaceChildren(...[...app.prompts.values()].map((p) => {
    if (p.kind !== 'approve') {
      return el('div', { class: 'prompt' }, el('div', { class: 'what' }, el('span', { class: 'tag' }, T('Needs your OK')),
        `${p.host || ''} ${T('is waiting for you on the computer')} (${T(p.kind === 'hostkey' ? 'a host key to trust' : 'a password or code')})`));
    }
    const verb = { upload: 'upload a file to', download: 'download a file from', input: 'type into your terminal on' }[p.action] || 'run on';
    return el('div', { class: 'prompt' },
      el('div', { class: 'what' }, el('span', { class: 'tag' }, T('Needs your OK')), T('An agent wants to {0} {1}', T(verb), p.host)),
      el('pre', {}, p.command || ''),
      el('div', { class: 'actions' },
        el('button', { onclick: () => answer(p, null) }, T('Deny')),
        el('span', { class: 'grow' }),
        el('button', { onclick: () => answer(p, 'host') }, T('Allow 15 min')),
        el('button', { class: 'ok', onclick: () => answer(p, 'once') }, T('Allow'))));
  }));
}
function answer(p, value) {
  send({ t: 'answer', id: p.id, answer: value });
  app.prompts.delete(p.id);
  renderPrompts();
}

// ---------- agent runs, shown like a trajectory ----------

function renderRuns() {
  renderDrawer();
  if (app.view !== 'runs') return;
  const runs = [...app.runs.values()].reverse();
  const running = runs.filter((r) => r.status === 'running').length;
  $('#runs-view').replaceChildren(
    el('div', { class: 'runs-head' }, T('{0} runs · {1} running', runs.length, running)),
    ...(runs.length ? runs.map((r) => el('div', { class: `step ${r.status}` },
      el('span', { class: 'bar-l' }),
      el('div', { class: 'main' },
        el('div', { class: 'tags' }, el('span', { class: 'tag' }, `@${r.agent}`),
          el('span', { class: `tag ${r.status === 'running' ? 'blue' : r.status === 'failed' ? 'red' : ''}` }, T(r.status === 'running' ? 'Running' : r.status === 'done' ? 'Done' : 'Failed'))),
        el('div', { class: 'text' }, r.task || ''),
        el('div', { class: 'what' }, r.status === 'running' ? r.activity || '' : r.error || '')),
      el('div', { class: 'right' }, r.status === 'running' ? el('button', { class: 'small-btn', onclick: () => send({ t: 'stop-run', id: r.id }) }, T('Stop')) : null)))
      : [el('div', { class: 'empty' }, T('No agent runs yet.'))]));
}

// ---------- notifications ----------

let unseen = 0;
async function notify(title, body) {
  navigator.vibrate?.(120);
  if (!document.hidden) return;
  unseen++;
  document.title = `(${unseen}) Crew`;
  if (Cap.LocalNotifications) {
    try { await Cap.LocalNotifications.schedule({ notifications: [{ id: Date.now() % 100000, title, body }] }); } catch { /* notifications off */ }
  }
}
document.addEventListener('visibilitychange', () => { if (!document.hidden) { unseen = 0; document.title = 'Crew'; } });

let toastTimer = null;
function toast(text) {
  const t = $('#toast');
  t.textContent = text;
  t.classList.remove('hidden');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add('hidden'), 2600);
}

// ---------- starting and opening chats ----------

// A new chat: Crew starts the session; once it's up and has settled, the first message goes in.
function startChat(text) {
  if (!app.project) { pickProject(); return; }
  app.pending = { project: app.project, agent: app.agent, text, known: new Set(app.terminals.map((t) => t.termId)), until: Date.now() + 30000 };
  send({ t: 'open', projectPath: app.project, agent: app.agent, resume: false });
  toast(T('Starting a {0} session in Crew…', AGENT_NAME[app.agent]));
}
function checkPending() {
  const p = app.pending;
  if (!p || p.term) return;
  if (Date.now() > p.until) { app.pending = null; return; }
  const t = app.terminals.find((x) => !p.known.has(x.termId) && x.agent === p.agent);
  if (!t) return;
  p.term = t.termId;
  openTerminal(t);
  // Typed once the agent has drawn its prompt: its first quiet moment, or after 10 s at most.
  p.timer = setTimeout(() => flushPending(t.termId), 10000);
}
function flushPending(termId) {
  const p = app.pending;
  if (!p || p.term !== termId) return;
  clearTimeout(p.timer);
  app.pending = null;
  if (p.text) typeMessage({ termId, agent: p.agent }, p.text);
}

function resumeSession(p, s) {
  const open = app.terminals.find((t) => t.sessionId === s.id);
  if (open) { openTerminal(open); return; }
  send({ t: 'open', projectPath: p.path, sessionId: s.id, agent: s.agent, provider: s.provider, resume: true });
  toast(T('Opening it in Crew…'));
  // It shows under Open once Crew has started it; open it here then.
  const until = Date.now() + 15000;
  const wait = setInterval(() => {
    const t = app.terminals.find((x) => x.sessionId === s.id);
    if (t || Date.now() > until) { clearInterval(wait); if (t) openTerminal(t); }
  }, 500);
}

// ---------- a terminal ----------

// While the phone shows a terminal, Crew gives the terminal the phone's size, so the agent lays its
// screen out for the phone: it fits the width, and scrolls up and down.
const term = new Terminal({ fontFamily: 'ui-monospace, Menlo, "Cascadia Mono", monospace', fontSize: 12, lineHeight: 1.1, scrollback: 5000, disableStdin: true, convertEol: false, theme: themeColors() });
const fit = new FitAddon.FitAddon();
term.loadAddon(fit);
term.open($('#term'));
// Drawn on the GPU (WebGL) where the phone allows it, which is much smoother than the default
// renderer; if the GPU context is lost, xterm falls back to its default renderer.
try {
  const gl = new WebglAddon.WebglAddon();
  gl.onContextLoss(() => gl.dispose());
  term.loadAddon(gl);
} catch { /* no WebGL: the default renderer */ }

// Columns and rows that fill the space exactly, from the size of one character: the fit add-on
// keeps room for a scrollbar this page hides.
function phoneSize() {
  const cell = term._core?._renderService?.dimensions?.css?.cell;
  const box = $('#term');
  if (cell?.width && cell?.height && box.clientWidth) {
    return { cols: Math.max(20, Math.floor(box.clientWidth / cell.width)), rows: Math.max(8, Math.floor(box.clientHeight / cell.height)) };
  }
  const d = fit.proposeDimensions();
  return d && d.cols > 0 ? { cols: Math.max(20, d.cols), rows: Math.max(8, d.rows) } : { cols: 50, rows: 30 };
}
let resizeTimer = null;
function refit() {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    if (!app.current || app.view !== 'term') return;
    const { cols, rows } = phoneSize();
    if (cols === term.cols && rows === term.rows) return;
    term.resize(cols, rows);
    send({ t: 'resize', termId: app.current.termId, cols, rows });
  }, 150);
}

// Swiping scrolls: the terminal's history, or, for an agent that takes the mouse (Codex), the
// agent's own view, with wheel reports as a mouse wheel sends them.
// A flick keeps going and slows down, as native lists do.
(() => {
  const view = $('#term-view');
  let lastY = null;
  let lastT = 0;
  let velocity = 0; // px per ms
  let acc = 0;
  let glide = 0;
  const lineHeight = () => term._core?._renderService?.dimensions?.css?.cell?.height || term.options.fontSize * 1.25;
  function scrollBy(px) {
    acc += px;
    const lines = Math.trunc(acc / lineHeight());
    if (!lines) return;
    acc -= lines * lineHeight();
    if (term.modes.mouseTrackingMode !== 'none' && app.current) {
      const report = lines > 0 ? '\x1b[<65;1;1M' : '\x1b[<64;1;1M';
      send({ t: 'input', termId: app.current.termId, data: report.repeat(Math.min(Math.abs(lines), 10)) });
    } else term.scrollLines(lines);
  }
  view.addEventListener('touchstart', (e) => {
    cancelAnimationFrame(glide);
    lastY = e.touches[0].clientY;
    lastT = performance.now();
    velocity = 0;
    acc = 0;
  }, { passive: true });
  view.addEventListener('touchmove', (e) => {
    if (lastY == null) return;
    const y = e.touches[0].clientY;
    const now = performance.now();
    const dy = lastY - y;
    velocity = 0.8 * (dy / Math.max(1, now - lastT)) + 0.2 * velocity;
    lastY = y;
    lastT = now;
    scrollBy(dy);
    e.preventDefault();
  }, { passive: false });
  view.addEventListener('touchend', () => {
    lastY = null;
    if (performance.now() - lastT > 80 || Math.abs(velocity) < 0.15) return; // a stop, not a flick
    let prev = performance.now();
    const step = (now) => {
      const dt = now - prev;
      prev = now;
      scrollBy(velocity * dt);
      velocity *= Math.pow(0.996, dt); // friction
      if (Math.abs(velocity) > 0.03) glide = requestAnimationFrame(step);
    };
    glide = requestAnimationFrame(step);
  }, { passive: true });
})();

function themeColors() {
  const dark = matchMedia('(prefers-color-scheme: dark)').matches;
  return dark ? { background: '#161718', foreground: '#e9eaec', cursor: '#e9eaec' } : { background: '#ffffff', foreground: '#1b1c1f', cursor: '#1b1c1f', selectionBackground: '#d6ddfb' };
}

function openTerminal(t) {
  if (app.current && app.current.termId !== t.termId) send({ t: 'detach', termId: app.current.termId });
  app.current = t;
  setView('term');
  term.reset();
  const { cols, rows } = phoneSize();
  term.resize(cols, rows);
  term.write(`\x1b[90m${T('Loading…')}\x1b[0m`);
  send({ t: 'attach', termId: t.termId, cols, rows });
  if (t.agent !== 'ssh') send({ t: 'chips', sessionId: t.sessionId });
}

function onSnapshot(m) {
  if (app.current?.termId !== m.termId) return;
  term.reset();
  term.resize(m.cols, m.rows);
  term.write(m.data, () => term.scrollToBottom());
}

// A chip's menu, built by Crew for this session; the pick is made there.
const chipMenus = new Map();
let chipSeq = 0;
function openChip(kind) {
  const t = app.current;
  if (!t) return;
  const reqId = ++chipSeq;
  sheet(T(kind === 'model' ? 'Model' : kind === 'effort' ? 'Effort' : 'Mode'), [{ label: T('Loading…'), run: () => {} }]);
  chipMenus.set(reqId, (menu) => {
    if (!menu?.items?.length) { closeSheet(); toast(menu?.hint || T('Try again when the agent is idle.')); return; }
    sheet(T(kind === 'model' ? 'Model' : kind === 'effort' ? 'Effort' : 'Mode'), menu.items.map((it, i) => ({
      label: T(it.label), sub: it.hint ? T(it.hint) : '', on: it.checked, divider: it.separator,
      run: () => { send({ t: 'chip-pick', sessionId: t.sessionId, menuId: menu.id, index: i }); toast(T('Switching…')); },
    })));
  });
  send({ t: 'chip-menu', sessionId: t.sessionId, kind, reqId });
}

function showKeys(on) {
  $('#keys').classList.toggle('hidden', !on);
  $('#btn-keys').classList.toggle('on', on);
}

// Interrupt: Esc stops Claude's and Codex's current turn; a host's terminal gets Ctrl+C.
function stop() {
  const t = app.current;
  if (t) send({ t: 'input', termId: t.termId, data: t.agent === 'ssh' ? '\x03' : '\x1b' });
}

// Working or idle, as Crew sees it from the terminal's output.
function onActivity(m) {
  const t = app.terminals.find((x) => x.termId === m.termId);
  if (!t) return;
  t.busy = m.busy;
  if (app.current?.termId === m.termId) app.current.busy = m.busy;
  if (!m.busy && app.pending?.term === m.termId) flushPending(m.termId);
  // The agent may have switched model or mode itself (or from the computer): show the chips anew.
  if (!m.busy && app.current?.termId === m.termId && app.current.agent !== 'ssh') send({ t: 'chips', sessionId: app.current.sessionId });
  renderComposer();
  renderDrawer();
  // Finished after a real stretch of work while you weren't watching it: say so.
  const watching = app.current?.termId === m.termId && !document.hidden;
  if (!m.busy && m.ran >= 8000 && !watching) {
    toast(T('Finished: {0}', t.title));
    notify(T('Finished'), t.title);
  }
}

// Typing: a message for an agent goes in as a paste (so newlines stay in it) and then Enter; a
// command for a host's terminal is typed, then Enter.
function typeMessage(t, text) {
  if (t.agent === 'ssh') {
    for (const line of text.replace(/\r\n?/g, '\n').split('\n')) send({ t: 'input', termId: t.termId, data: `${line}\r` });
  } else {
    send({ t: 'input', termId: t.termId, data: `\x1b[200~${text.replace(/\r\n?|\n/g, '\r')}\x1b[201~` });
    setTimeout(() => send({ t: 'input', termId: t.termId, data: '\r' }), text.length > 200 ? 700 : 200);
  }
}
function submit() {
  const box = $('#in-text');
  const text = box.value;
  if (!text.trim()) {
    if (app.view === 'term' && app.current?.busy) stop();
    return;
  }
  if (app.view === 'welcome') startChat(text);
  else if (app.current) typeMessage(app.current, text);
  box.value = '';
  autosize();
}
const KEYS = { esc: '\x1b', tab: '\t', stab: '\x1b[Z', up: '\x1b[A', down: '\x1b[B', left: '\x1b[D', right: '\x1b[C', ctrlc: '\x03', y: 'y', n: 'n', enter: '\r' };
for (const b of document.querySelectorAll('#keys button')) b.addEventListener('click', () => app.current && send({ t: 'input', termId: app.current.termId, data: KEYS[b.dataset.key] }));

function autosize() {
  const t = $('#in-text');
  t.style.height = 'auto';
  t.style.height = `${Math.min(140, Math.max(50, t.scrollHeight))}px`;
  t.style.overflowY = t.scrollHeight > 140 ? 'auto' : 'hidden';
  renderSend();
}

// Photos and files: uploaded to the computer (next to Crew's pasted images), and their paths put
// into the message so the agent can open them.
const uploads = new Map();
let uploadSeq = 0;
function upload(file) {
  return new Promise((resolve, reject) => {
    if (file.size > 20 * 1024 * 1024) { reject(new Error(T('{0} is over 20 MB.', file.name))); return; }
    const r = new FileReader();
    r.onerror = () => reject(new Error(T("Couldn't read {0}.", file.name)));
    r.onload = () => {
      const reqId = ++uploadSeq;
      const timer = setTimeout(() => { uploads.delete(reqId); reject(new Error(T("The computer didn't answer."))); }, 60000);
      uploads.set(reqId, (m) => { clearTimeout(timer); if (m.error) reject(new Error(m.error)); else resolve(m.path); });
      send({ t: 'upload', reqId, name: file.name || 'photo.jpg', data: String(r.result).replace(/^data:[^,]*,/, '') });
    };
    r.readAsDataURL(file);
  });
}
async function attachFiles(files) {
  if (!files.length) return;
  toast(files.length > 1 ? T('Sending {0} files…', files.length) : T('Sending…'));
  const box = $('#in-text');
  for (const file of files) {
    try {
      const p = await upload(file);
      box.value = `${box.value}${box.value && !/\s$/.test(box.value) ? ' ' : ''}"${p}" `;
      autosize();
    } catch (e) { toast(e.message); return; }
  }
  toast(T('Added. Say what to do with it, then send.'));
  box.focus();
}

// ---------- pairing screen ----------

function renderSaved() {
  const list = store.get();
  $('#saved').replaceChildren(...list.map((c) => el('div', { class: 'row', onclick: () => connect(c) },
    el('span', { html: ICON.monitor }), el('span', { class: 't' }, c.name), el('span', { class: 'when' }, c.hosts[0] || ''),
    el('button', { class: 'small-btn', onclick: (e) => { e.stopPropagation(); store.set(store.get().filter((x) => x.token !== c.token)); renderSaved(); } }, T('Forget')))));
}

// ---------- wiring ----------

$('#btn-send').addEventListener('click', submit);
$('#in-text').addEventListener('input', autosize);
$('#in-text').addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submit(); } });
$('#btn-drawer').addEventListener('click', openDrawer);
$('#btn-drawer-close').addEventListener('click', closeDrawer);
$('#scrim').addEventListener('click', closeDrawer);
$('#sheet-scrim').addEventListener('click', closeSheet);
$('#btn-new').addEventListener('click', () => setView('welcome'));
$('#d-new').addEventListener('click', () => { closeDrawer(); setView('welcome'); });
$('#d-runs').addEventListener('click', () => { closeDrawer(); setView('runs'); });
$('#d-computers').addEventListener('click', () => { closeDrawer(); pickComputer(); });
$('#d-search-btn').addEventListener('click', () => { const s = $('#d-search'); s.classList.toggle('hidden'); if (!s.classList.contains('hidden')) s.focus(); else { s.value = ''; renderDrawer(); } });
$('#d-search').addEventListener('input', renderDrawer);
$('#d-lang').addEventListener('click', () => {
  lang = lang === 'zh' ? 'en' : 'zh';
  try { localStorage.setItem('lang', lang); } catch { /* */ }
  applyStatic();
  renderAll();
  setDot(conn.ready ? 'on' : '');
});
$('#btn-keys').addEventListener('click', () => showKeys($('#keys').classList.contains('hidden')));
$('#btn-attach').addEventListener('click', () => $('#in-file').click());
$('#in-file').addEventListener('change', (e) => { const files = [...e.target.files]; e.target.value = ''; attachFiles(files); });
$('#btn-manual').addEventListener('click', () => {
  const [host, port] = $('#in-host').value.trim().split(':');
  const token = $('#in-token').value.trim().replace(/^.*·\s*/, '');
  if (!host || !token) { setStatus(T('Enter the address and the pairing code shown in Crew.'), true); return; }
  connect({ name: host, hosts: [host], port: +port || 7788, token });
});
$('#btn-scan').addEventListener('click', async () => {
  const scanner = Cap.BarcodeScanner;
  if (!scanner) { setStatus(T('Scan the QR code in Crew with your phone camera, or enter the address and code here.'), true); return; }
  try {
    const perm = await scanner.requestPermissions();
    if (perm.camera !== 'granted' && perm.camera !== 'limited') { setStatus(T('Crew needs the camera to scan the code.'), true); return; }
    const { barcodes } = await scanner.scan({ formats: ['QR_CODE'] });
    const pairing = barcodes?.length && parsePairing(barcodes[0].rawValue);
    if (!pairing) { setStatus(T("That isn't Crew's pairing code."), true); return; }
    connect(pairing);
  } catch (e) { setStatus(String(e.message || e), true); }
});
window.addEventListener('resize', refit);
// The space changes too when an approval card or the keys come and go, or the box grows.
if (window.ResizeObserver) new ResizeObserver(refit).observe($('#term-view'));
window.visualViewport?.addEventListener('resize', refit);
Cap.App?.addListener?.('resume', () => { if (conn.wanted && !conn.ready) connect(conn.computer); });
Cap.App?.addListener?.('backButton', () => {
  if (!$('#sheet').classList.contains('hidden')) closeSheet();
  else if ($('#drawer').classList.contains('open')) closeDrawer();
  else if (app.view !== 'welcome') setView('welcome');
});
Cap.LocalNotifications?.requestPermissions?.().catch(() => {});

// Start: the last computer, or the pairing screen. A pairing link (Crew's QR code opened by the
// camera) pairs directly.
(() => {
  applyStatic();
  setView('welcome');
  const link = parsePairing(location.href);
  const last = store.get()[0];
  // Opened in a browser from Crew itself: this address answers, so try it first, and keep the
  // pairing code out of the address bar and history.
  if (!window.Capacitor && /^https?:$/.test(location.protocol)) {
    $('#btn-scan').classList.add('hidden');
    if (link) {
      link.hosts = [location.hostname, ...link.hosts.filter((h) => h !== location.hostname)];
      history.replaceState(null, '', location.pathname);
    }
  }
  showScreen('connect');
  if (link) connect(link);
  else if (last) connect(last);
})();
