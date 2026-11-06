//! The Usage page, opened from the status bar: estimated cost and tokens by period, provider and
//! model, read from the agents' session files. Layout and calculations follow T3 Code's (MIT). Costs
//! are what the tokens would cost at API list prices, so on a subscription they show what it's worth
//! rather than what was paid.

use std::cell::Cell;

use chrono::{Datelike, Duration as Days, Local, TimeZone, Timelike};
use crew_core::usage::{Row, UsageData};
use gpui::{Hsla, MouseDownEvent, PathBuilder, TextAlign, TextRun, fill, point, size};

use super::overlays::{ToastKind, entry};
use super::*;
use crate::i18n::zh;

const HOUR: i64 = 3_600_000;
const PERIODS: [(i64, &str); 4] = [(1, "Past 24h"), (7, "7 days"), (30, "30 days"), (90, "90 days")];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Metric {
    Cost,
    Tokens,
    Limits,
}

pub(super) struct UsagePage {
    days: i64,
    metric: Metric,
    provider: Option<String>,
    by_model: bool,
    hover: Option<usize>,
    chart_bounds: Rc<Cell<Bounds<Pixels>>>,
}

pub(super) struct PriceEditor {
    model: String,
    summary: String,
    input: Entity<InputState>,
    output: Entity<InputState>,
    cache_read: Entity<InputState>,
    cache_write: Entity<InputState>,
}

#[derive(Default, Clone)]
struct Totals {
    input: i64,
    cached: i64,
    write: i64,
    output: i64,
    reasoning: i64,
    tokens: i64,
    cost: f64,
    savings: f64,
    unpriced: i64,
    cost_input: f64,
    cost_read: f64,
    cost_write: f64,
    cost_output: f64,
    sessions: HashSet<usize>,
}

struct Bucket {
    from: i64,
    to: i64,
    total: Totals,
    per_provider: HashMap<String, Totals>,
}

struct ModelLine {
    provider: String,
    model: String,
    total: Totals,
}

struct Summary {
    buckets: Vec<Bucket>,
    all: Totals,
    providers: Vec<(String, Totals)>,
    models: Vec<ModelLine>,
}

fn add_row(t: &mut Totals, row: &Row, config: &Config, data: &UsageData) {
    let tokens = row.tokens();
    t.input += row.input;
    t.cached += row.cached;
    t.write += row.write;
    t.output += row.output;
    t.reasoning += row.reasoning;
    t.tokens += tokens;
    t.sessions.insert(row.session);
    let Some(r) = data.rates_for(config, &row.model, row.speed) else {
        t.unpriced += tokens;
        return;
    };
    let (input, read, write, output) = (row.input as f64 * r.input, row.cached as f64 * r.cache_read, row.write as f64 * r.cache_write, row.output as f64 * r.output);
    t.cost += input + read + write + output;
    t.savings += row.cached as f64 * (r.input - r.cache_read);
    t.cost_input += input;
    t.cost_read += read;
    t.cost_write += write;
    t.cost_output += output;
}

/// The time slots of a period: hours for a day, otherwise local days.
fn buckets_for(days: i64) -> Vec<Bucket> {
    let empty = |from: i64, to: i64| Bucket { from, to, total: Totals::default(), per_provider: HashMap::new() };
    if days == 1 {
        let end = usage::now_ms() / HOUR * HOUR;
        return (0..24).map(|i| end - (23 - i) * HOUR).map(|at| empty(at, at + HOUR)).collect();
    }
    let today = Local::now().date_naive();
    let midnight = |d: chrono::NaiveDate| d.and_hms_opt(0, 0, 0).and_then(|t| Local.from_local_datetime(&t).earliest()).map_or(0, |t| t.timestamp_millis());
    (0..days).rev().map(|i| today - Days::days(i)).map(|d| empty(midnight(d), midnight(d + Days::days(1)))).collect()
}

fn summarize(data: &UsageData, config: &Config, days: i64, provider: Option<&str>) -> Summary {
    let mut buckets = buckets_for(days);
    let since = buckets[0].from;
    let mut all = Totals::default();
    let mut providers: HashMap<String, Totals> = HashMap::new();
    let mut models: HashMap<(String, String), Totals> = HashMap::new();
    for row in data.rows.iter().filter(|r| r.hour >= since) {
        add_row(providers.entry(row.provider.clone()).or_default(), row, config, data);
        if provider.is_some_and(|p| p != row.provider) {
            continue;
        }
        add_row(&mut all, row, config, data);
        add_row(models.entry((row.provider.clone(), row.model.clone())).or_default(), row, config, data);
        // Rows are hourly, so a row's hour decides its slot.
        if let Some(b) = buckets.iter_mut().find(|b| row.hour >= b.from && row.hour < b.to) {
            add_row(&mut b.total, row, config, data);
            add_row(b.per_provider.entry(row.provider.clone()).or_default(), row, config, data);
        }
    }
    Summary {
        buckets,
        all,
        providers: providers.into_iter().collect(),
        models: models.into_iter().map(|((provider, model), total)| ModelLine { provider, model, total }).collect(),
    }
}

