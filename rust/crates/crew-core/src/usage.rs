//! The status bar's numbers. Plan limits come from Crew's status line hook, Codex's logs and a live
//! check; today's tokens and estimated API cost are read out of the session files. The rules for
//! reading those files come from usage-scan.js in the Electron app (and T3 Code before it, MIT).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents;
use crate::config::{Config, Paths};
use crate::env;
use crate::sessions::ProviderHome;

// ---------- plan limits ----------

#[derive(Clone, Debug, PartialEq)]
pub struct PlanWindow {
    /// Percent used.
    pub used: f64,
    /// Epoch milliseconds.
    pub resets_at: Option<i64>,
    pub minutes: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanUsage {
    pub at: i64,
    pub plan: Option<String>,
    pub windows: Vec<PlanWindow>,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Whichever reading is newest wins.
pub fn newest(a: Option<PlanUsage>, b: Option<PlanUsage>) -> Option<PlanUsage> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.at > a.at { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// What the status line hook last saved. A plan's limits are shared, so any session's reading counts.
pub fn claude_from_hook(paths: &Paths) -> Option<PlanUsage> {
    let text = std::fs::read_to_string(paths.data_dir.join("usage-claude.json")).ok()?;
    let j: Value = serde_json::from_str(&text).ok()?;
    let limits = j.get("rate_limits")?;
    let window = |w: Option<&Value>, minutes| {
        let w = w?;
        Some(PlanWindow { used: w.get("used_percentage")?.as_f64()?, resets_at: w.get("resets_at").and_then(Value::as_f64).map(|s| (s * 1000.) as i64), minutes })
    };
    let windows: Vec<_> = [window(limits.get("five_hour"), 300), window(limits.get("seven_day"), 10080)].into_iter().flatten().collect();
    (!windows.is_empty()).then(|| PlanUsage { at: j.get("at").and_then(Value::as_i64).unwrap_or(0), plan: None, windows })
}

fn sorted_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

fn file_mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn codex_window(w: Option<&Value>, used: &str, resets: &str, minutes: &str) -> Option<PlanWindow> {
    let w = w?;
    Some(PlanWindow {
        used: w.get(used)?.as_f64()?,
        resets_at: w.get(resets).and_then(Value::as_f64).map(|s| (s * 1000.) as i64),
        minutes: w.get(minutes).and_then(Value::as_i64).unwrap_or(0),
    })
}

/// The newest `rate_limits` Codex wrote into a session log.
pub fn codex_from_logs(paths: &Paths) -> Option<PlanUsage> {
    let sessions = paths.codex_home.join("sessions");
    let mut files = Vec::new();
    'walk: for y in sorted_names(&sessions) {
        for m in sorted_names(&sessions.join(&y)) {
            for d in sorted_names(&sessions.join(&y).join(&m)) {
                let dir = sessions.join(&y).join(&m).join(&d);
                files.extend(sorted_names(&dir).into_iter().filter(|f| f.ends_with(".jsonl")).map(|f| dir.join(f)));
                if files.len() >= 40 {
                    break 'walk;
                }
            }
        }
    }
    let mut by_mtime: Vec<(PathBuf, i64)> = files.into_iter().map(|f| { let t = file_mtime_ms(&f); (f, t) }).collect();
    by_mtime.sort_by_key(|(_, t)| -t);
    for (file, _) in by_mtime.into_iter().take(15) {
        let Ok(mut f) = std::fs::File::open(&file) else { continue };
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut bytes = Vec::new();
        if f.seek(SeekFrom::Start(size.saturating_sub(1024 * 1024))).is_err() || f.read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines().rev().filter(|l| l.contains("\"rate_limits\":{")) {
            let Ok(j) = serde_json::from_str::<Value>(line) else { continue };
            let p = j.get("payload");
            let Some(rl) = p.and_then(|p| p.get("rate_limits").or_else(|| p.get("info").and_then(|i| i.get("rate_limits")))) else { continue };
            let windows: Vec<_> = ["primary", "secondary"].iter().filter_map(|k| codex_window(rl.get(k), "used_percent", "resets_at", "window_minutes")).collect();
            if !windows.is_empty() {
                let at = j.get("timestamp").and_then(Value::as_str).and_then(parse_ms).unwrap_or(0);
                return Some(PlanUsage { at, plan: rl.get("plan_type").and_then(Value::as_str).map(str::to_string), windows });
            }
        }
    }
    None
}

/// Codex's current limits, asked of Codex itself: its app-server fetches them with Codex's sign-in.
pub fn codex_live(config: &Config) -> Option<PlanUsage> {
    let command = config.str("codexCommand");
    let launch = agents::shell_command(&[if command.is_empty() { "codex".to_string() } else { command.to_string() }, "app-server".to_string()]);
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args).env_clear().envs(env::clean_env()).envs(launch.env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let hello = json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"crew","title":"Crew","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}});
    let _ = writeln!(stdin, "{hello}");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut result = None;
    while let Ok(line) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        match m.get("id").and_then(Value::as_i64) {
            Some(1) => {
                let _ = writeln!(stdin, "{}", json!({"method":"initialized"}));
                let _ = writeln!(stdin, "{}", json!({"id":2,"method":"account/rateLimits/read"}));
            }
            Some(2) => {
                let rl = m.get("result").and_then(|r| r.get("rateLimits"));
                let windows: Vec<_> = ["primary", "secondary"]
                    .iter()
                    .filter_map(|k| codex_window(rl.and_then(|r| r.get(k)), "usedPercent", "resetsAt", "windowDurationMins"))
                    .collect();
                if !windows.is_empty() {
                    result = Some(PlanUsage { at: now_ms(), plan: rl.and_then(|r| r.get("planType")).and_then(Value::as_str).map(str::to_string), windows });
                }
                break;
            }
            _ => {}
        }
    }
    drop(stdin); // the app-server leaves when its input ends; killing reaches only cmd.exe
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// "7:50pm", "Nov 2, 7pm" or "Nov 2 at 7:30am", in local time.
fn parse_reset_time(text: &str, now: DateTime<Local>) -> Option<i64> {
    static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:([A-Za-z]{3})[a-z]*\s+(\d{1,2}),?\s+(?:at\s+)?)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)").unwrap());
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let m = TIME.captures(text)?;
    let hour = m[3].parse::<u32>().ok()? % 12 + if m[5].eq_ignore_ascii_case("pm") { 12 } else { 0 };
    let minute = m.get(4).map_or(Some(0), |x| x.as_str().parse().ok())?;
    let at = |date: NaiveDate| Local.from_local_datetime(&date.and_hms_opt(hour, minute, 0)?).earliest();
    let today = now.date_naive();
    if let Some(month) = m.get(1) {
        let lower = month.as_str().to_lowercase();
        let month = MONTHS.iter().position(|x| *x == lower)? as u32 + 1;
        let day = m[2].parse().ok()?;
        let this_year = at(NaiveDate::from_ymd_opt(today.year(), month, day)?)?;
        if this_year.timestamp_millis() < now.timestamp_millis() - 86_400_000 {
            return Some(at(NaiveDate::from_ymd_opt(today.year() + 1, month, day)?)?.timestamp_millis());
        }
        return Some(this_year.timestamp_millis());
    }
    let same_day = at(today)?;
    if same_day <= now { Some(at(today.succ_opt()?)?.timestamp_millis()) } else { Some(same_day.timestamp_millis()) }
}

