// Token usage and estimated cost, for the Usage page beside the plan limits (renderer/usage-page.js).
// usage-scan.js does the reading in a utility process, which exits when it's done. Prices come from
// LiteLLM's list, fetched at most daily; usage-prices.json is a trimmed copy for when that fails.

const { ipcMain, net, utilityProcess } = require('electron');
const fs = require('fs');
const path = require('path');

const PRICES_URL = 'https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json';
const PRICES_FRESH = 24 * 3600e3;
const STATS_FRESH = 60e3;
const SCAN_TIMEOUT = 5 * 60e3;

let deps = {}; // { dataDir, roots(), send(channel) }
let last = null;
let running = null;

const fetchedPrices = () => path.join(deps.dataDir(), 'usage-prices.json');
const bundledPrices = path.join(__dirname, 'usage-prices.json');

// The list to use: a recent download, a new one, an older one, or the bundled copy.
async function pricesFile() {
  const file = fetchedPrices();
  let age = Infinity;
  try { age = Date.now() - fs.statSync(file).mtimeMs; } catch { /* none yet */ }
  if (age < PRICES_FRESH) return { path: file, source: 'fresh' };
  try {
    const r = await net.fetch(PRICES_URL, { signal: AbortSignal.timeout(15000) });
    if (!r.ok) throw new Error(String(r.status));
    const text = await r.text();
    if (typeof JSON.parse(text) !== 'object') throw new Error('not a list');
    fs.writeFileSync(`${file}.tmp`, text);
    fs.renameSync(`${file}.tmp`, file);
    return { path: file, source: 'fresh' };
  } catch {
    return age !== Infinity ? { path: file, source: 'cached' } : { path: bundledPrices, source: 'bundled' };
  }
}

function runScan(job) {
  return new Promise((resolve) => {
    let child;
    try { child = utilityProcess.fork(path.join(__dirname, 'usage-scan.js'), [], { serviceName: 'Crew usage scan', stdio: 'ignore' }); } catch (e) { resolve({ error: e.message }); return; }
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try { child.kill(); } catch { /* gone */ }
      resolve(v);
    };
    const timer = setTimeout(() => finish({ error: 'The scan took too long.' }), SCAN_TIMEOUT);
    child.once('message', finish);
    child.once('exit', () => finish({ error: 'The scan stopped.' }));
    child.once('spawn', () => child.postMessage(job));
  });
}

function refresh() {
  if (running) return running;
  running = (async () => {
    const prices = await pricesFile();
    const r = await runScan({ roots: deps.roots(), cachePath: path.join(deps.dataDir(), 'usage-scan-cache.json'), pricesPath: prices.path });
    if (r.ok) last = { ...r.ok, pricing: prices.source };
    else last = { ...(last || { rows: [], rates: {} }), error: r.error };
    running = null;
    deps.send('usage-stats:changed');
    return last;
  })();
  return running;
}

function init(d) {
  deps = d;
  ipcMain.handle('usage-stats:get', async (_e, force) => {
    if (force || !last) return refresh();
    if (Date.now() - last.at > STATS_FRESH) refresh(); // the page updates when it's done
    return last;
  });
}

module.exports = { init };