fn nice_max(v: f64) -> f64 {
    if v <= 0. {
        return 1.;
    }
    let p = 10f64.powf(v.log10().floor());
    [1., 1.5, 2., 3., 4., 5., 7.5, 10.].iter().map(|m| m * p).find(|x| *x >= v).unwrap_or(10. * p)
}

/// The slopes of a smooth line that never overshoots its points (monotone cubic).
fn monotone_slopes(points: &[(f32, f32)]) -> Vec<f32> {
    let n = points.len();
    if n < 2 {
        return vec![0.; n];
    }
    let m: Vec<f32> = points.windows(2).map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0)).collect();
    let mut t = vec![m[0]];
    for i in 1..n - 1 {
        t.push(if m[i - 1] * m[i] <= 0. { 0. } else { 2. * m[i - 1] * m[i] / (m[i - 1] + m[i]) });
    }
    t.push(m[n - 2]);
    t
}

fn fmt_share(s: Option<f64>) -> String {
    s.map_or("—".to_string(), |s| format!("{:.1}%", s * 100.))
}

fn bucket_label(b: &Bucket, days: i64) -> String {
    let Some(d) = Local.timestamp_millis_opt(b.from).single() else { return String::new() };
    if days == 1 { format!("{:02}:00", d.hour()) } else { format!("{}/{}", d.month(), d.day()) }
}

fn sessions_text(n: usize) -> String {
    if zh() { format!("{n} 个会话") } else if n == 1 { "1 session".to_string() } else { format!("{n} sessions") }
}

impl CrewApp {
    pub(super) fn open_usage_page(&mut self, window: &Window, cx: &mut Context<Self>) {
        let days = self.config.f32("usageDays", 1.) as i64;
        let metric = match self.config.str("usageMetric") {
            "tokens" => Metric::Tokens,
            "limits" => Metric::Limits,
            _ => Metric::Cost,
        };
        self.usage_page = Some(UsagePage {
            days: if [1, 7, 30, 90].contains(&days) { days } else { 1 },
            metric,
            provider: None,
            by_model: true,
            hover: None,
            chart_bounds: Rc::default(),
        });
        self.refresh_usage(true, window, cx);
        cx.notify();
    }

    /// From a plan's item in the status bar: its limits.
    pub(super) fn open_usage_limits(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.open_usage_page(window, cx);
        if let Some(page) = self.usage_page.as_mut() {
            page.metric = Metric::Limits;
        }
    }

    /// From today's cost in the status bar: costs or tokens, whichever was looked at last.
    pub(super) fn open_usage_costs(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.open_usage_page(window, cx);
        if let Some(page) = self.usage_page.as_mut().filter(|p| p.metric == Metric::Limits) {
            page.metric = Metric::Cost;
        }
    }

    fn provider_label(&self, id: &str) -> String {
        match id {
            "claude" => "Claude Code".into(),
            "codex" => "Codex".into(),
            other => self.providers.iter().find(|p| p.id == other).map_or_else(|| other.to_string(), |p| p.name.clone()),
        }
    }

    fn provider_color(&self, id: &str) -> Hsla {
        match id {
            "claude" => self.colors.accent,
            "codex" => self.colors.text,
            other => {
                let h = other.chars().fold(0u32, |h, c| (h * 31 + c as u32) % 360);
                hsla(h as f32 / 360., 0.55, 0.48, 1.)
            }
        }
    }

    fn provider_mark(&self, id: &str) -> AnyElement {
        match id {
            "claude" | "codex" => self.agent_badge(id, 14.),
            other => self.providers.iter().find(|p| p.id == other).map_or_else(|| div().size(px(16.)).into_any_element(), |p| self.provider_icon(p)),
        }
    }