/// The "Current session" and "Current week" bars of Claude Code's /usage screen.
pub fn parse_claude_usage_screen(text: &str) -> Option<PlanUsage> {
    static SESSION: LazyLock<Regex> = LazyLock::new(|| bar_regex("Current session"));
    static WEEK_ALL: LazyLock<Regex> = LazyLock::new(|| bar_regex(r"Current week \(all models\)"));
    static WEEK: LazyLock<Regex> = LazyLock::new(|| bar_regex("Current week"));
    let now = Local::now();
    let bar = |r: &Regex, minutes| {
        let m = r.captures(text)?;
        Some(PlanWindow { used: m[1].parse().ok()?, resets_at: parse_reset_time(&m[2], now), minutes })
    };
    let windows: Vec<_> = [bar(&SESSION, 300), bar(&WEEK_ALL, 10080).or_else(|| bar(&WEEK, 10080))].into_iter().flatten().collect();
    static PLAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Claude (Pro|Max|Team|Enterprise)\b").unwrap());
    let plan = PLAN.captures(text).map(|m| m[1].to_string());
    (!windows.is_empty()).then(|| PlanUsage { at: now_ms(), plan, windows })
}

fn bar_regex(label: &str) -> Regex {
    Regex::new(&format!(r"(?i){label}[^\n]*\n?[^%]*?(\d+(?:\.\d+)?)%\s*used[\s\S]*?Resets\s+([^\n(]+)")).unwrap()
}

