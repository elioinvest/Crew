/* global api, $, el, state, AGENTS, providerIcon, showMenu, gitDialog, i18n, usage, windowName */
// The Usage page, opened from the status bar: estimated cost and tokens by period, provider and
// model, read from the agents' session files (usage-scan.js). Layout and calculations follow T3
// Code's (MIT). Costs are what the tokens would cost at API list prices, so on a subscription they
// show what it's worth rather than what was paid.

const usagePage = {
  open: false,
  days: [1, 7, 30, 90].includes(+localStorage.getItem('usageDays')) ? +localStorage.getItem('usageDays') : 1,
  metric: ['cost', 'tokens', 'limits'].includes(localStorage.getItem('usageMetric')) ? localStorage.getItem('usageMetric') : 'cost',
  provider: null, // null for all
  breakdown: 'model',
  data: null,
  loading: false,
};
const usageEl = $('#usage-page');
const USAGE_PERIODS = [[1, 'Past 24h'], [7, '7 days'], [30, '30 days'], [90, '90 days']];
const USAGE_METRICS = [['cost', 'Cost'], ['tokens', 'Tokens'], ['limits', 'Limits']];
const HOUR = 3600e3;

// ---------- numbers ----------

const fmtUsd = (n) => (n >= 1000 ? `$${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` : `$${n.toFixed(2)}`);
function fmtTokens(n) {
  if (n < 1000) return String(Math.round(n));
  const [d, u] = n >= 1e9 ? [1e9, 'B'] : n >= 1e6 ? [1e6, 'M'] : [1e3, 'K'];
  const v = n / d;
  return `${v >= 100 ? Math.round(v) : v.toPrecision(3)}${u}`;
}
const fmtShare = (s) => (s == null ? '—' : `${(s * 100).toFixed(1)}%`);

function providerInfo(id) {
  if (id === 'claude') return { label: 'Claude Code', color: '#d97757', icon: () => el('span', { class: 'agent-icon', html: AGENTS.claude.badge }) };
  if (id === 'codex') return { label: 'Codex', color: 'var(--text)', icon: () => el('span', { class: 'agent-icon', html: AGENTS.codex.badge }) };
  const p = state.providers.find((x) => x.id === id);
  let h = 0;
  for (const c of id) h = (h * 31 + c.charCodeAt(0)) % 360;
  return { label: p?.name || id, color: `hsl(${h} 55% 48%)`, icon: () => (p ? el('span', { class: 'agent-icon' }, providerIcon(p)) : el('span', { class: 'agent-icon' })) };
}

// Per token. A price set here wins over the list, and has no fast tiers.
function ratesFor(model) {
  const o = state.config.usagePrices?.[model];
  if (o) return { input: o.input / 1e6, output: o.output / 1e6, cacheRead: (o.cacheRead ?? o.input) / 1e6, cacheWrite: (o.cacheWrite ?? o.input) / 1e6 };
  return usagePage.data?.rates?.[model] || null;
}

// Rows: [hour, provider, model, speed index, session index, input, cached, cache write, output, reasoning]
function costOfRow(row) {
  const base = ratesFor(row[2]);
  if (!base) return null;
  const r = (row[3] === 1 && base.fast) || (row[3] === 2 && base.ultrafast) || base;
  return { input: row[5] * r.input, cacheRead: row[6] * r.cacheRead, cacheWrite: row[7] * r.cacheWrite, output: row[8] * r.output, savings: row[6] * (r.input - r.cacheRead) };
}

function emptyTotals() {
  return { input: 0, cached: 0, write: 0, output: 0, reasoning: 0, tokens: 0, cost: 0, savings: 0, unpricedTokens: 0, byType: { input: 0, cacheRead: 0, cacheWrite: 0, output: 0 }, sessions: new Set() };
}

function addRow(t, row) {
  const tokens = row[5] + row[6] + row[7] + row[8];
  t.input += row[5];
  t.cached += row[6];
  t.write += row[7];
  t.output += row[8];
  t.reasoning += row[9];
  t.tokens += tokens;
  t.sessions.add(row[4]);
  const c = costOfRow(row);
  if (!c) { t.unpricedTokens += tokens; return; }
  t.cost += c.input + c.cacheRead + c.cacheWrite + c.output;
  t.savings += c.savings;
  for (const k of ['input', 'cacheRead', 'cacheWrite', 'output']) t.byType[k] += c[k];
}

