// Custom model providers (providers.js in the main process): the list in Settings, the editor, and
// each provider's icon. A provider runs on Claude Code (Anthropic Messages) or Codex (OpenAI
// Responses) and shows up in the New session menus.

const PROTOCOL_LABELS = {
  responses: 'OpenAI Responses',
  anthropic: 'Anthropic Messages',
};
const PROTOCOL_HINTS = {
  responses: 'Runs on Codex. For APIs that serve /v1/responses.',
  anthropic: 'Runs on Claude Code. For APIs that serve /v1/messages.',
};

// The provider's icon: its image, its emoji, or its first letter on a color from its ID.
function providerIcon(p, size = '') {
  const icon = el('span', { class: `provider-icon ${size}`.trim(), title: p.name });
  if (p.icon?.image) icon.append(el('img', { src: p.icon.image, alt: '' }));
  else if (p.icon?.emoji) icon.textContent = p.icon.emoji;
  else {
    let h = 0;
    for (const c of p.id || p.name || '?') h = (h * 31 + c.charCodeAt(0)) % 360;
    icon.classList.add('letter');
    icon.style.background = `hsl(${h} 55% 48%)`;
    icon.textContent = (p.name || p.id || '?').trim().charAt(0).toUpperCase();
  }
  return icon;
}

async function loadProviders() {
  state.providers = await api.listProviders();
  renderProviderList();
  renderSidebar();
}

function renderProviderList() {
  const list = $('#provider-list');
  if (!list) return;
  list.replaceChildren(...(state.providers.length ? state.providers.map((p) => el('div', { class: 'provider-row' },
    providerIcon(p),
    el('span', { class: 'provider-name' }, p.name),
    el('span', { class: 'provider-meta' }, `${PROTOCOL_LABELS[p.protocol]} · ${p.models.length ? `${p.models.length} model${p.models.length === 1 ? '' : 's'}` : 'no models'}`),
    el('span', { class: 'spacer' }),
    el('button', { type: 'button', class: 'small-btn', onclick: () => editProvider(p) }, 'Edit'),
    el('button', { type: 'button', class: 'small-btn danger', onclick: () => deleteProvider(p) }, 'Delete')))
    : [el('span', { class: 'field-hint' }, 'None yet. Add a relay, a self-hosted service or another API compatible with OpenAI or Anthropic.')]));
}

async function deleteProvider(p) {
  if (!(await confirmBox(`Delete ${p.name}?`, 'Sessions started on it are kept, but resume on the agent\'s own account.', 'Delete'))) return; // hosts.js
  await api.deleteProvider(p.id);
  loadProviders();
}

const providerModal = $('#provider-modal');