// ---------- today's tokens ----------

const SYNTHETIC: &str = "<synthetic>";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Record {
    /// Minutes since the epoch.
    t: i64,
    model: String,
    session: String,
    input: i64,
    cached: i64,
    write: i64,
    output: i64,
    reasoning: i64,
    /// 0 standard, 1 fast, 2 ultrafast.
    speed: u8,
    key: Option<String>,
}

fn parse_ms(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text).ok().map(|t| t.timestamp_millis())
}

fn count(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_f64).filter(|n| n.is_finite() && *n > 0.).map_or(0, |n| n.trunc() as i64)
}

/// One line per content block, each repeating the whole message's usage, so a message counts once
/// (by message and request id, across files, as resumed sessions copy them).
fn claude_records(line: &str) -> Vec<Record> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return Vec::new();
    }
    let Ok(o) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    if o.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    let Some(m) = o.get("message") else { return Vec::new() };
    let Some(u) = m.get("usage").filter(|u| u.is_object()) else { return Vec::new() };
    let Some(t) = o.get("timestamp").and_then(Value::as_str).and_then(parse_ms).map(|ms| ms.div_euclid(60_000)) else { return Vec::new() };
    let model = m.get("model").and_then(Value::as_str).unwrap_or("");
    if model.is_empty() || model == SYNTHETIC {
        return Vec::new();
    }
    let id = m.get("id").and_then(Value::as_str);
    let key = match (id, o.get("requestId").and_then(Value::as_str)) {
        (Some(id), Some(request)) => Some(format!("{id}:{request}")),
        (Some(id), None) => Some(id.to_string()),
        _ => None,
    };
    let speed = u8::from(u.get("speed").and_then(Value::as_str) == Some("fast"));
    let session = o.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
    let record = |model: &str, u: &Value, key: Option<String>| Record {
        t,
        model: model.to_string(),
        session: session.clone(),
        input: count(u.get("input_tokens")),
        cached: count(u.get("cache_read_input_tokens")),
        write: count(u.get("cache_creation_input_tokens")),
        output: count(u.get("output_tokens")),
        reasoning: 0,
        speed,
        key,
    };
    let mut out = vec![record(model, u, key.clone())];
    // Advisor calls run on another model at its own rates and appear only here.
    for (i, a) in u.get("iterations").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let advisor = a.get("type").and_then(Value::as_str) == Some("advisor_message");
        let Some(model) = a.get("model").and_then(Value::as_str).filter(|m| advisor && !m.is_empty() && *m != SYNTHETIC) else { continue };
        out.push(record(model, a, key.as_ref().map(|k| format!("{k}:advisor:{i}"))));
    }
    out
}

/// input, cached, cache write, output, reasoning, total.
type Counters = [i64; 6];
const FIELDS: [&str; 6] = ["input_tokens", "cached_input_tokens", "cache_write_input_tokens", "output_tokens", "reasoning_output_tokens", "total_tokens"];

fn valid(u: Counters) -> Option<Counters> {
    let ok = u.iter().all(|v| *v >= 0) && u[1] + u[2] <= u[0] && u[4] <= u[3] && u[5] >= u[0] + u[3];
    ok.then_some(u)
}

fn codex_counters(raw: Option<&Value>) -> Option<Counters> {
    let raw = raw?.as_object()?;
    let field = |k: &str| raw.get(k).filter(|v| !v.is_null());
    let whole = |v: &Value| v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.).map(|f| f as i64));
    let input = whole(field(FIELDS[0])?)?;
    let output = whole(field(FIELDS[3])?)?;
    let or_zero = |k: &str| match field(k) {
        Some(v) => whole(v),
        None => Some(0),
    };
    let total = match field(FIELDS[5]) {
        Some(v) => whole(v)?,
        None => input + output,
    };
    valid([input, or_zero(FIELDS[1])?, or_zero(FIELDS[2])?, output, or_zero(FIELDS[4])?, total])
}