// The time slots of a period: hours for a day, otherwise local days.
function usageBuckets(days) {
  const now = Date.now();
  if (days === 1) {
    const end = Math.floor(now / HOUR) * HOUR;
    return Array.from({ length: 24 }, (_, i) => { const at = end - (23 - i) * HOUR; return { from: at, to: at + HOUR }; });
  }
  const out = [];
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  for (let i = days - 1; i >= 0; i--) {
    const start = new Date(d);
    start.setDate(d.getDate() - i);
    const end = new Date(start);
    end.setDate(start.getDate() + 1);
    out.push({ from: start.getTime(), to: end.getTime() });
  }
  return out;
}

function summarize() {
  const { days, provider } = usagePage;
  const buckets = usageBuckets(days).map((b) => ({ ...b, total: emptyTotals(), perProvider: new Map() }));
  const since = buckets[0].from;
  const all = emptyTotals();
  const providers = new Map();
  const models = new Map();
  for (const row of usagePage.data?.rows || []) {
    if (row[0] < since) continue;
    if (!providers.has(row[1])) providers.set(row[1], emptyTotals());
    addRow(providers.get(row[1]), row);
    if (provider && row[1] !== provider) continue;
    addRow(all, row);
    const key = `${row[1]}|${row[2]}`;
    if (!models.has(key)) models.set(key, { provider: row[1], model: row[2], total: emptyTotals() });
    addRow(models.get(key).total, row);
    // Rows are hourly, so a row's hour decides its slot.
    const b = days === 1 ? buckets[Math.round((row[0] - since) / HOUR)] : buckets.find((x) => row[0] >= x.from && row[0] < x.to);
    if (!b) continue;
    addRow(b.total, row);
    if (!b.perProvider.has(row[1])) b.perProvider.set(row[1], emptyTotals());
    addRow(b.perProvider.get(row[1]), row);
  }
  return { buckets, all, providers, models: [...models.values()] };
}

// ---------- the chart ----------

