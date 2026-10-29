const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('api', {
  listProjects: () => ipcRenderer.invoke('projects:list'),
  addProject: () => ipcRenderer.invoke('projects:add'),
  hideProject: (p) => ipcRenderer.invoke('projects:hide', p),
  revealProject: (p) => ipcRenderer.invoke('projects:reveal', p),
  pickFiles: () => ipcRenderer.invoke('dialog:files'),
  saveAttachment: (bytes, mime) => ipcRenderer.invoke('attachments:save', bytes, mime),
  pathForFile: (file) => webUtils.getPathForFile(file),
  fileExists: (p) => ipcRenderer.invoke('file:exists', p),
  openFile: (p) => ipcRenderer.invoke('file:open', p),
  showFile: (p) => ipcRenderer.invoke('file:show', p),
  sessionState: (id, agent) => ipcRenderer.invoke('sessions:state', id, agent),
  renameSession: (id, title, agent) => ipcRenderer.invoke('sessions:rename', id, title, agent),
  deleteSession: (id, title, agent) => ipcRenderer.invoke('sessions:delete', id, title, agent),
  onProjectsChanged: (cb) => ipcRenderer.on('projects:changed', () => cb()),
  onBrowserNewTab: (cb) => ipcRenderer.on('browser:new-tab', (_e, url) => cb(url)),
  onBrowserShortcut: (cb) => ipcRenderer.on('browser:shortcut', (_e, k) => cb(k)),

  gitRoot: (cwd) => ipcRenderer.invoke('git:root', cwd),
  git: (cwd, args, input) => ipcRenderer.invoke('git:run', cwd, args, input),
  gitIgnore: (root, pattern) => ipcRenderer.invoke('git:ignore', root, pattern),

  getConfig: () => ipcRenderer.invoke('config:get'),
  setConfig: (patch) => ipcRenderer.invoke('config:set', patch),

  spawn: (opts) => ipcRenderer.invoke('pty:spawn', opts),
  checkAgent: (agent) => ipcRenderer.invoke('agent:check', agent),
  installAgent: (opts) => ipcRenderer.invoke('pty:install', opts),
  write: (id, data) => ipcRenderer.send('pty:write', id, data),
  resize: (id, cols, rows) => ipcRenderer.send('pty:resize', id, cols, rows),
  kill: (id) => ipcRenderer.send('pty:kill', id),
  rekey: (id, sessionId) => ipcRenderer.send('pty:rekey', id, sessionId),
  onData: (cb) => ipcRenderer.on('pty:data', (_e, id, data) => cb(id, data)),
  onExit: (cb) => ipcRenderer.on('pty:exit', (_e, id, code) => cb(id, code)),
});