fn advance(previous: Option<Counters>, last: Counters) -> Option<Counters> {
    let p = previous.unwrap_or([0; 6]);
    valid(std::array::from_fn(|i| p[i] + last[i]))
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct CodexState {
    model: String,
    speed: u8,
    session: String,
    last_signature: Option<Counters>,
    cumulative: Option<Counters>,
    sequence_start: Option<(i64, Counters)>,
    saw_meta: bool,
    fork_copies: bool,
    fork_anchor: i64,
}

impl CodexState {
    fn rebase(&mut self, cumulative: Counters) {
        self.cumulative = Some(cumulative);
        if cumulative[0] + cumulative[3] == 0 {
            self.sequence_start = None;
        }
    }
}

const FORK_COPY_GAP: i64 = 1000; // ms: forks restamp their parent's history in a burst

/// token_count events carry no model (the last turn_context has it), can be re-emitted, and forks
/// copy their parent's history, so the cumulative counters decide what's a real request.
fn codex_record(line: &str, state: &mut CodexState) -> Option<Record> {
    if !["\"token_count\"", "\"turn_context\"", "\"thread_settings_applied\"", "\"session_meta\""].iter().any(|k| line.contains(k)) {
        return None;
    }
    let o = serde_json::from_str::<Value>(line).ok()?;
    let p = o.get("payload").filter(|p| p.is_object())?;
    let at = o.get("timestamp").and_then(Value::as_str).and_then(parse_ms);
    let kind = o.get("type").and_then(Value::as_str);
    if kind == Some("session_meta") {
        if state.saw_meta {
            return None; // copied ancestors' metas would misattribute what follows
        }
        state.saw_meta = true;
        if let Some(id) = p.get("id").or_else(|| p.get("session_id")).and_then(Value::as_str) {
            state.session = id.to_string();
        }
        let parent = p.pointer("/source/subagent/thread_spawn/parent_thread_id");
        let forked = p.get("forked_from_id").is_some_and(Value::is_string) || parent.is_some_and(Value::is_string);
        if let (true, Some(at)) = (forked, at) {
            state.fork_copies = true;
            state.fork_anchor = at;
        }
        return None;
    }
    if kind == Some("turn_context") {
        if let Some(model) = p.get("model").and_then(Value::as_str) {
            state.model = model.to_string();
        }
        return None;
    }
    let inner = p.get("type").and_then(Value::as_str);
    if inner == Some("thread_settings_applied") {
        state.speed = match p.pointer("/thread_settings/service_tier").and_then(Value::as_str) {
            Some("priority") | Some("fast") => 1,
            Some("ultrafast") => 2,
            _ => 0,
        };
        return None;
    }
    let info = p.get("info").filter(|i| i.is_object());
    if inner != Some("token_count") || info.is_none() {
        return None;
    }
    let info = info?;
    let at = at?;
    if state.model.is_empty() {
        return None;
    }
    let last = codex_counters(info.get("last_token_usage"));
    let has_cumulative = info.get("total_token_usage").is_some_and(|v| !v.is_null());
    let cumulative = if has_cumulative { codex_counters(info.get("total_token_usage")) } else { None };

    if state.fork_copies {
        if at - state.fork_anchor < FORK_COPY_GAP {
            state.fork_anchor = at;
            // Already billed in the parent, but it sets the baseline here.
            if let Some(c) = cumulative {
                state.rebase(c);
            } else if let (false, Some(l), true) = (has_cumulative, last, last != state.last_signature) {
                state.cumulative = advance(state.cumulative, l);
            }
            state.last_signature = last;
            return None;
        }
        state.fork_copies = false;
        state.last_signature = None;
    }

    let previous = state.cumulative;
    if let Some(c) = cumulative {
        state.rebase(c);
    }
    let last_counters = last?;
    if has_cumulative && cumulative.is_none() {
        return None;
    }
    // Counters with nothing in them report the context's size, not a request.
    if last_counters[0] + last_counters[3] == 0 {
        state.last_signature = None;
        return None;
    }
    if cumulative.is_some() && cumulative == previous {
        return None; // re-emitted
    }
    if last_counters[5] != last_counters[0] + last_counters[3] {
        return None;
    }
    if let Some(c) = cumulative {
        // A session resumed in a new process restarts its counters at this request.
        let restarted = c == last_counters;
        let start = (at, last_counters);
        if restarted && state.sequence_start == Some(start) {
            state.cumulative = previous;
            return None;
        }
        let reconciled = match previous {
            None => valid(std::array::from_fn(|i| c[i] - last_counters[i])).is_some(),
            Some(p) => (0..6).all(|i| c[i] - p[i] == last_counters[i]),
        };
        if !restarted && !reconciled {
            return None;
        }
        if restarted {
            state.sequence_start = Some(start);
        }
    } else {
        if last == state.last_signature {
            return None;
        }
        state.cumulative = Some(advance(previous, last_counters)?);
    }
    state.last_signature = last;
    let [input, cached, write, output, reasoning, _] = last_counters;
    let input = input - cached - write; // Codex's input includes the cached part
    if input + cached + write + output == 0 {
        return None;
    }
    Some(Record { t: at.div_euclid(60_000), model: state.model.clone(), session: state.session.clone(), input, cached, write, output, reasoning, speed: state.speed, key: None })
}

/// Per token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelRates {
    pub standard: Rates,
    pub fast: Option<Rates>,
    pub ultrafast: Option<Rates>,
}