function editProvider(p) {
  const isNew = !p;
  p = p || { id: '', name: '', baseUrl: '', protocol: 'responses', models: [], defaultModel: '', icon: null, hasKey: false };
  let icon = p.icon ? { ...p.icon } : null;
  // One editable row per model: its ID and the name the model menu shows. One row is the default.
  let rows = p.models.map((m) => ({ id: m.id, name: m.name || '' }));
  let defaultRow = rows.find((r) => r.id === p.defaultModel) || rows[0] || null;

  const id = textInput(p.id, { placeholder: 'acme-gateway' });
  const name = textInput(p.name, { placeholder: 'Display name' });
  // The ID is made valid as it's typed ("DeepSeek Relay" -> "deepseek-relay"), and follows the
  // display name until it's typed in itself.
  const toId = (v) => v.toLowerCase().replace(/[^a-z0-9-]+/g, '-').replace(/^[^a-z]+/, '').replace(/-{2,}/g, '-').slice(0, 40);
  let idTyped = !!p.id;
  id.addEventListener('input', () => { const v = toId(id.value); if (v !== id.value) id.value = v; idTyped = !!id.value; });
  id.addEventListener('blur', () => { id.value = id.value.replace(/-+$/, ''); });
  name.addEventListener('input', () => { if (!idTyped) id.value = toId(name.value).replace(/-+$/, ''); });
  const url = textInput(p.baseUrl, { placeholder: 'https://gateway.example/v1' });
  const protocol = el('select', {}, ...Object.entries(PROTOCOL_LABELS).map(([v, l]) => el('option', { value: v }, l)));
  protocol.value = p.protocol;
  const protocolHint = el('span', { class: 'field-hint' });
  const syncProtocol = () => { protocolHint.textContent = PROTOCOL_HINTS[protocol.value]; };
  protocol.addEventListener('change', syncProtocol);
  syncProtocol();
  const key = el('input', { type: 'password', spellcheck: 'false', placeholder: p.hasKey ? 'Saved. Leave empty to keep it' : 'Enter the API key' });

  // Icon: an image file, an emoji, or the letter avatar.
  const preview = el('span', { class: 'provider-icon-preview' });
  const emoji = textInput(icon?.emoji || '', { placeholder: 'Emoji', maxlength: '8', class: 'emoji-input' });
  const syncIcon = () => preview.replaceChildren(providerIcon({ id: id.value || 'x', name: name.value || id.value || '?', icon }, 'large'));
  emoji.addEventListener('input', () => { icon = emoji.value.trim() ? { emoji: emoji.value.trim() } : null; syncIcon(); });
  id.addEventListener('input', syncIcon);
  name.addEventListener('input', syncIcon);
  const iconRow = el('div', { class: 'provider-icon-row' }, preview,
    el('button', { type: 'button', class: 'small-btn', onclick: async () => {
      const r = await api.pickProviderIcon();
      if (!r) return;
      if (r.error) { status.textContent = r.error; return; }
      icon = { image: r.image };
      emoji.value = '';
      syncIcon();
    } }, 'Choose image…'),
    emoji,
    el('button', { type: 'button', class: 'small-btn', onclick: () => { icon = null; emoji.value = ''; syncIcon(); } }, 'Reset'));
  syncIcon();

  // A readable name from a model ID: "workbuddy-ai/deepseek-v4.1-flash" -> "Deepseek-V4.1-Flash".
  const prettyName = (mid) => mid.split('/').pop().replace(/(^|[-_ .])([a-z])/g, (_, sep, c) => sep + c.toUpperCase());

  // The model list: rows fetched from the provider or added by hand, each editable.
  const modelList = el('div', { class: 'provider-models' });
  const star = '<svg viewBox="0 0 24 24"><path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z"/></svg>';
  const trash = '<svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6"/></svg>';
  const renderModels = (focusLast = false) => {
    if (!rows.includes(defaultRow)) defaultRow = rows[0] || null;
    modelList.replaceChildren(...(rows.length ? rows.map((r) => {
      const mid = textInput(r.id, { placeholder: 'Model ID', class: 'model-id' });
      const mname = textInput(r.name, { placeholder: 'Display name' });
      mid.addEventListener('input', () => { r.id = mid.value.trim(); });
      mname.addEventListener('input', () => { r.name = mname.value; });
      // A name follows a fresh ID until it's typed in itself.
      mid.addEventListener('change', () => { if (!r.name && r.id) { r.name = prettyName(r.id); mname.value = r.name; } });
      return el('div', { class: `provider-model${r === defaultRow ? ' default' : ''}` }, mid, mname,
        el('button', { type: 'button', class: `icon-btn small model-star${r === defaultRow ? ' on' : ''}`, title: r === defaultRow ? 'Default model for new sessions' : 'Make this the default model for new sessions', html: star, onclick: () => { defaultRow = r; renderModels(); } }),
        el('button', { type: 'button', class: 'icon-btn small', title: 'Remove', html: trash, onclick: () => { rows = rows.filter((x) => x !== r); renderModels(); } }));
    }) : [el('div', { class: 'provider-models-empty' }, 'No models yet. Fetch the provider\'s list, or add a model and type its ID.')]));
    if (focusLast) modelList.lastElementChild?.querySelector('input')?.focus();
  };
  renderModels();
  const addModel = () => { rows.push({ id: '', name: '' }); renderModels(true); };
  const fetchBtn = el('button', { type: 'button', class: 'small-btn' }, 'Fetch available models');
  fetchBtn.addEventListener('click', async () => {
    fetchBtn.disabled = true;
    status.textContent = 'Fetching the models…';
    const r = await api.fetchProviderModels({ id: isNew ? '' : p.id, baseUrl: url.value.trim(), protocol: protocol.value, key: key.value.trim() });
    fetchBtn.disabled = false;
    if (r.error) { status.textContent = `Couldn't fetch the models: ${r.error}`; return; }
    rows = rows.filter((x) => x.id || x.name); // drop empty rows
    const have = new Set(rows.map((x) => x.id));
    const fresh = r.models.filter((m) => !have.has(m));
    rows.push(...fresh.map((m) => ({ id: m, name: prettyName(m) })));
    status.textContent = r.models.length ? `Found ${r.models.length} models (${fresh.length} new).` : 'The provider listed no models. Add them by hand.';
    renderModels();
  });

  const status = el('div', { class: 'field-hint provider-status' });
  const saveBtn = el('button', { type: 'button', class: 'primary' }, isNew ? 'Create provider' : 'Save');
  saveBtn.addEventListener('click', async () => {
    // Rows left completely empty are dropped; a named row needs its ID.
    const kept = rows.filter((r) => r.id || r.name.trim());
    const missing = kept.findIndex((r) => !r.id);
    if (missing >= 0) { status.textContent = `Model ${missing + 1}: the model ID can't be empty.`; return; }
    const ids = kept.map((r) => r.id);
    const dup = ids.find((x, i) => ids.indexOf(x) !== i);
    if (dup) { status.textContent = `${dup} is in the list twice.`; return; }
    try {
      await api.saveProvider({
        originalId: isNew ? '' : p.id, id: toId(id.value || name.value).replace(/-+$/, ''), name: name.value.trim(), baseUrl: url.value.trim(),
        protocol: protocol.value, key: key.value, models: kept.map((r) => ({ id: r.id, name: r.name.trim() || prettyName(r.id) })),
        defaultModel: kept.includes(defaultRow) ? defaultRow.id : kept[0]?.id || '', icon,
      });
    } catch (e) {
      status.textContent = String(e.message || e).replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
      return;
    }
    providerModal.classList.add('hidden');
    loadProviders();
  });

  providerModal.firstElementChild.replaceChildren(
    el('h2', {}, isNew ? 'Add a model provider' : `Edit ${p.name}`),
    el('p', { class: 'field-hint' }, 'Connect a relay, a self-hosted service or another API compatible with OpenAI or Anthropic. Crew runs it with Codex or Claude Code, set up through their official options.'),
    field('Provider ID', id, 'Starts with a lowercase letter; a-z, 0-9 and -. It identifies the provider in Crew.'),
    field('Display name', name),
    el('label', {}, 'Icon', iconRow, el('span', { class: 'field-hint' }, 'An image (PNG, SVG, ICO… under 512 KB) or an emoji. Without one, the first letter of the name is shown.')),
    field('API URL', url),
    el('label', {}, 'API protocol', protocol, protocolHint),
    field('API key', key, 'Saved encrypted with Windows data protection. Agents get it only in the sessions you start on this provider.'),
    el('div', { class: 'provider-models-head' }, el('span', {}, 'Models'), fetchBtn),
    el('span', { class: 'field-hint' }, 'Each model has its ID (sent to the API) and the name shown in the session\'s model menu. ★ marks the default for new sessions.'),
    modelList,
    el('div', {}, el('button', { type: 'button', class: 'small-btn', onclick: addModel }, '+ Add model')),
    status,
    el('div', { class: 'modal-actions' },
      el('button', { type: 'button', onclick: () => providerModal.classList.add('hidden') }, 'Cancel'),
      saveBtn));
  providerModal.classList.remove('hidden');
  (isNew ? id : name).focus();
}

$('#provider-add').addEventListener('click', () => editProvider(null));