function niceMax(v) {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 1.5, 2, 3, 4, 5, 7.5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

// A smooth line that never overshoots its points (monotone cubic).
function smoothPath(pts) {
  if (pts.length < 2) return pts.length ? `M${pts[0][0]} ${pts[0][1]}` : '';
  const n = pts.length;
  const dx = [];
  const m = [];
  for (let i = 0; i < n - 1; i++) { dx[i] = pts[i + 1][0] - pts[i][0]; m[i] = (pts[i + 1][1] - pts[i][1]) / dx[i]; }
  const t = [m[0]];
  for (let i = 1; i < n - 1; i++) t[i] = m[i - 1] * m[i] <= 0 ? 0 : (2 * m[i - 1] * m[i]) / (m[i - 1] + m[i]);
  t[n - 1] = m[n - 2];
  let d = `M${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < n - 1; i++) {
    const c = dx[i] / 3;
    d += `C${(pts[i][0] + c).toFixed(1)} ${(pts[i][1] + c * t[i]).toFixed(1)} ${(pts[i + 1][0] - c).toFixed(1)} ${(pts[i + 1][1] - c * t[i + 1]).toFixed(1)} ${pts[i + 1][0].toFixed(1)} ${pts[i + 1][1].toFixed(1)}`;
  }
  return d;
}

const bucketLabel = (b, days) => {
  const d = new Date(b.from);
  if (days !== 1) return `${d.getMonth() + 1}/${d.getDate()}`;
  return d.toLocaleTimeString([], { hour: 'numeric' });
};

function usageChart(buckets, ids) {
  const metric = usagePage.metric;
  // Drawn at its real size, so the labels aren't stretched.
  const card = usageEl.firstElementChild.clientWidth || 1000;
  const W = Math.max(280, card > 760 ? card - 48 - 300 - 24 : card - 48);
  const H = 240;
  const left = 56;
  const bottom = 26;
  const top = 8;
  const val = (t) => (metric === 'cost' ? t.cost : t.tokens);
  const series = ids.map((id) => ({ id, info: providerInfo(id), values: buckets.map((b) => (b.perProvider.get(id) ? val(b.perProvider.get(id)) : 0)) }));
  const max = niceMax(Math.max(0, ...series.flatMap((s) => s.values)));
  const X = (i) => left + (buckets.length < 2 ? 0 : (i / (buckets.length - 1)) * (W - left - 4));
  const Y = (v) => top + (1 - v / max) * (H - top - bottom);
  const fmt = (v) => (v === 0 ? '0' : metric === 'cost' ? fmtUsd(v) : fmtTokens(v));
  let s = '';
  for (const f of [0, 1 / 3, 2 / 3, 1]) {
    const y = Y(max * f);
    s += `<line x1="${left}" x2="${W - 4}" y1="${y}" y2="${y}" class="usage-grid"/><text x="${left - 10}" y="${y + 4}" class="usage-axis" text-anchor="end">${fmt(max * f)}</text>`;
  }
  for (const i of [0, Math.floor((buckets.length - 1) / 2), buckets.length - 1]) {
    s += `<text x="${X(i)}" y="${H - 6}" class="usage-axis" text-anchor="${i === 0 ? 'start' : i === buckets.length - 1 ? 'end' : 'middle'}">${bucketLabel(buckets[i], usagePage.days)}</text>`;
  }
  for (const line of series) {
    const pts = line.values.map((v, i) => [X(i), Y(v)]);
    const d = smoothPath(pts);
    s += `<path d="${d}L${X(buckets.length - 1)} ${Y(0)}L${X(0)} ${Y(0)}Z" fill="${line.info.color}" opacity=".1" stroke="none"/><path d="${d}" fill="none" stroke="${line.info.color}" stroke-width="2"/>`;
  }
  s += '<line class="usage-cursor hidden" y1="8" x1="0" x2="0"/>';
  const svg = el('div', { class: 'usage-chart-svg', html: `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">${s}</svg>` });
  const tip = el('div', { class: 'usage-tip hidden' });
  const cursor = svg.querySelector('.usage-cursor');
  cursor.setAttribute('y2', String(Y(0)));
  svg.addEventListener('mousemove', (e) => {
    const r = svg.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * W;
    const i = Math.max(0, Math.min(buckets.length - 1, Math.round(((x - left) / (W - left - 4)) * (buckets.length - 1))));
    cursor.setAttribute('x1', String(X(i)));
    cursor.setAttribute('x2', String(X(i)));
    cursor.classList.remove('hidden');
    tip.replaceChildren(el('div', { class: 'usage-tip-head' }, bucketLabel(buckets[i], usagePage.days)), ...series.filter((l) => l.values[i] > 0 || series.length === 1).map((l) => el('div', {}, el('span', { class: 'usage-dot', style: `background:${l.info.color}` }), `${l.info.label} `, el('b', {}, fmt(l.values[i])))));
    tip.classList.remove('hidden');
    tip.style.left = `${Math.min((X(i) / W) * r.width + 10, r.width - 170)}px`;
  });
  svg.addEventListener('mouseleave', () => { cursor.classList.add('hidden'); tip.classList.add('hidden'); });
  return el('div', { class: 'usage-chart' }, svg, tip);
}

// ---------- the page ----------

function segmented(options, value, onpick) {
  return el('div', { class: 'usage-seg' }, ...options.map(([v, label]) => el('button', { class: v === value ? 'on' : '', onclick: () => onpick(v) }, label)));
}

function shareBar(segments) {
  const total = segments.reduce((n, s) => n + s.value, 0);
  const bar = el('div', { class: 'usage-share' });
  if (total > 0) for (const s of segments) if (s.value > 0) { const part = el('span', { class: `usage-ink-${s.ink}`, title: s.label }); part.style.flexGrow = String(s.value / total); bar.append(part); }
  return el('div', {}, bar, el('div', { class: 'usage-legend' }, ...segments.map((s) => el('span', {}, el('i', { class: `usage-ink-${s.ink}` }), el('span', { class: 'muted' }, s.label), ` ${s.text}`))));
}

const usageStat = (label, value) => el('div', { class: 'usage-stat' }, el('div', { class: 'muted' }, label), el('div', { class: 'usage-stat-v' }, value));

function usageModelDialog(m) {
  const cur = state.config.usagePrices?.[m.model];
  const rate = ratesFor(m.model);
  const perM = (v) => (v == null ? '' : String(+(v * 1e6).toFixed(4)));
  const input = m.total.input + m.total.cached + m.total.write;
  const lines = [
    `${fmtTokens(m.total.tokens)} tokens`,
    input ? `${i18n.t('Cache hit rate')} ${((m.total.cached / input) * 100).toFixed(1)}%` : null,
    m.total.reasoning ? `${i18n.t('Reasoning')} ${fmtTokens(m.total.reasoning)}` : null,
    !m.total.unpricedTokens && m.total.tokens ? `${fmtUsd((m.total.cost / m.total.tokens) * 1e6)} / M` : null,
  ].filter(Boolean).join(' · ');
  gitDialog({
    title: m.model,
    text: `${lines}\n${i18n.t('Prices are USD per million tokens. Leave them all empty to use the price list.')}`,
    fields: [
      { id: 'input', label: 'Input', value: cur ? String(cur.input) : perM(rate?.input) },
      { id: 'output', label: 'Output', value: cur ? String(cur.output) : perM(rate?.output) },
      { id: 'cacheRead', label: 'Cache read', value: cur?.cacheRead != null ? String(cur.cacheRead) : perM(rate?.cacheRead) },
      { id: 'cacheWrite', label: 'Cache write', value: cur?.cacheWrite != null ? String(cur.cacheWrite) : perM(rate?.cacheWrite) },
    ],
    ok: 'Save',
  }).then(async (v) => {
    if (!v) return;
    const prices = { ...state.config.usagePrices };
    const num = (s) => (s === '' || !Number.isFinite(+s) || +s < 0 ? null : +s);
    const listed = usagePage.data?.rates?.[m.model];
    const same = listed && [['input', listed.input], ['output', listed.output], ['cacheRead', listed.cacheRead], ['cacheWrite', listed.cacheWrite]].every(([k, r]) => num(v[k]) !== null && Math.abs(num(v[k]) - r * 1e6) < 1e-3);
    if (num(v.input) === null || num(v.output) === null || same) delete prices[m.model];
    else prices[m.model] = { input: num(v.input), output: num(v.output), ...(num(v.cacheRead) !== null ? { cacheRead: num(v.cacheRead) } : {}), ...(num(v.cacheWrite) !== null ? { cacheWrite: num(v.cacheWrite) } : {}) };
    state.config = await api.setConfig({ usagePrices: prices });
    renderUsagePage();
    renderTokenItem();
  });
}

function usageBreakdown(sum) {
  const metric = usagePage.metric;
  const cost = metric === 'cost';
  const val = (t) => (cost ? t.cost : t.tokens);
  const whole = val(sum.all) || 1;
  const hourly = usagePage.days === 1;
  const head = el('div', { class: 'usage-section-head' }, el('h3', {}, 'Breakdown'),
    segmented([['model', 'Model'], ['time', hourly ? 'Hour' : 'Day']], usagePage.breakdown, (v) => { usagePage.breakdown = v; renderUsagePage(); }));
  if (usagePage.breakdown === 'time') {
    const rows = sum.buckets.filter((b) => b.total.tokens > 0).reverse().map((b) => el('tr', {},
      el('td', {}, new Date(b.from).toLocaleString([], hourly ? { month: 'numeric', day: 'numeric', hour: 'numeric' } : { year: 'numeric', month: 'numeric', day: 'numeric' })),
      el('td', { class: 'num' }, fmtUsd(b.total.cost)),
      el('td', { class: 'num muted' }, fmtShare(val(b.total) / whole)),
      el('td', { class: 'num muted' }, fmtTokens(b.total.tokens)),
      el('td', { class: 'num muted' }, String(b.total.sessions.size))));
    return el('div', {}, head, el('table', { class: 'usage-table' },
      el('thead', {}, el('tr', {}, el('th', {}, hourly ? 'Hour' : 'Day'), el('th', { class: 'num' }, 'Cost'), el('th', { class: 'num' }, 'Share'), el('th', { class: 'num' }, 'Tokens'), el('th', { class: 'num' }, 'Sessions'))),
      el('tbody', {}, ...rows)));
  }
  const models = [...sum.models].sort((a, b) => b.total.tokens - a.total.tokens || b.total.cost - a.total.cost);
  const rows = models.map((m, i) => {
    const unknown = m.total.unpricedTokens >= m.total.tokens;
    const share = cost && unknown ? null : val(m.total) / whole;
    const info = providerInfo(m.provider);
    const bar = el('div', { class: 'usage-model-bar' });
    bar.style.width = `${Math.max(1, (share ?? 0) * 100)}%`;
    bar.style.background = info.color;
    return el('tr', { class: 'usage-model-row', title: 'Click for details and to set its price', onclick: () => usageModelDialog(m) },
      el('td', { class: 'muted' }, String(i + 1)),
      el('td', {}, el('div', { class: 'usage-model-name' }, info.icon(), el('span', { class: 'usage-mono' }, m.model), state.config.usagePrices?.[m.model] ? el('span', { class: 'usage-tag' }, 'Your price') : null), bar),
      el('td', { class: 'num' }, unknown ? el('span', { class: 'usage-unpriced' }, 'Set price') : fmtUsd(m.total.cost)),
      el('td', { class: 'num muted' }, fmtShare(share)),
      el('td', { class: 'num muted' }, fmtTokens(m.total.tokens)));
  });
  return el('div', {}, head, el('table', { class: 'usage-table' },
    el('thead', {}, el('tr', {}, el('th', {}, '#'), el('th', {}, 'Model'), el('th', { class: 'num' }, 'Cost'), el('th', { class: 'num' }, 'Share'), el('th', { class: 'num' }, 'Tokens'))),
    el('tbody', {}, ...rows)));
}

function usageLimitsView() {
  const fmtReset = (at) => {
    if (!at) return '';
    const mins = Math.max(0, Math.round((at - Date.now()) / 60000));
    const text = mins >= 1440 ? `${Math.floor(mins / 1440)}d ${Math.floor((mins % 1440) / 60)}h` : mins >= 60 ? `${Math.floor(mins / 60)}h ${mins % 60}m` : `${mins}m`;
    return `${i18n.t('Resets in')} ${text}`;
  };
  const groups = Object.keys(AGENTS).filter((a) => usage[a]?.windows?.length).map((a) => el('div', { class: 'usage-limit-group' },
    el('div', { class: 'usage-provider-name' }, el('span', { class: 'agent-icon', html: AGENTS[a].badge }), el('b', {}, providerInfo(a).label), usage[a].plan ? el('span', { class: 'muted' }, usage[a].plan) : null),
    ...usage[a].windows.map((w) => {
      const pct = Math.min(100, Math.round(w.used));
      const fill = el('i', {});
      fill.style.width = `${pct}%`;
      fill.className = pct >= 90 ? 'high' : pct >= 70 ? 'warn' : '';
      return el('div', { class: 'usage-limit' },
        el('div', { class: 'usage-limit-head' }, el('span', {}, windowName(w.minutes)), el('span', {}, el('b', {}, `${pct}%`), ' ', el('span', { class: 'muted' }, fmtReset(w.resetsAt)))),
        el('div', { class: 'usage-limit-bar' }, fill));
    })));
  return el('div', { class: 'usage-body' }, ...(groups.length ? groups : [el('div', { class: 'git-placeholder' }, 'Checking plan usage…')]));
}

function renderUsagePage() {
  if (!usagePage.open) return;
  const scroll = usageEl.querySelector('.usage-scroll')?.scrollTop || 0;
  const metric = usagePage.metric;
  const label = usagePage.provider ? providerInfo(usagePage.provider).label : 'All providers';
  const head = el('div', { class: 'usage-head' },
    el('div', { class: 'usage-crumbs' }, el('span', { class: 'muted' }, 'Usage'), el('span', { class: 'muted' }, '/'),
      el('button', {
        class: 'usage-picker',
        onclick: (e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const known = [...new Set((usagePage.data?.rows || []).map((x) => x[1]))];
          showMenu({ clientX: r.left, clientY: r.bottom + 4 }, [
            { label: 'All providers', checked: !usagePage.provider, action: () => { usagePage.provider = null; renderUsagePage(); } },
            ...known.map((id) => ({ label: providerInfo(id).label, checked: usagePage.provider === id, action: () => { usagePage.provider = id; renderUsagePage(); } })),
          ]);
        },
      }, label, el('span', { html: '<svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>' }))),
    el('span', { class: 'spacer' }),
    metric === 'limits' ? null : segmented(USAGE_PERIODS, usagePage.days, (v) => { usagePage.days = v; localStorage.setItem('usageDays', v); renderUsagePage(); }),
    segmented(USAGE_METRICS, metric, (v) => { usagePage.metric = v; localStorage.setItem('usageMetric', v); renderUsagePage(); }),
    el('button', { class: `icon-btn small${usagePage.loading ? ' spin' : ''}`, title: 'Refresh', html: '<svg viewBox="0 0 24 24"><path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/></svg>', onclick: () => loadUsageStats(true) }),
    el('button', { class: 'icon-btn small', title: 'Close', html: '<svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>', onclick: closeUsagePage }));

  let body;
  if (metric === 'limits') body = usageLimitsView();
  else if (!usagePage.data) body = el('div', { class: 'git-placeholder' }, 'Reading session files…');
  else if (usagePage.data.error && !usagePage.data.rows?.length) body = el('div', { class: 'git-placeholder' }, usagePage.data.error);
  else {
    const sum = usageBody();
    body = sum;
  }
  const card = usageEl.firstElementChild;
  card.replaceChildren(head, el('div', { class: 'usage-scroll' }, body));
  card.querySelector('.usage-scroll').scrollTop = scroll;
}

function usageBody() {
  const sum = summarize();
  const { all } = sum;
  const metric = usagePage.metric;
  const cost = metric === 'cost';
  const val = (t) => (cost ? t.cost : t.tokens);
  const whole = [...sum.providers.values()].reduce((n, t) => n + val(t), 0) || 1;
  const unpricedShare = all.tokens ? all.unpricedTokens / all.tokens : 0;
  const shown = [...sum.providers.keys()].filter((id) => !usagePage.provider || id === usagePage.provider).sort((a, b) => val(sum.providers.get(b)) - val(sum.providers.get(a)));
  const sessionsText = (n) => `${n.toLocaleString('en-US')} ${i18n.t(n === 1 ? 'session' : 'sessions')}`;

  const summary = el('div', { class: 'usage-summary' },
    el('div', { class: 'usage-big' }, cost ? fmtUsd(all.cost) : fmtTokens(all.tokens)),
    el('div', { class: 'muted usage-sub' }, sessionsText(all.sessions.size), cost ? ` · ${i18n.t('API estimate')}` : ''),
    cost && unpricedShare > 0 ? el('div', { class: 'usage-warn' }, `${i18n.t('Leaves out unpriced models:')} ${unpricedShare < 0.001 ? '<0.1%' : fmtShare(unpricedShare)}`) : null,
    usagePage.data.pricing === 'bundled' && cost ? el('div', { class: 'muted usage-sub' }, 'Using the built-in price list: the latest one could not be fetched.') : null,
    ...shown.map((id) => {
      const t = sum.providers.get(id);
      const info = providerInfo(id);
      const line = el('div', {
        class: `usage-provider${usagePage.provider === id ? ' on' : ''}`,
        onclick: () => { usagePage.provider = usagePage.provider === id ? null : id; renderUsagePage(); },
      },
      el('div', { class: 'usage-provider-name' },
        el('span', { class: 'usage-dot', style: `background:${info.color}` }), info.icon(), el('span', {}, info.label),
        el('span', { class: 'muted usage-small' }, sessionsText(t.sessions.size)),
        el('span', { class: 'spacer' }), el('span', {}, cost ? fmtUsd(t.cost) : fmtTokens(t.tokens))),
      el('div', { class: 'muted usage-small' }, cost ? `${fmtShare(t.cost / whole)} ${i18n.t('of cost')} · ${fmtTokens(t.tokens)} tokens` : `${fmtShare(t.tokens / whole)} ${i18n.t('of tokens')} · ${fmtUsd(t.cost)}`));
      return line;
    }));

  const hourly = usagePage.days === 1;
  const title = i18n.t(`${hourly ? 'Hourly' : 'Daily'} ${cost ? 'cost' : 'processed tokens'}`);
  const chart = el('div', { class: 'usage-chart-box' }, el('h3', {}, title), usageChart(sum.buckets, shown.length ? shown : ['claude']));
  const totals = el('div', { class: 'usage-section' }, el('h3', {}, 'Totals'), el('div', { class: 'usage-stats' },
    usageStat('Processed tokens', fmtTokens(all.tokens)),
    usageStat('Cached input', fmtTokens(all.cached)),
    usageStat('Uncached input', fmtTokens(all.input)),
    usageStat('Output', fmtTokens(all.output)),
    usageStat('Cache savings', fmtUsd(all.savings))));
  const types = cost
    ? [['Input', all.byType.input, 1], ['Cache read', all.byType.cacheRead, 2], ['Cache write', all.byType.cacheWrite, 3], ['Output', all.byType.output, 4]].map(([label, value, ink]) => ({ label, value, ink, text: fmtUsd(value) }))
    : [['Input', all.input, 1], ['Cache read', all.cached, 2], ['Cache write', all.write, 3], ['Output', all.output, 4]].map(([label, value, ink]) => ({ label, value, ink, text: fmtTokens(value) }));
  const byType = el('div', { class: 'usage-section' }, el('h3', {}, cost ? 'Cost by type' : 'Tokens by type'), shareBar(types));
  return el('div', { class: 'usage-body' },
    el('div', { class: 'usage-top' }, summary, chart), totals, byType, el('div', { class: 'usage-section' }, usageBreakdown(sum)));
}

async function loadUsageStats(force = false) {
  usagePage.loading = true;
  renderUsagePage();
  try { usagePage.data = await api.usageStats(force); } catch { /* keep what's shown */ }
  usagePage.loading = false;
  renderUsagePage();
  renderTokenItem();
}

function openUsagePage() {
  usagePage.open = true;
  usageEl.classList.remove('hidden');
  renderUsagePage();
  loadUsageStats();
}

function closeUsagePage() {
  usagePage.open = false;
  usageEl.classList.add('hidden');
}

let usageResize = 0;
addEventListener('resize', () => { clearTimeout(usageResize); usageResize = setTimeout(renderUsagePage, 100); });
usageEl.addEventListener('mousedown', (e) => { if (e.target === usageEl) closeUsagePage(); });
document.addEventListener('keydown', (e) => {
  if (usagePage.open && e.key === 'Escape' && $('#git-dialog').classList.contains('hidden') && $('#menu').classList.contains('hidden')) closeUsagePage();
});

// ---------- beside the plan limits in the status bar ----------

function renderTokenItem() {
  const item = $('#sb-tokens');
  const rows = usagePage.data?.rows;
  if (!rows) { item.classList.add('hidden'); return; }
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  const t = emptyTotals();
  for (const row of rows) if (row[0] >= start.getTime()) addRow(t, row);
  item.classList.remove('hidden');
  $('#sb-tokens-text').textContent = `${i18n.t('Today')} ${fmtUsd(t.cost)} · ${fmtTokens(t.tokens)}`;
  item.title = i18n.t('Token usage and estimated API cost');
}

$('#sb-tokens').addEventListener('click', openUsagePage);
api.onUsageStatsChanged(async () => {
  usagePage.data = await api.usageStats();
  renderTokenItem();
  renderUsagePage();
});
// The first read waits until Crew has settled; after that it follows as long as Crew is in front.
setTimeout(() => loadUsageStats(), 8000);
setInterval(() => { if (document.hasFocus() && !usagePage.loading) loadUsageStats(); }, 5 * 60e3);