/// Rates per token, from LiteLLM's price list. Cache rates missing from an entry use the input rate.
fn price_table(doc: &Value) -> HashMap<String, ModelRates> {
    fn rates(e: &Value, suffix: &str, base: Option<Rates>) -> Option<Rates> {
        let num = |k: String| e.get(k).and_then(Value::as_f64);
        let input = num(format!("input_cost_per_token{suffix}"))?;
        let output = num(format!("output_cost_per_token{suffix}"))?;
        let cache = |name: &str, of_base: fn(&Rates) -> f64| {
            num(format!("{name}{suffix}")).unwrap_or(match base {
                Some(b) if b.input > 0. => of_base(&b) / b.input * input,
                _ => input,
            })
        };
        Some(Rates { input, output, cache_read: cache("cache_read_input_token_cost", |b| b.cache_read), cache_write: cache("cache_creation_input_token_cost", |b| b.cache_write) })
    }
    let mut table = HashMap::new();
    for (name, e) in doc.as_object().into_iter().flatten() {
        let Some(standard) = rates(e, "", None) else { continue };
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        let fast = match e.pointer("/provider_specific_entry/fast").and_then(Value::as_f64).filter(|x| *x > 0.) {
            Some(x) => Some(Rates { input: standard.input * x, output: standard.output * x, cache_read: standard.cache_read * x, cache_write: standard.cache_write * x }),
            None => rates(e, "_priority", Some(standard)),
        };
        table.insert(key, ModelRates { standard, fast, ultrafast: rates(e, "_ultrafast", Some(standard)) });
    }
    // "anthropic/claude-x" also answers to "claude-x", unless resellers disagree on its price.
    let mut aliases: HashMap<String, Option<ModelRates>> = HashMap::new();
    for (key, r) in &table {
        let bare = key.rsplit('/').next().unwrap_or(key);
        if bare.is_empty() || bare == key || table.contains_key(bare) {
            continue;
        }
        aliases.entry(bare.to_string()).and_modify(|a| if a.is_some_and(|a| a.standard != r.standard) { *a = None }).or_insert(Some(*r));
    }
    table.extend(aliases.into_iter().filter_map(|(k, r)| r.map(|r| (k, r))));
    table
}

/// Bare family names could be any generation, so they stay unpriced. "[1m]" variants price as the base.
fn rate_of<'a>(table: &'a HashMap<String, ModelRates>, model: &str) -> Option<&'a ModelRates> {
    let lower = model.trim().to_lowercase();
    let key = lower.split('[').next().unwrap_or("");
    let bare = key.rsplit('/').next().unwrap_or("");
    if bare.is_empty() || ["synthetic", "<synthetic>", "opus", "sonnet", "haiku", "fable"].contains(&bare) {
        return None;
    }
    table.get(key)
}