    fn segmented<V: Clone + PartialEq + 'static>(&self, id: &'static str, options: Vec<(V, &'static str)>, value: &V, pick: fn(&mut UsagePage, V), cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(8.))
            .bg(c.hover)
            .children(options.into_iter().enumerate().map(|(i, (v, label))| {
                let on = v == *value;
                div()
                    .id((id, i))
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(if on { c.text } else { c.muted })
                    .when(on, |d| d.bg(c.main_bg))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if let Some(p) = app.usage_page.as_mut() {
                            pick(p, v.clone());
                        }
                        app.save_usage_choices();
                        cx.notify();
                    }))
                    .child(t(label))
            }))
            .into_any_element()
    }

    fn save_usage_choices(&mut self) {
        let Some(p) = &self.usage_page else { return };
        let (days, metric) = (p.days, match p.metric {
            Metric::Cost => "cost",
            Metric::Tokens => "tokens",
            Metric::Limits => "limits",
        });
        if self.config.f32("usageDays", 1.) as i64 != days {
            self.save_setting("usageDays", json!(days));
        }
        if self.config.str("usageMetric") != metric {
            self.save_setting("usageMetric", json!(metric));
        }
    }

    fn render_usage_chart(&self, sum: &Summary, ids: &[String], page: &UsagePage, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let cost = page.metric == Metric::Cost;
        let value = |t: &Totals| if cost { t.cost } else { t.tokens as f64 };
        let series: Vec<(String, Hsla, Vec<f64>)> = ids
            .iter()
            .map(|id| (self.provider_label(id), self.provider_color(id), sum.buckets.iter().map(|b| b.per_provider.get(id).map_or(0., value)).collect()))
            .collect();
        let top_value = series.iter().flat_map(|s| s.2.iter().copied()).fold(0., f64::max);
        let max = nice_max(top_value);
        let labels: Vec<String> = sum.buckets.iter().map(|b| bucket_label(b, page.days)).collect();
        let count = labels.len();
        let fmt = move |v: f64| if v == 0. { "0".to_string() } else if cost { usage::fmt_usd(v) } else { usage::fmt_tokens(v as i64) };
        let hover = page.hover;
        let tip = hover.map(|i| {
            let lines: Vec<(Hsla, String, String)> = series
                .iter()
                .filter(|s| s.2[i] > 0. || series.len() == 1)
                .map(|s| (s.1, s.0.clone(), fmt(s.2[i])))
                .collect();
            (i, labels[i].clone(), lines)
        });
        let (grid, muted, background) = (c.border, c.muted, c.main_bg);
        let bounds_cell = page.chart_bounds.clone();
        let paint_series = series.clone();
        let paint_labels = labels.clone();
        const LEFT: f32 = 56.;
        const RIGHT: f32 = 4.;
        const TOP: f32 = 8.;
        const BOTTOM: f32 = 26.;
        let chart = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, _, window, cx| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let x_of = |i: usize| LEFT + if count < 2 { 0. } else { i as f32 / (count - 1) as f32 * (w - LEFT - RIGHT) };
                let y_of = |v: f64| TOP + (1. - (v / max) as f32) * (h - TOP - BOTTOM);
                let at = |x: f32, y: f32| bounds.origin + point(px(x), px(y));
                let font = window.text_style().font();
                let text = |window: &mut Window, s: &str, x: f32, y: f32, align: TextAlign, cx: &mut gpui::App| {
                    let run = TextRun { len: s.len(), font: font.clone(), color: muted, background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(SharedString::from(s.to_string()), px(11.), &[run], None);
                    let left = match align {
                        TextAlign::Right => x - f32::from(line.width),
                        TextAlign::Center => x - f32::from(line.width) / 2.,
                        _ => x,
                    };
                    let _ = line.paint(at(left, y), px(14.), TextAlign::Left, None, window, cx);
                };
                for f in [0., 1. / 3., 2. / 3., 1.] {
                    let y = y_of(max * f);
                    window.paint_quad(fill(Bounds::new(at(LEFT, y), size(px(w - LEFT - RIGHT), px(1.))), grid));
                    text(window, &fmt(max * f), LEFT - 10., y - 7., TextAlign::Right, cx);
                }
                if count > 0 {
                    for (i, align) in [(0, TextAlign::Left), ((count - 1) / 2, TextAlign::Center), (count - 1, TextAlign::Right)] {
                        text(window, &paint_labels[i], x_of(i), h - 18., align, cx);
                    }
                }
                for (_, color, values) in &paint_series {
                    let points: Vec<(f32, f32)> = values.iter().enumerate().map(|(i, v)| (x_of(i), y_of(*v))).collect();
                    if points.len() < 2 {
                        continue;
                    }
                    let slopes = monotone_slopes(&points);
                    let curve = |b: &mut PathBuilder| {
                        for i in 0..points.len() - 1 {
                            let (x0, y0) = points[i];
                            let (x1, y1) = points[i + 1];
                            let k = (x1 - x0) / 3.;
                            b.cubic_bezier_to(at(x1, y1), at(x0 + k, y0 + k * slopes[i]), at(x1 - k, y1 - k * slopes[i + 1]));
                        }
                    };
                    let mut area = PathBuilder::fill();
                    area.move_to(at(points[0].0, points[0].1));
                    curve(&mut area);
                    area.line_to(at(points[points.len() - 1].0, y_of(0.)));
                    area.line_to(at(points[0].0, y_of(0.)));
                    area.close();
                    if let Ok(path) = area.build() {
                        window.paint_path(path, color.opacity(0.1));
                    }
                    let mut line = PathBuilder::stroke(px(2.));
                    line.move_to(at(points[0].0, points[0].1));
                    curve(&mut line);
                    if let Ok(path) = line.build() {
                        window.paint_path(path, *color);
                    }
                }
                if let Some(i) = hover {
                    window.paint_quad(fill(Bounds::new(at(x_of(i), TOP), size(px(1.), px(h - TOP - BOTTOM))), muted));
                }
            },
        )
        .size_full();
        let cell = page.chart_bounds.clone();
        div()
            .id("usage-chart")
            .relative()
            .w_full()
            .h(px(240.))
            .on_mouse_move(cx.listener(move |app, e: &MouseMoveEvent, _, cx| {
                let b = cell.get();
                let width = f32::from(b.size.width) - LEFT - RIGHT;
                if width <= 0. || count < 2 {
                    return;
                }
                let x = f32::from(e.position.x - b.origin.x);
                let inside = b.contains(&e.position);
                let index = inside.then(|| (((x - LEFT) / width * (count - 1) as f32).round() as i64).clamp(0, count as i64 - 1) as usize);
                if let Some(p) = app.usage_page.as_mut() {
                    if p.hover != index {
                        p.hover = index;
                        cx.notify();
                    }
                }
            }))
            .child(chart)
            .when_some(tip, |d, (i, label, lines)| {
                let share = if count < 2 { 0. } else { i as f32 / (count - 1) as f32 };
                d.child(
                    div()
                        .absolute()
                        .top(px(8.))
                        .when(share < 0.6, |d| d.left(relative(share.max(0.08))))
                        .when(share >= 0.6, |d| d.right(relative((1. - share).max(0.02))))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(background)
                        .border_1()
                        .border_color(grid)
                        .shadow_md()
                        .text_size(px(12.))
                        .child(div().text_color(muted).child(label))
                        .children(lines.into_iter().map(|(color, name, value)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(div().size(px(7.)).rounded_full().bg(color))
                                .child(name)
                                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(value))
                        })),
                )
            })
            .into_any_element()
    }

    fn share_bar(&self, segments: Vec<(&'static str, f64, Hsla, String)>) -> AnyElement {
        let c = self.colors;
        let total: f64 = segments.iter().map(|s| s.1).sum();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .h(px(8.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(c.hover)
                    .when(total > 0., |d| {
                        d.children(segments.iter().filter(|s| s.1 > 0.).map(|s| div().h_full().w(relative((s.1 / total) as f32)).bg(s.2)))
                    }),
            )
            .child(div().flex().flex_wrap().gap(px(14.)).text_size(px(12.5)).children(segments.iter().map(|s| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(div().size(px(8.)).rounded(px(2.)).bg(s.2))
                    .child(div().text_color(c.muted).child(t(s.0)))
                    .child(s.3.clone())
            })))
            .into_any_element()
    }

    fn edit_model_price(&mut self, line_model: &str, total: &Totals, window: &mut Window, cx: &mut Context<Self>) {
        let override_price = self.config.raw().get("usagePrices").and_then(|p| p.get(line_model)).cloned();
        let list = self.usage_data.as_ref().and_then(|d| d.rates_for(&self.config, line_model, 0));
        let per_million = |v: Option<f64>| v.map_or(String::new(), |v| format!("{}", ((v * 1e6) * 10000.).round() / 10000.));
        let saved = |key: &str| override_price.as_ref().and_then(|p| p.get(key)).and_then(Value::as_f64).map(|v| v.to_string());
        let value = |key: &str, rate: Option<f64>| saved(key).unwrap_or_else(|| per_million(rate));
        let field = |text: String, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).default_value(text));
        let input_tokens = total.input + total.cached + total.write;
        let mut parts = vec![if zh() { format!("{} 令牌", usage::fmt_tokens(total.tokens)) } else { format!("{} tokens", usage::fmt_tokens(total.tokens)) }];
        if input_tokens > 0 {
            parts.push(format!("{} {:.1}%", t("Cache hit rate"), total.cached as f64 / input_tokens as f64 * 100.));
        }
        if total.reasoning > 0 {
            parts.push(format!("{} {}", t("Reasoning"), usage::fmt_tokens(total.reasoning)));
        }
        if total.unpriced == 0 && total.tokens > 0 {
            parts.push(format!("{} / M", usage::fmt_usd(total.cost / total.tokens as f64 * 1e6)));
        }
        let input = field(value("input", list.map(|r| r.input)), window, cx);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.price_editor = Some(PriceEditor {
            model: line_model.to_string(),
            summary: parts.join(" · "),
            input,
            output: field(value("output", list.map(|r| r.output)), window, cx),
            cache_read: field(value("cacheRead", list.map(|r| r.cache_read)), window, cx),
            cache_write: field(value("cacheWrite", list.map(|r| r.cache_write)), window, cx),
        });
        cx.notify();
    }

    fn save_model_price(&mut self, cx: &mut Context<Self>) {
        let Some(e) = &self.price_editor else { return };
        let number = |s: &Entity<InputState>| s.read(cx).value().trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.);
        let (input, output, read, write) = (number(&e.input), number(&e.output), number(&e.cache_read), number(&e.cache_write));
        let model = e.model.clone();
        // Prices that are just the list's aren't an override.
        let listed = self.usage_data.as_ref().and_then(|d| d.rates.get(&model)).map(|m| m.standard);
        let same = listed.is_some_and(|l| {
            [(input, l.input), (output, l.output), (read, l.cache_read), (write, l.cache_write)].iter().all(|(v, r)| v.is_some_and(|v| (v - r * 1e6).abs() < 1e-3))
        });
        let mut prices = match self.config.raw().get("usagePrices") {
            Some(Value::Object(m)) => m.clone(),
            _ => serde_json::Map::new(),
        };
        match (input, output) {
            (Some(i), Some(o)) if !same => {
                let mut price = serde_json::Map::new();
                price.insert("input".into(), json!(i));
                price.insert("output".into(), json!(o));
                if let Some(r) = read {
                    price.insert("cacheRead".into(), json!(r));
                }
                if let Some(w) = write {
                    price.insert("cacheWrite".into(), json!(w));
                }
                prices.insert(model, Value::Object(price));
            }
            _ => {
                prices.remove(&model);
            }
        }
        self.save_setting("usagePrices", Value::Object(prices));
        self.price_editor = None;
        self.recompute_today();
        cx.notify();
    }

    pub(super) fn render_price_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let e = self.price_editor.as_ref()?;
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.heading(e.model.clone()))
            .child(div().text_size(px(12.5)).text_color(c.muted).child(e.summary.clone()))
            .child(div().text_size(px(12.5)).text_color(c.muted).child(t("Prices are USD per million tokens. Leave them all empty to use the price list.")))
            .child(div().flex().gap(px(10.)).child(self.field(t("Input"), Input::new(&e.input).small(), None)).child(self.field(t("Output"), Input::new(&e.output).small(), None)))
            .child(div().flex().gap(px(10.)).child(self.field(t("Cache read"), Input::new(&e.cache_read).small(), None)).child(self.field(t("Cache write"), Input::new(&e.cache_write).small(), None)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(self.button("price-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                        app.price_editor = None;
                        cx.notify();
                    })))
                    .child(self.button("price-save", t("Save"), true).on_click(cx.listener(|app, _, _, cx| app.save_model_price(cx)))),
            );
        Some(self.modal("price-editor", 460., body))
    }

    fn render_limits(&self) -> AnyElement {
        let c = self.colors;
        let now = usage::now_ms();
        let reset_text = |at: Option<i64>| {
            let Some(at) = at else { return String::new() };
            let mins = ((at - now).max(0) as f64 / 60000.).round() as i64;
            let text = if mins >= 1440 { format!("{}d {}h", mins / 1440, mins % 1440 / 60) } else if mins >= 60 { format!("{}h {}m", mins / 60, mins % 60) } else { format!("{mins}m") };
            format!("{} {text}", t("Resets in"))
        };
        let groups: Vec<AnyElement> = [("claude", &self.plan_claude), ("codex", &self.plan_codex)]
            .into_iter()
            .filter_map(|(agent, plan)| plan.as_ref().filter(|p| !p.windows.is_empty()).map(|p| (agent, p)))
            .map(|(agent, plan)| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(self.agent_badge(agent, 14.))
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.provider_label(agent)))
                            .when_some(plan.plan.clone(), |d, name| d.child(div().text_color(c.muted).child(name))),
                    )
                    .children(plan.windows.iter().map(|w| {
                        let pct = w.used.round().clamp(0., 100.);
                        let color: Hsla = if pct >= 90. { gpui::rgb(0xc4462d).into() } else if pct >= 70. { gpui::rgb(0xc98a12).into() } else { c.running };
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_size(px(13.))
                                    .child(window_name(w.minutes))
                                    .child(div().flex().gap(px(6.)).child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("{pct}%"))).child(div().text_color(c.muted).child(reset_text(w.resets_at)))),
                            )
                            .child(div().h(px(8.)).rounded_full().bg(c.hover).overflow_hidden().child(div().h_full().w(relative(pct as f32 / 100.)).bg(color)))
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .p(px(24.))
            .map(|d| if groups.is_empty() { d.child(div().text_color(c.muted).child(t("Checking plan usage…"))) } else { d.children(groups) })
            .into_any_element()
    }

    fn stat(&self, label: &'static str, value: String) -> AnyElement {
        let c = self.colors;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(110.))
            .gap(px(2.))
            .child(div().text_size(px(12.)).text_color(c.muted).child(t(label)))
            .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child(value))
            .into_any_element()
    }

    fn table_cell(width: f32, right: bool) -> gpui::Div {
        div().flex_none().w(px(width)).when(right, |d| d.flex().justify_end())
    }

    fn render_breakdown(&self, sum: &Summary, page: &UsagePage, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let cost = page.metric == Metric::Cost;
        let value = |t: &Totals| if cost { t.cost } else { t.tokens as f64 };
        let whole = { let v = value(&sum.all); if v > 0. { v } else { 1. } };
        let hourly = page.days == 1;
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .pb(px(8.))
            .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::MEDIUM).child(t("Breakdown")))
            .child(self.segmented("usage-breakdown", vec![(true, "Model"), (false, if hourly { "Hour" } else { "Day" })], &page.by_model, |p, v| p.by_model = v, cx));
        let header = |cells: Vec<(&'static str, Option<f32>)>| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .py(px(4.))
                .border_b_1()
                .border_color(c.border)
                .text_size(px(12.))
                .text_color(c.muted)
                .children(cells.into_iter().map(|(label, width)| match width {
                    Some(w) => Self::table_cell(w, true).child(t(label)),
                    None => div().flex_1().min_w(px(0.)).child(t(label)),
                }))
        };
        if !page.by_model {
            let rows = sum.buckets.iter().rev().filter(|b| b.total.tokens > 0).map(|b| {
                let label = Local.timestamp_millis_opt(b.from).single().map_or(String::new(), |d| if hourly { d.format("%-m/%-d %H:00").to_string() } else { d.format("%Y/%-m/%-d").to_string() });
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(5.))
                    .text_size(px(13.))
                    .child(div().flex_1().min_w(px(0.)).child(label))
                    .child(Self::table_cell(80., true).child(usage::fmt_usd(b.total.cost)))
                    .child(Self::table_cell(70., true).text_color(c.muted).child(fmt_share(Some(value(&b.total) / whole))))
                    .child(Self::table_cell(80., true).text_color(c.muted).child(usage::fmt_tokens(b.total.tokens)))
                    .child(Self::table_cell(70., true).text_color(c.muted).child(b.total.sessions.len().to_string()))
            });
            return div()
                .flex()
                .flex_col()
                .child(head)
                .child(header(vec![(if hourly { "Hour" } else { "Day" }, None), ("Cost", Some(80.)), ("Share", Some(70.)), ("Tokens", Some(80.)), ("Sessions", Some(70.))]))
                .children(rows)
                .into_any_element();
        }
        let mut order: Vec<&ModelLine> = sum.models.iter().collect();
        order.sort_by(|a, b| b.total.tokens.cmp(&a.total.tokens).then(b.total.cost.total_cmp(&a.total.cost)));
        let rows: Vec<AnyElement> = order
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let unknown = m.total.unpriced >= m.total.tokens;
                let share = if cost && unknown { None } else { Some(value(&m.total) / whole) };
                let color = self.provider_color(&m.provider);
                let your_price = self.config.raw().get("usagePrices").is_some_and(|p| p.get(&m.model).is_some());
                let (model, total) = (m.model.clone(), m.total.clone());
                div()
                    .id(("usage-model", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(5.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .rounded(px(6.))
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |app, _, window, cx| app.edit_model_price(&model, &total, window, cx)))
                    .child(Self::table_cell(24., false).text_color(c.muted).child((i + 1).to_string()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(3.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(self.provider_mark(&m.provider))
                                    .child(div().min_w(px(0.)).truncate().font_family("Cascadia Mono").text_size(px(12.5)).child(m.model.clone()))
                                    .when(your_price, |d| d.child(div().flex_none().px(px(6.)).rounded(px(4.)).bg(c.hover).text_size(px(11.)).text_color(c.muted).child(t("Your price")))),
                            )
                            .child(div().h(px(3.)).rounded_full().w(relative(share.unwrap_or(0.).max(0.01) as f32)).bg(color)),
                    )
                    .child(Self::table_cell(80., true).map(|d| if unknown { d.text_color(c.accent).child(t("Set price")) } else { d.child(usage::fmt_usd(m.total.cost)) }))
                    .child(Self::table_cell(70., true).text_color(c.muted).child(fmt_share(share)))
                    .child(Self::table_cell(80., true).text_color(c.muted).child(usage::fmt_tokens(m.total.tokens)))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .child(head)
            .child(header(vec![("#", Some(24.)), ("Model", None), ("Cost", Some(80.)), ("Share", Some(70.)), ("Tokens", Some(80.))]))
            .children(rows)
            .into_any_element()
    }

    fn render_usage_body(&self, data: &UsageData, page: &UsagePage, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let sum = summarize(data, &self.config, page.days, page.provider.as_deref());
        let all = &sum.all;
        let cost = page.metric == Metric::Cost;
        let value = |t: &Totals| if cost { t.cost } else { t.tokens as f64 };
        let whole = { let v: f64 = sum.providers.iter().map(|p| value(&p.1)).sum(); if v > 0. { v } else { 1. } };
        let unpriced = if all.tokens > 0 { all.unpriced as f64 / all.tokens as f64 } else { 0. };
        let mut shown: Vec<&(String, Totals)> = sum.providers.iter().filter(|p| page.provider.as_ref().is_none_or(|x| *x == p.0)).collect();
        shown.sort_by(|a, b| value(&b.1).total_cmp(&value(&a.1)));
        let ids: Vec<String> = if shown.is_empty() { vec!["claude".to_string()] } else { shown.iter().map(|p| p.0.clone()).collect() };

        let provider_rows: Vec<AnyElement> = shown
            .iter()
            .map(|(id, totals)| {
                let on = page.provider.as_deref() == Some(id.as_str());
                let pick = id.clone();
                let line = if cost {
                    format!("{} {} · {} tokens", fmt_share(Some(totals.cost / whole)), t("of cost"), usage::fmt_tokens(totals.tokens))
                } else {
                    format!("{} {} · {}", fmt_share(Some(totals.tokens as f64 / whole)), t("of tokens"), usage::fmt_usd(totals.cost))
                };
                div()
                    .id(SharedString::from(format!("usage-provider-{id}")))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(c.hover))
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if let Some(p) = app.usage_page.as_mut() {
                            p.provider = if p.provider.as_deref() == Some(pick.as_str()) { None } else { Some(pick.clone()) };
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(13.))
                            .child(div().flex_none().size(px(7.)).rounded_full().bg(self.provider_color(id)))
                            .child(self.provider_mark(id))
                            .child(self.provider_label(id))
                            .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(12.)).text_color(c.muted).child(sessions_text(totals.sessions.len())))
                            .child(if cost { usage::fmt_usd(totals.cost) } else { usage::fmt_tokens(totals.tokens) }),
                    )
                    .child(div().text_size(px(12.)).text_color(c.muted).child(line))
                    .into_any_element()
            })
            .collect();
        let summary = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(300.))
            .gap(px(4.))
            .child(div().text_size(px(32.)).font_weight(gpui::FontWeight::SEMIBOLD).child(if cost { usage::fmt_usd(all.cost) } else { usage::fmt_tokens(all.tokens) }))
            .child(div().text_size(px(12.5)).text_color(c.muted).child(if cost { format!("{} · {}", sessions_text(all.sessions.len()), t("API estimate")) } else { sessions_text(all.sessions.len()) }))
            .when(cost && unpriced > 0., |d| {
                let share = if unpriced < 0.001 { "<0.1%".to_string() } else { fmt_share(Some(unpriced)) };
                d.child(div().text_size(px(12.5)).text_color(c.accent).child(format!("{} {share}", t("Leaves out unpriced models:"))))
            })
            .when(cost && data.pricing == "bundled", |d| d.child(div().text_size(px(12.)).text_color(c.muted).child(t("Using the built-in price list: the latest one could not be fetched."))))
            .child(div().pt(px(6.)).flex().flex_col().gap(px(2.)).children(provider_rows));

        let title = match (page.days == 1, cost) {
            (true, true) => t("Hourly cost"),
            (true, false) => t("Hourly processed tokens"),
            (false, true) => t("Daily cost"),
            (false, false) => t("Daily processed tokens"),
        };
        let section_title = |text: &'static str| div().pb(px(8.)).text_size(px(14.)).font_weight(gpui::FontWeight::MEDIUM).child(t(text));
        let (ink1, ink2, ink3, ink4) = (c.text.opacity(0.6), c.text.opacity(0.3), c.text.opacity(0.72), c.text);
        let types: Vec<(&'static str, f64, Hsla, String)> = if cost {
            vec![
                ("Input", all.cost_input, ink1, usage::fmt_usd(all.cost_input)),
                ("Cache read", all.cost_read, ink2, usage::fmt_usd(all.cost_read)),
                ("Cache write", all.cost_write, ink3, usage::fmt_usd(all.cost_write)),
                ("Output", all.cost_output, ink4, usage::fmt_usd(all.cost_output)),
            ]
        } else {
            vec![
                ("Input", all.input as f64, ink1, usage::fmt_tokens(all.input)),
                ("Cache read", all.cached as f64, ink2, usage::fmt_tokens(all.cached)),
                ("Cache write", all.write as f64, ink3, usage::fmt_tokens(all.write)),
                ("Output", all.output as f64, ink4, usage::fmt_tokens(all.output)),
            ]
        };
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .p(px(24.))
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(summary)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().pb(px(10.)).text_size(px(14.)).font_weight(gpui::FontWeight::MEDIUM).child(title))
                            .child(self.render_usage_chart(&sum, &ids, page, cx)),
                    ),
            )
            .child(
                div().flex().flex_col().child(section_title("Totals")).child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(12.))
                        .child(self.stat("Processed tokens", usage::fmt_tokens(all.tokens)))
                        .child(self.stat("Cached input", usage::fmt_tokens(all.cached)))
                        .child(self.stat("Uncached input", usage::fmt_tokens(all.input)))
                        .child(self.stat("Output", usage::fmt_tokens(all.output)))
                        .child(self.stat("Cache savings", usage::fmt_usd(all.savings))),
                ),
            )
            .child(div().flex().flex_col().child(section_title(if cost { "Cost by type" } else { "Tokens by type" })).child(self.share_bar(types)))
            .child(self.render_breakdown(&sum, page, cx))
            .into_any_element()
    }

    pub(super) fn render_usage_page(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let page = self.usage_page.as_ref()?;
        let label = page.provider.as_deref().map_or_else(|| t("All providers").to_string(), |p| self.provider_label(p));
        let known: Vec<String> = {
            let mut ids: Vec<String> = self.usage_data.as_ref().map_or(Vec::new(), |d| d.rows.iter().map(|r| r.provider.clone()).collect());
            ids.sort();
            ids.dedup();
            ids
        };
        let picker: Vec<(String, String)> = known.iter().map(|id| (id.clone(), self.provider_label(id))).collect();
        let body = if page.metric == Metric::Limits {
            self.render_limits()
        } else if let Some(data) = &self.usage_data {
            self.render_usage_body(data, page, cx)
        } else {
            div().p(px(24.)).text_color(c.muted).child(t("Reading session files…")).into_any_element()
        };
        let metrics = vec![(Metric::Cost, "Cost"), (Metric::Tokens, "Tokens"), (Metric::Limits, "Limits")];
        Some(
            div()
                .id("usage-page")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.35))
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|app, _, _, cx| {
                    app.usage_page = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("usage-card")
                        .flex()
                        .flex_col()
                        .w(relative(0.9))
                        .max_w(px(1040.))
                        .h(relative(0.9))
                        .max_h(px(820.))
                        .rounded(px(12.))
                        .overflow_hidden()
                        .bg(c.main_bg)
                        .border_1()
                        .border_color(c.border)
                        .shadow_md()
                        .text_color(c.text)
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(8.))
                                .px(px(20.))
                                .py(px(10.))
                                .border_b_1()
                                .border_color(c.border)
                                .text_size(px(13.))
                                .child(div().text_color(c.muted).child(t("Usage")))
                                .child(div().text_color(c.muted).child("/"))
                                .child(
                                    div()
                                        .id("usage-picker")
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |app, e: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            let mut items = vec![entry(t("All providers"), |app, _, _| {
                                                if let Some(p) = app.usage_page.as_mut() {
                                                    p.provider = None;
                                                }
                                            })];
                                            items.extend(picker.iter().map(|(id, label)| {
                                                let id = id.clone();
                                                entry(label.clone(), move |app, _, _| {
                                                    if let Some(p) = app.usage_page.as_mut() {
                                                        p.provider = Some(id.clone());
                                                    }
                                                })
                                            }));
                                            app.open_menu(e.position, items, cx);
                                        }))
                                        .child(label)
                                        .child(Icon::new(IconName::ChevronDown).size(px(13.))),
                                )
                                .child(div().flex_1())
                                .when(page.metric != Metric::Limits, |d| d.child(self.segmented("usage-days", PERIODS.to_vec(), &page.days, |p, v| { p.days = v; p.hover = None; }, cx)))
                                .child(self.segmented("usage-metric", metrics, &page.metric, |p, v| p.metric = v, cx))
                                .child(self.icon_button("usage-page-refresh", IconName::RefreshCw).p(px(4.)).on_click(cx.listener(|app, _, window, cx| {
                                    app.refresh_usage(true, window, cx);
                                    app.toast(t("Reading session files…"), ToastKind::Plain, cx);
                                })))
                                .child(self.icon_button("usage-page-close", IconName::Close).p(px(4.)).on_click(cx.listener(|app, _, _, cx| {
                                    app.usage_page = None;
                                    cx.notify();
                                }))),
                        )
                        .child(div().id("usage-scroll").flex_1().min_h(px(0.)).overflow_y_scroll().child(body)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_limits_for_the_chart() {
        assert_eq!(nice_max(0.), 1.);
        assert_eq!(nice_max(1.2), 1.5);
        assert_eq!(nice_max(46.), 50.);
        assert_eq!(nice_max(7000.), 7500.);
    }

    #[test]
    fn the_curve_flattens_at_a_turning_point() {
        let s = monotone_slopes(&[(0., 0.), (1., 2.), (2., 1.)]);
        assert_eq!(s[1], 0.);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn a_day_has_24_hourly_slots_and_a_week_seven_days() {
        assert_eq!(buckets_for(1).len(), 24);
        let week = buckets_for(7);
        assert_eq!(week.len(), 7);
        assert!(week[6].from > week[0].from && week[6].to > usage::now_ms());
    }
}