/// A price set in Settings wins over the list, and has no fast tiers.
fn set_price(config: &Config, model: &str) -> Option<Rates> {
    let o = config.raw().get("usagePrices")?.get(model)?;
    let million = |k: &str| o.get(k).and_then(Value::as_f64).map(|v| v / 1e6);
    let input = million("input")?;
    Some(Rates { input, output: million("output")?, cache_read: million("cacheRead").unwrap_or(input), cache_write: million("cacheWrite").unwrap_or(input) })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Today {
    pub cost: f64,
    pub tokens: i64,
}

/// One hour of one model on one provider in one session.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Epoch milliseconds at the start of the hour.
    pub hour: i64,
    pub provider: String,
    pub model: String,
    pub speed: u8,
    pub session: usize,
    pub input: i64,
    pub cached: i64,
    pub write: i64,
    pub output: i64,
    pub reasoning: i64,
}

impl Row {
    pub fn tokens(&self) -> i64 {
        self.input + self.cached + self.write + self.output
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub rows: Vec<Row>,
    pub rates: HashMap<String, ModelRates>,
    /// "fresh", "cached" or "bundled".
    pub pricing: String,
}

impl UsageData {
    /// Per token, for this row's speed. A price set in Settings wins, and has no fast tiers.
    pub fn rates_for(&self, config: &Config, model: &str, speed: u8) -> Option<Rates> {
        set_price(config, model).or_else(|| {
            let m = self.rates.get(model)?;
            Some(match speed {
                1 => m.fast.unwrap_or(m.standard),
                2 => m.ultrafast.unwrap_or(m.standard),
                _ => m.standard,
            })
        })
    }

    /// Today's totals, as the status bar shows them.
    pub fn today(&self, config: &Config) -> Today {
        let midnight = Local::now().date_naive().and_hms_opt(0, 0, 0).and_then(|d| Local.from_local_datetime(&d).earliest()).map_or(0, |d| d.timestamp_millis());
        let mut t = Today::default();
        for r in self.rows.iter().filter(|r| r.hour >= midnight) {
            t.tokens += r.tokens();
            if let Some(c) = self.rates_for(config, &r.model, r.speed) {
                t.cost += r.input as f64 * c.input + r.cached as f64 * c.cache_read + r.write as f64 * c.cache_write + r.output as f64 * c.output;
            }
        }
        t
    }
}

#[derive(Default, Serialize, Deserialize)]
struct FileScan {
    is_codex: bool,
    provider: String,
    size: u64,
    modified_ms: i64,
    offset: u64,
    state: CodexState,
    records: Vec<Record>,
}

const CACHE_VERSION: u32 = 1;
const KEEP_DAYS: i64 = 100;
const PRICES_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const PRICES_FRESH_MS: i64 = 24 * 3_600_000;

#[derive(Default, Serialize, Deserialize)]
struct ScanCache {
    version: u32,
    files: HashMap<String, FileScan>,
}

/// Keeps what it has read of each file, on disk too, so a scan reads only what was added. A transcript
/// since deleted still counts: what was spent stays spent.
#[derive(Default)]
pub struct UsageScanner {
    cache: Option<ScanCache>,
}

fn list_jsonl(dir: &Path, out: &mut Vec<(PathBuf, u64, i64)>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            list_jsonl(&e.path(), out);
        } else if e.file_name().to_string_lossy().ends_with(".jsonl") {
            if let Ok(meta) = e.metadata() {
                let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
                out.push((e.path(), meta.len(), modified));
            }
        }
    }
}

/// Up to the last complete line: a line still being written is read again next time.
fn read_new_lines(scan: &mut FileScan, file: &Path) -> std::io::Result<()> {
    let mut f = std::fs::File::open(file)?;
    f.seek(SeekFrom::Start(scan.offset))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else { return Ok(()) };
    for line in bytes[..end].split(|b| *b == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else { continue };
        if scan.is_codex {
            scan.records.extend(codex_record(line, &mut scan.state));
        } else {
            scan.records.extend(claude_records(line));
        }
    }
    scan.offset += end as u64 + 1;
    Ok(())
}

/// The list to use: a recent download, a new one, an older one, or the bundled copy.
fn price_doc(paths: &Paths) -> (Value, String) {
    let file = paths.data_dir.join("usage-prices.json");
    let age = now_ms() - file_mtime_ms(&file);
    let saved = || std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if file.exists() && age < PRICES_FRESH_MS {
        if let Some(doc) = saved() {
            return (doc, "fresh".into());
        }
    }
    let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().new_agent();
    let fetched = agent.get(PRICES_URL).call().ok().and_then(|mut r| r.body_mut().read_to_string().ok()).filter(|t| serde_json::from_str::<Value>(t).is_ok_and(|v| v.is_object()));
    if let Some(text) = fetched {
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, &text).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
        if let Ok(doc) = serde_json::from_str(&text) {
            return (doc, "fresh".into());
        }
    }
    if let Some(doc) = saved() {
        return (doc, "cached".into());
    }
    (serde_json::from_str(include_str!("../../../../usage-prices.json")).unwrap_or(Value::Null), "bundled".into())
}

impl UsageScanner {
    /// One row per hour, provider, model, speed and session, as the Electron app's scanner gives.
    pub fn scan(&mut self, paths: &Paths, homes: &[ProviderHome]) -> UsageData {
        let cache_file = paths.data_dir.join("usage-scan-cache-rs.json");
        let cache = self.cache.get_or_insert_with(|| {
            std::fs::read(&cache_file).ok().and_then(|b| serde_json::from_slice::<ScanCache>(&b).ok()).filter(|c| c.version == CACHE_VERSION).unwrap_or(ScanCache { version: CACHE_VERSION, files: HashMap::new() })
        });

        let mut roots = vec![(paths.claude_projects(), false, "claude".to_string()), (paths.codex_home.join("sessions"), true, "codex".to_string())];
        for h in homes {
            roots.push(if h.cli == "claude" { (h.home.join("projects"), false, h.id.clone()) } else { (h.home.join("sessions"), true, h.id.clone()) });
        }
        let mut scanned = 0;
        for (root, is_codex, provider) in roots {
            let mut found = Vec::new();
            list_jsonl(&root, &mut found);
            for (file, size, modified) in found {
                let scan = cache.files.entry(file.to_string_lossy().to_string()).or_default();
                scan.provider = provider.clone();
                if scan.size == size && scan.modified_ms == modified {
                    continue;
                }
                if size < scan.offset || scan.is_codex != is_codex {
                    *scan = FileScan { is_codex, provider: provider.clone(), ..FileScan::default() };
                }
                if read_new_lines(scan, &file).is_ok() {
                    scan.size = size;
                    scan.modified_ms = modified;
                    scanned += 1;
                }
            }
        }
        if scanned > 0 {
            if let Ok(bytes) = serde_json::to_vec(&*cache) {
                let tmp = cache_file.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &cache_file);
                }
            }
        }

        let since = (now_ms() - KEEP_DAYS * 86_400_000).div_euclid(60_000);
        let mut seen = HashSet::new();
        let mut sessions: HashMap<&str, usize> = HashMap::new();
        let mut rows: HashMap<(i64, &str, &str, u8, usize), Row> = HashMap::new();
        for scan in cache.files.values() {
            for r in &scan.records {
                // A message copied to another file counts once; a rollout that moved leaves a copy too.
                let key = r.key.clone().unwrap_or_else(|| format!("{}:{}:{}:{}:{}", r.session, r.t, r.input, r.cached, r.output));
                if !seen.insert(key) || r.t < since {
                    continue;
                }
                let next = sessions.len();
                let session = *sessions.entry(r.session.as_str()).or_insert(next);
                let hour = r.t.div_euclid(60) * 3_600_000;
                let row = rows.entry((hour, scan.provider.as_str(), r.model.as_str(), r.speed, session)).or_insert_with(|| Row {
                    hour,
                    provider: scan.provider.clone(),
                    model: r.model.clone(),
                    speed: r.speed,
                    session,
                    input: 0,
                    cached: 0,
                    write: 0,
                    output: 0,
                    reasoning: 0,
                });
                row.input += r.input;
                row.cached += r.cached;
                row.write += r.write;
                row.output += r.output;
                row.reasoning += r.reasoning;
            }
        }
        let rows: Vec<Row> = rows.into_values().collect();
        let (doc, pricing) = price_doc(paths);
        let table = price_table(&doc);
        let mut rates = HashMap::new();
        for r in &rows {
            if !rates.contains_key(&r.model) {
                if let Some(m) = rate_of(&table, &r.model) {
                    rates.insert(r.model.clone(), *m);
                }
            }
        }
        UsageData { rows, rates, pricing }
    }
}

// ---------- how the status bar writes them ----------

pub fn fmt_usd(n: f64) -> String {
    if n < 1000. {
        return format!("${n:.2}");
    }
    let text = format!("{n:.2}");
    let (whole, cents) = text.split_once('.').unwrap_or((&text, "00"));
    let digits: Vec<char> = whole.chars().collect();
    let grouped: String = digits.iter().enumerate().flat_map(|(i, c)| {
        let comma = i > 0 && (digits.len() - i) % 3 == 0;
        comma.then_some(',').into_iter().chain([*c])
    }).collect();
    format!("${grouped}.{cents}")
}

pub fn fmt_tokens(n: i64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let (d, unit) = if n >= 1_000_000_000 { (1e9, "B") } else if n >= 1_000_000 { (1e6, "M") } else { (1e3, "K") };
    let v = n as f64 / d;
    if v >= 100. {
        format!("{v:.0}{unit}")
    } else if v >= 10. {
        format!("{v:.1}{unit}")
    } else {
        format!("{v:.2}{unit}")
    }
}

pub fn until_text(ms: i64) -> String {
    let m = ((ms as f64 / 60_000.).round() as i64).max(1);
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 { format!("{h}h {}m", m % 60) } else { format!("{}d {}h", h / 24, h % 24) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claude_message_keeps_its_ids_and_parts() {
        let line = r#"{"type":"assistant","timestamp":"2026-11-05T01:02:03.000Z","sessionId":"s","requestId":"r","message":{"id":"m","model":"claude-x","usage":{"input_tokens":1,"cache_read_input_tokens":2,"cache_creation_input_tokens":3,"output_tokens":4}}}"#;
        let r = &claude_records(line)[0];
        assert_eq!((r.input, r.cached, r.write, r.output), (1, 2, 3, 4));
        assert_eq!(r.key.as_deref(), Some("m:r"));
    }

    #[test]
    fn codex_counts_a_request_once_and_takes_cached_out_of_input() {
        let mut s = CodexState { model: "gpt".into(), ..Default::default() };
        let line = |ts: &str, last: &str, total: &str| {
            format!(r#"{{"timestamp":"2026-11-05T01:00:{ts}.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{last},"total_token_usage":{total}}}}}}}"#)
        };
        let a = r#"{"input_tokens":10,"cached_input_tokens":4,"output_tokens":5,"total_tokens":15}"#;
        let first = codex_record(&line("01", a, a), &mut s).expect("a real request");
        assert_eq!((first.input, first.cached, first.output), (6, 4, 5));
        assert!(codex_record(&line("02", a, a), &mut s).is_none(), "re-emitted");
        let total = r#"{"input_tokens":20,"cached_input_tokens":8,"output_tokens":10,"total_tokens":30}"#;
        assert!(codex_record(&line("03", a, total), &mut s).is_some());
    }

    #[test]
    fn prices_answer_to_bare_names_and_skip_families() {
        let doc = json!({"anthropic/claude-x":{"input_cost_per_token":1.0,"output_cost_per_token":2.0},"opus":{"input_cost_per_token":1.0,"output_cost_per_token":2.0}});
        let table = price_table(&doc);
        assert_eq!(rate_of(&table, "claude-x[1m]").unwrap().standard.cache_read, 1.0);
        assert!(rate_of(&table, "opus").is_none());
    }

    #[test]
    fn the_usage_screen_gives_both_windows() {
        let screen = "Current session\n  ████ 48% used\n  Resets 7:50pm (Asia/Shanghai)\n\nCurrent week (all models)\n  ██ 51% used\n  Resets Nov 9, 7pm (Asia/Shanghai)\n";
        let u = parse_claude_usage_screen(screen).unwrap();
        assert_eq!(u.windows.len(), 2);
        assert_eq!((u.windows[0].used, u.windows[0].minutes), (48., 300));
        assert_eq!((u.windows[1].used, u.windows[1].minutes), (51., 10080));
        assert!(u.windows.iter().all(|w| w.resets_at.is_some()));
    }

    #[test]
    fn writes_numbers_like_the_status_bar() {
        assert_eq!(fmt_tokens(34_000_000), "34.0M");
        assert_eq!(fmt_tokens(1_230), "1.23K");
        assert_eq!(fmt_usd(15.5), "$15.50");
        assert_eq!(until_text(80 * 60_000), "1h 20m");
        assert_eq!(until_text(20 * 86_400_000 + 6 * 3_600_000), "20d 6h");
    }
}
