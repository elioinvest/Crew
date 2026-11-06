//! Draws the Git panel. The list takes a narrow panel beside the terminal. The diff and the history
//! need more room, and lie over the terminal instead of squeezing it: resizing one makes its whole
//! history reflow and its agent redraw the screen, which would lag on every switch.

use std::ops::Range;

use crew_core::git::{self, Change, Commit, DiffLine, Entry, LineKind, RefKind};
use gpui::{ClickEvent, HighlightStyle, Hsla, MouseDownEvent, PathBuilder, StyledText, fill, point, size, uniform_list};

use super::git_panel::{BarKind, DiffRow, GitView, HunkActions, fmt_date, short, split_path, step};
use super::*;

const ROW_H: f32 = 24.;
const DIFF_ROW_H: f32 = 21.;
const LANE_W: f32 = 14.;
const LANE_COLORS: [u32; 8] = [0x4a8fd8, 0xd97757, 0x3fa45b, 0xa46fd6, 0xd6a23a, 0x3aa8a8, 0xd65a8a, 0x7f8f3a];

fn status_color(s: char) -> Hsla {
    gpui::rgb(match s {
        'M' | 'T' => 0xd6962c,
        'A' => 0x3fa45b,
        'D' => 0xd0533a,
        'R' | 'C' => 0x4a8fd8,
        'U' => 0xb3362a,
        _ => 0x8a8580,
    })
    .into()
}

fn status_mark(s: char) -> &'static str {
    match s {
        'M' | 'T' => "•",
        'A' | '?' => "+",
        'D' => "−",
        'R' | 'C' => "→",
        'U' => "!",
        _ => "•",
    }
}

fn stage_state(e: &Entry) -> Option<bool> {
    match (e.staged.is_some(), e.unstaged.is_some()) {
        (true, false) => Some(true),
        (true, true) => None,
        _ => Some(false),
    }
}

fn group_state(list: &[Entry]) -> Option<bool> {
    if list.iter().all(|e| stage_state(e) == Some(true)) {
        Some(true)
    } else if list.iter().any(|e| stage_state(e) != Some(false)) {
        None
    } else {
        Some(false)
    }
}

fn add_color(dark: bool) -> Hsla {
    gpui::rgb(if dark { 0x7bc77f } else { 0x2f8a3a }).into()
}

fn del_color(dark: bool) -> Hsla {
    gpui::rgb(if dark { 0xf08b7e } else { 0xc0392b }).into()
}

impl CrewApp {
    fn is_dark(&self) -> bool {
        self.config.str("theme") == "dark"
    }

    fn status_icon(&self, s: char) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(15.))
            .rounded(px(3.))
            .bg(status_color(s))
            .text_color(gpui::white())
            .text_size(px(11.))
            .child(status_mark(s))
            .into_any_element()
    }

    fn small_button(&self, id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
        self.button(id, label, false).px(px(8.)).py(px(1.)).text_size(px(12.))
    }

    fn counts(&self, path: &str) -> Option<(u64, u64)> {
        let g = self.git.as_ref()?;
        let (a, b) = (g.numstat_staged.get(path), g.numstat_unstaged.get(path));
        (a.is_some() || b.is_some()).then(|| (a.map_or(0, |x| x.0) + b.map_or(0, |x| x.0), a.map_or(0, |x| x.1) + b.map_or(0, |x| x.1)))
    }

    fn render_file_row(&self, e: &Entry, list: Rc<Vec<Entry>>, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let dark = self.is_dark();
        let (dir, base) = split_path(&e.path);
        let picked = g.picked.contains(&e.path);
        let counts = self.counts(&e.path);
        let id = SharedString::from(format!("git-file-{}-{depth}-{}", e.path, e.status));
        let (click_entry, click_list) = (e.clone(), list.clone());
        let (menu_entry, menu_list) = (e.clone(), list);
        let check_entry = e.clone();
        let tree = g.tree;
        div()
            .id(id.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .pl(px(10. + depth as f32 * 14.))
            .pr(px(8.))
            .cursor_pointer()
            .text_size(px(12.5))
            .when(picked, |d| d.bg(c.hover))
            .hover(move |s| s.bg(c.hover))
            .on_click(cx.listener(move |app, ev: &ClickEvent, _, cx| app.pick_file(&click_entry, &click_list, ev.modifiers(), true, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, ev: &MouseDownEvent, _, cx| {
                if !app.git.as_ref().is_some_and(|g| g.picked.contains(&menu_entry.path)) {
                    app.pick_file(&menu_entry, &menu_list, gpui::Modifiers::default(), false, cx);
                }
                app.file_menu(menu_entry.clone(), (*menu_list).clone(), ev.position, cx);
            }))
            .child(self.status_icon(e.status))
            .child(div().flex_none().max_w(relative(0.6)).truncate().child(base.to_string()))
            .when(!tree, |d| d.child(div().flex_1().min_w(px(0.)).truncate().text_size(px(11.5)).text_color(c.muted).child(e.orig.as_ref().map_or_else(|| dir.to_string(), |o| format!("{o} →")))))
            .when(tree, |d| d.child(div().flex_1()))
            .when_some(counts, |d, (a, r)| {
                d.child(div().flex().flex_none().gap(px(4.)).text_size(px(11.5)).child(div().text_color(add_color(dark)).child(format!("+{a}"))).child(div().text_color(del_color(dark)).child(format!("−{r}"))))
            })
            .child(
                div()
                    .id(SharedString::from(format!("{id}-check")))
                    .flex_none()
                    .occlude()
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.toggle_staged(std::slice::from_ref(&check_entry), cx);
                    }))
                    .child(self.check_box(stage_state(e))),
            )
            .into_any_element()
    }

    /// The tree view: folders with one child folder chain collapse into one row.
    fn tree_rows(&self, list: Rc<Vec<Entry>>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        #[derive(Default)]
        struct Node {
            dirs: std::collections::BTreeMap<String, Node>,
            files: Vec<Entry>,
        }
        fn under(n: &Node, out: &mut Vec<Entry>) {
            out.extend(n.files.iter().cloned());
            n.dirs.values().for_each(|d| under(d, out));
        }
        let mut root = Node::default();
        for e in list.iter() {
            let mut node = &mut root;
            let parts: Vec<&str> = e.path.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                node = node.dirs.entry((*part).to_string()).or_default();
            }
            node.files.push(e.clone());
        }
        let c = self.colors;
        let mut rows = Vec::new();
        fn walk(app: &CrewApp, node: &Node, depth: usize, prefix: &str, list: &Rc<Vec<Entry>>, c: Colors, rows: &mut Vec<AnyElement>, cx: &mut Context<CrewApp>) {
            for (first, d) in &node.dirs {
                let mut name = first.clone();
                let mut cur = d;
                while cur.files.is_empty() && cur.dirs.len() == 1 {
                    let (next, inner) = cur.dirs.iter().next().unwrap();
                    name = format!("{name}/{next}");
                    cur = inner;
                }
                let key = format!("{prefix}{name}");
                let folded = app.git.as_ref().is_some_and(|g| g.folded_dirs.contains(&key));
                let mut kids = Vec::new();
                under(cur, &mut kids);
                let state = group_state(&kids);
                let toggle = key.clone();
                rows.push(
                    div()
                        .id(SharedString::from(format!("git-dir-{key}")))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .h(px(24.))
                        .pl(px(10. + depth as f32 * 14.))
                        .pr(px(8.))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if let Some(g) = app.git.as_mut() {
                                if !g.folded_dirs.remove(&toggle) {
                                    g.folded_dirs.insert(toggle.clone());
                                }
                            }
                            cx.notify();
                        }))
                        .child(div().flex_none().text_color(c.muted).child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.))))
                        .child(div().flex_1().min_w(px(0.)).truncate().child(name))
                        .child(
                            div()
                                .id(SharedString::from(format!("git-dir-check-{key}")))
                                .flex_none()
                                .occlude()
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.toggle_staged(&kids, cx);
                                }))
                                .child(app.check_box(state)),
                        )
                        .into_any_element(),
                );
                if !folded {
                    walk(app, cur, depth + 1, &format!("{key}/"), list, c, rows, cx);
                }
            }
            for e in &node.files {
                rows.push(app.render_file_row(e, list.clone(), depth, cx));
            }
        }
        walk(self, &root, 0, "", &list, c, &mut rows, cx);
        rows
    }

    fn render_section(&self, title: &'static str, list: Vec<Entry>, empty_text: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let folded = g.folded_sections.contains(title);
        let count = list.len();
        let state = group_state(&list);
        let toggle_list = list.clone();
        let shared = Rc::new(list);
        let rows: Vec<AnyElement> = if folded {
            Vec::new()
        } else if g.tree {
            self.tree_rows(shared.clone(), cx)
        } else {
            shared.iter().map(|e| self.render_file_row(e, shared.clone(), 0, cx)).collect()
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("git-section-{title}")))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h(px(26.))
                    .pl(px(8.))
                    .pr(px(8.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if let Some(g) = app.git.as_mut() {
                            if !g.folded_sections.remove(title) {
                                g.folded_sections.insert(title.to_string());
                            }
                        }
                        cx.notify();
                    }))
                    .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.)))
                    .child(div().flex_1().font_weight(gpui::FontWeight::SEMIBOLD).child(t(title)))
                    .when(count > 0, |d| {
                        d.child(count.to_string()).child(
                            div()
                                .id(SharedString::from(format!("git-section-check-{title}")))
                                .occlude()
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.toggle_staged(&toggle_list, cx);
                                }))
                                .child(self.check_box(state)),
                        )
                    }),
            )
            .when(!folded, |d| {
                if rows.is_empty() {
                    d.child(div().px(px(12.)).py(px(4.)).text_size(px(12.)).text_color(c.muted).child(t(empty_text)))
                } else {
                    d.children(rows)
                }
            })
            .into_any_element()
    }

    fn render_changes_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let all = git::entries(&g.status);
        let (conflicts, rest): (Vec<Entry>, Vec<Entry>) = all.into_iter().partition(|e| e.status == 'U');
        let mut parts: Vec<AnyElement> = Vec::new();
        if let Some(op) = g.refs.operation {
            let then = if op == "merge" { "commit" } else { "continue" };
            parts.push(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .m(px(6.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(gpui::rgb(if self.is_dark() { 0x33291a } else { 0xfdf3dc }))
                    .text_size(px(12.5))
                    .child(div().w_full().child(format!("A {op} is in progress. Resolve any conflicts and stage the files, then {then}.")))
                    .when(op != "merge", |d| d.child(self.small_button("git-continue", t("Continue")).on_click(cx.listener(|app, _, _, cx| app.continue_operation(cx)))))
                    .child(self.small_button("git-abort", format!("Abort {op}")).on_click(cx.listener(|app, _, window, cx| app.abort_operation(window, cx))))
                    .into_any_element(),
            );
        }
        if !conflicts.is_empty() {
            parts.push(self.render_section("Conflicts", conflicts, "", cx));
        }
        if g.by_tracked {
            let (tracked, untracked): (Vec<Entry>, Vec<Entry>) = rest.into_iter().partition(|e| e.status != '?');
            parts.push(self.render_section("Tracked", tracked, "No changes", cx));
            if !untracked.is_empty() {
                parts.push(self.render_section("Untracked", untracked, "", cx));
            }
        } else {
            let staged: Vec<Entry> = rest.iter().filter(|e| e.staged.is_some()).cloned().collect();
            let unstaged: Vec<Entry> = rest.iter().filter(|e| e.unstaged.is_some()).cloned().collect();
            parts.push(self.render_section("Staged", staged, "No staged changes yet", cx));
            parts.push(self.render_section("Unstaged", unstaged, "No changes", cx));
        }
        div().id("git-files").flex().flex_col().flex_1().min_h(px(0.)).overflow_y_scroll().bg(c.main_bg).children(parts).into_any_element()
    }

    fn render_foot(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let history = g.view == GitView::History;
        let branch_label = if !g.status.branch.is_empty() {
            g.status.branch.clone()
        } else if let Some(h) = &g.status.head {
            format!("detached at {}", short(h))
        } else {
            t("no commits yet").to_string()
        };
        let sync = self.sync_action();
        let flags: Vec<&str> = [(g.amend, "Amend"), (g.signoff, "Signoff"), (g.no_verify, "Skip hooks"), (g.push_after, "Then push")].iter().filter(|x| x.0).map(|x| t(x.1)).collect();
        let last = g.refs.last.clone();
        let head = g.status.head.clone();
        let folded = g.commit_folded;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(6.))
            .p(px(8.))
            .border_t_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("git-branch-picker")
                            .flex()
                            .flex_1()
                            .min_w(px(0.))
                            .items_center()
                            .gap(px(5.))
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_size(px(12.5))
                            .hover(move |s| s.bg(c.hover))
                            .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                if let Some(root) = app.git.as_ref().map(|g| g.root.clone()) {
                                    app.open_branch_picker(e.position, root, true, window, cx);
                                }
                            }))
                            .child(div().flex_none().text_color(c.muted).child(Icon::empty().path(assets::GIT_BRANCH).size(px(13.))))
                            .child(div().min_w(px(0.)).truncate().child(branch_label)),
                    )
                    .when_some(sync, |d, (icon, label)| {
                        let symbol = match icon {
                            "pull" => IconName::ArrowDown,
                            "push" => IconName::ArrowUp,
                            _ => IconName::RefreshCw,
                        };
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .rounded(px(7.))
                                .border_1()
                                .border_color(c.border)
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .id("git-sync-go")
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .px(px(8.))
                                        .py(px(2.))
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(c.hover))
                                        .on_click(cx.listener(|app, _, window, cx| app.run_sync(window, cx)))
                                        .child(Icon::new(symbol).size(px(13.)))
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .id("git-sync-more")
                                        .px(px(3.))
                                        .py(px(2.))
                                        .border_l_1()
                                        .border_color(c.border)
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(c.hover))
                                        .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            app.sync_menu(e.position, cx);
                                        }))
                                        .child(Icon::new(IconName::ChevronDown).size(px(12.))),
                                ),
                        )
                    }),
            )
            .when(!history, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .when(!folded, |d| d.child(Textarea::new(&g.message).small()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(self.icon_button("git-commit-fold", if folded { IconName::ChevronUp } else { IconName::ChevronDown }).p(px(3.)).on_click(cx.listener(|app, _, _, cx| {
                                    let Some(g) = app.git.as_mut() else { return };
                                    g.commit_folded = !g.commit_folded;
                                    let v = g.commit_folded;
                                    app.save_setting("gitCommitFolded", json!(v));
                                    cx.notify();
                                })))
                                .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(11.5)).text_color(c.muted).child(flags.join(" · ")))
                                .child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .rounded(px(7.))
                                        .bg(c.accent)
                                        .text_color(gpui::white())
                                        .text_size(px(12.5))
                                        .child(div().id("git-commit-go").px(px(12.)).py(px(3.)).cursor_pointer().on_click(cx.listener(|app, _, _, cx| app.do_commit(cx))).child(self.commit_label()))
                                        .child(
                                            div()
                                                .id("git-commit-more")
                                                .px(px(4.))
                                                .py(px(3.))
                                                .cursor_pointer()
                                                .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    app.commit_options_menu(e.position, cx);
                                                }))
                                                .child(Icon::new(IconName::ChevronDown).size(px(13.))),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(last, |d, last| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(12.))
                        .text_color(c.muted)
                        .child(div().flex_1().min_w(px(0.)).truncate().child(last.subject.clone()))
                        .when(last.has_parent, |d| d.child(self.icon_button("git-uncommit", IconName::Undo2).p(px(3.)).on_click(cx.listener(|app, _, _, cx| app.uncommit(cx)))))
                        .when_some(head, |d, head| d.child(self.icon_button("git-to-history", Icon::empty().path(assets::GIT_BRANCH)).p(px(3.)).on_click(cx.listener(move |app, _, _, cx| app.jump_to_commit(head.clone(), cx))))),
                )
            })
            .into_any_element()
    }

    fn render_code(&self, line: &DiffLine) -> AnyElement {
        let c = self.colors;
        let base = div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().font_family("Cascadia Mono").text_size(px(12.5)).text_color(c.text);
        match &line.words {
            None => base.child(line.text.clone()).into_any_element(),
            Some(w) => {
                let text = format!("{}{}{}", w.pre, w.mid, w.post);
                let (from, to) = (w.pre.len(), w.pre.len() + w.mid.len());
                let tint: Hsla = if line.kind == LineKind::Add { add_color(self.is_dark()).opacity(0.28) } else { del_color(self.is_dark()).opacity(0.28) };
                let style = HighlightStyle { background_color: Some(tint), ..HighlightStyle::default() };
                base.child(StyledText::new(text).with_highlights([(from..to, style)])).into_any_element()
            }
        }
    }

    fn line_colors(&self, kind: LineKind) -> (Option<Hsla>, Hsla) {
        let dark = self.is_dark();
        match kind {
            LineKind::Add => (Some(gpui::rgb(if dark { 0x1d3324 } else { 0xe6f4ea }).into()), gpui::rgb(if dark { 0x6fcf8a } else { 0x1f7a3a }).into()),
            LineKind::Del => (Some(gpui::rgb(if dark { 0x3a2220 } else { 0xfbe9e7 }).into()), gpui::rgb(if dark { 0xf08b7e } else { 0xb3362a }).into()),
            _ => (None, self.colors.muted),
        }
    }

    fn render_diff_row(&self, index: usize, row: &DiffRow, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let number = |n: Option<u64>| div().flex_none().w(px(44.)).pr(px(6.)).flex().justify_end().font_family("Cascadia Mono").text_size(px(11.5)).text_color(c.muted).child(n.map_or(String::new(), |v| v.to_string()));
        let base = div().flex().items_center().w_full().h(px(DIFF_ROW_H)).overflow_hidden();
        match row {
            DiffRow::Line(l) => {
                let (bg, sign_color) = self.line_colors(l.kind);
                let sign = match l.kind {
                    LineKind::Add => "+",
                    LineKind::Del => "-",
                    _ => "",
                };
                base.when_some(bg, |d, bg| d.bg(bg))
                    .child(number(l.old))
                    .child(number(l.new))
                    .child(div().flex_none().w(px(16.)).text_color(sign_color).font_family("Cascadia Mono").child(sign))
                    .child(self.render_code(l))
                    .into_any_element()
            }
            DiffRow::Sides(a, b) => {
                let side = |l: &Option<DiffLine>, new: bool| {
                    let (bg, _) = l.as_ref().map_or((None, c.muted), |l| self.line_colors(l.kind));
                    div()
                        .flex()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .items_center()
                        .overflow_hidden()
                        .when_some(bg, |d, bg| d.bg(bg))
                        .when_some(l.as_ref(), |d, l| d.child(number(if new { l.new } else { l.old })).child(self.render_code(l)))
                };
                base.child(side(a, false)).child(div().flex_none().w(px(1.)).h_full().bg(c.border)).child(side(b, true)).into_any_element()
            }
            DiffRow::Hunk { title, tag, actions } => {
                let buttons: Vec<AnyElement> = match actions {
                    HunkActions::None => Vec::new(),
                    HunkActions::Unstage { header, hunk } => {
                        let (h, k) = (header.clone(), hunk.clone());
                        vec![self.small_button(("hunk-unstage", index), t("Unstage")).on_click(cx.listener(move |app, _, window, cx| app.apply_hunk(&h, &k, "unstage", window, cx))).into_any_element()]
                    }
                    HunkActions::Stage { header, hunk } => {
                        let (h1, k1, h2, k2) = (header.clone(), hunk.clone(), header.clone(), hunk.clone());
                        vec![
                            self.small_button(("hunk-stage", index), t("Stage")).on_click(cx.listener(move |app, _, window, cx| app.apply_hunk(&h1, &k1, "stage", window, cx))).into_any_element(),
                            self.small_button(("hunk-restore", index), t("Restore")).on_click(cx.listener(move |app, _, window, cx| app.apply_hunk(&h2, &k2, "discard", window, cx))).into_any_element(),
                        ]
                    }
                };
                let dark = self.is_dark();
                base.gap(px(6.))
                    .px(px(8.))
                    .bg(gpui::rgb(if dark { 0x232a35 } else { 0xf1f4fa }))
                    .text_size(px(12.))
                    .text_color(c.muted)
                    .when_some(*tag, |d, tag| d.child(div().flex_none().px(px(5.)).rounded(px(4.)).bg(c.border).child(t(tag))))
                    .child(div().flex_1().min_w(px(0.)).truncate().font_family("Cascadia Mono").child(title.clone()))
                    .children(buttons)
                    .into_any_element()
            }
            DiffRow::File { key, status, orig, folded, entry } => {
                let (dir, name) = split_path(key);
                let toggle = key.clone();
                let open_key = key.clone();
                let restore = entry.as_ref().and_then(|e| e.unstaged.clone()).filter(|_| *status != 'U');
                base.gap(px(6.))
                    .px(px(8.))
                    .bg(c.side_bg)
                    .border_t_1()
                    .border_color(c.border)
                    .text_size(px(12.5))
                    .child(div().id(("file-fold", index)).flex_none().cursor_pointer().text_color(c.muted).on_click(cx.listener(move |app, _, _, cx| app.toggle_file_fold(&toggle, cx))).child(Icon::new(if *folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.))))
                    .when_some(entry.clone(), |d, e| {
                        let state = stage_state(&e);
                        d.child(
                            div()
                                .id(("file-check", index))
                                .flex_none()
                                .on_click(cx.listener(move |app, _, _, cx| app.toggle_staged(std::slice::from_ref(&e), cx)))
                                .child(self.check_box(state)),
                        )
                    })
                    .child(self.status_icon(*status))
                    .child(div().flex_none().max_w(relative(0.5)).truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(name.to_string()))
                    .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(11.5)).text_color(c.muted).child(orig.as_ref().map_or_else(|| dir.to_string(), |o| format!("{o} →"))))
                    .when_some(restore, |d, change| {
                        d.child(self.small_button(("file-restore", index), t("Restore")).on_click(cx.listener(move |app, _, window, cx| app.discard(vec![change.clone()], window, cx))))
                    })
                    .when(entry.is_some(), |d| {
                        d.child(self.small_button(("file-view", index), t("View file")).on_click(cx.listener(move |app, _, _, cx| {
                            if let Some(g) = &app.git {
                                cx.open_with_system(&g.root.join(open_key.replace('/', "\\")));
                            }
                        })))
                    })
                    .into_any_element()
            }
            DiffRow::Note(text) => base.px(px(14.)).text_size(px(12.)).text_color(c.muted).child(text.clone()).into_any_element(),
            DiffRow::Placeholder(text) => base.justify_center().text_size(px(13.)).text_color(c.muted).child(text.clone()).into_any_element(),
            DiffRow::ShowAll => base
                .id(("diff-show-all", index))
                .px(px(14.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(c.accent)
                .on_click(cx.listener(|app, _, _, cx| app.show_full_diff(cx)))
                .child(t("Large diff: the rest is hidden. Show everything"))
                .into_any_element(),
            DiffRow::Conflict { path, index: part, ours, theirs } => {
                let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                let part = *part;
                base.gap(px(6.))
                    .px(px(10.))
                    .child(self.small_button(("conflict-ours", index), format!("Use {ours}")).on_click(cx.listener(move |app, _, _, cx| app.resolve_conflict(&p1, part, "ours", cx))))
                    .child(self.small_button(("conflict-theirs", index), format!("Use {theirs}")).on_click(cx.listener(move |app, _, _, cx| app.resolve_conflict(&p2, part, "theirs", cx))))
                    .child(self.small_button(("conflict-both", index), t("Use both")).on_click(cx.listener(move |app, _, _, cx| app.resolve_conflict(&p3, part, "both", cx))))
                    .into_any_element()
            }
            DiffRow::ConflictSide { label, text, ours } => {
                let (bg, _) = self.line_colors(if *ours { LineKind::Add } else { LineKind::Del });
                let row = base.px(px(10.)).when_some(bg, |d, bg| d.bg(bg));
                if text.is_empty() && !label.is_empty() {
                    row.text_size(px(11.5)).font_weight(gpui::FontWeight::SEMIBOLD).child(label.clone()).into_any_element()
                } else {
                    row.font_family("Cascadia Mono").text_size(px(12.5)).child(text.clone()).into_any_element()
                }
            }
            DiffRow::ConflictContext(text) => base.px(px(10.)).font_family("Cascadia Mono").text_size(px(12.5)).text_color(c.muted).child(text.clone()).into_any_element(),
            DiffRow::ConflictDone { path } => {
                let done = path.clone();
                base.gap(px(8.))
                    .px(px(10.))
                    .text_size(px(12.5))
                    .child(t("No conflict markers left in this file."))
                    .child(self.small_button(("conflict-done", index), t("Mark resolved")).on_click(cx.listener(move |app, _, _, cx| {
                        let change = Change { path: done.clone(), orig: None, status: 'U' };
                        app.stage(&[change], false, cx);
                    })))
                    .into_any_element()
            }
        }
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(g) = &self.git else { return div().into_any_element() };
        let rows = g.diff_rows.clone();
        let count = rows.len();
        let handle = g.diff_scroll.clone();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .child(
                uniform_list(
                    "git-diff",
                    count,
                    cx.processor(move |app, range: Range<usize>, _, cx| range.map(|i| app.render_diff_row(i, &rows[i], cx)).collect::<Vec<_>>()),
                )
                .track_scroll(&handle)
                .flex_1()
                .w_full(),
            )
            .into_any_element()
    }

    fn render_log_row(&self, i: usize, commit: &Commit, graph_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let room = if g.diff_open { g.wide_width } else { g.list_width };
        let is_head = g.status.head.as_deref() == Some(commit.hash.as_str());
        let selected = g.selected.as_deref() == Some(commit.hash.as_str());
        let graph = commit.graph.clone();
        let hash = commit.hash.clone();
        let (row_commit, select_hash) = (commit.clone(), commit.hash.clone());
        let background = c.main_bg;
        let lane_x = |n: usize| 10. + n as f32 * LANE_W;
        let lane_color = |n: usize| -> Hsla { gpui::rgb(LANE_COLORS[n % LANE_COLORS.len()]).into() };
        let painter = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let at = |x: f32, y: f32| bounds.origin + point(px(x), px(y));
                let mid = ROW_H / 2.;
                let curve = |window: &mut Window, x1: f32, y1: f32, x2: f32, y2: f32, color: Hsla| {
                    let mut b = PathBuilder::stroke(px(1.5));
                    b.move_to(at(x1, y1));
                    if (x1 - x2).abs() < 0.5 {
                        b.line_to(at(x2, y2));
                    } else {
                        let m = (y1 + y2) / 2.;
                        b.cubic_bezier_to(at(x2, y2), at(x1, m), at(x2, m));
                    }
                    if let Ok(path) = b.build() {
                        window.paint_path(path, color);
                    }
                };
                for (n, h) in graph.before.iter().enumerate() {
                    match h {
                        None => {}
                        Some(h) if *h == hash => curve(window, lane_x(n), 0., lane_x(graph.col), mid, lane_color(n)),
                        Some(_) => curve(window, lane_x(n), 0., lane_x(n), ROW_H, lane_color(n)),
                    }
                }
                for pc in &graph.parent_cols {
                    curve(window, lane_x(graph.col), mid, lane_x(*pc), ROW_H, lane_color(*pc));
                }
                let r = if is_head { 4.5 } else { 3.5 };
                let dot = Bounds::new(at(lane_x(graph.col) - r, mid - r), size(px(r * 2.), px(r * 2.)));
                let color = lane_color(graph.col);
                if is_head {
                    window.paint_quad(fill(dot, color).corner_radii(px(r)));
                    let inner = Bounds::new(at(lane_x(graph.col) - r + 2., mid - r + 2.), size(px(r * 2. - 4.), px(r * 2. - 4.)));
                    window.paint_quad(fill(inner, background).corner_radii(px(r)));
                } else {
                    window.paint_quad(fill(dot, color).corner_radii(px(r)));
                }
            },
        );
        let dark = self.is_dark();
        let shown_refs = if room < 460. { 2 } else { commit.refs.len() };
        let hidden_refs = commit.refs.len().saturating_sub(shown_refs);
        let badges: Vec<AnyElement> = commit
            .refs
            .iter()
            .take(shown_refs)
            .enumerate()
            .map(|(n, r)| {
                let (fg, bd, bg): (u32, u32, u32) = match (r.kind, dark) {
                    (RefKind::Tag, true) => (0xe4c870, 0x6a5a24, 0x2e2816),
                    (RefKind::Tag, false) => (0x8a6d14, 0xe0cd8a, 0xfbf5de),
                    (RefKind::Remote, true) => (0xb9a9f0, 0x4a3f73, 0x26213a),
                    (RefKind::Remote, false) => (0x6a52c0, 0xd6cdf2, 0xf3f0fc),
                    (_, true) => (0x8dbcf0, 0x35557a, 0x1d2a3a),
                    (_, false) => (0x1d5fae, 0xb9d3f0, 0xe8f1fc),
                };
                let (kind, name) = (r.kind, r.name.clone());
                div()
                    .id(SharedString::from(format!("ref-{i}-{n}")))
                    .flex_none()
                    .max_w(px(120.))
                    .truncate()
                    .px(px(5.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(gpui::rgb(bd))
                    .bg(gpui::rgb(bg))
                    .text_color(gpui::rgb(fg))
                    .text_size(px(11.))
                    .when(r.kind == RefKind::Head, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        app.ref_menu(kind, &name, e.position, cx);
                    }))
                    .child(r.name.clone())
                    .into_any_element()
            })
            .collect();
        let menu_commit = row_commit;
        div()
            .id(("git-log-row", i))
            .flex()
            .items_center()
            .w_full()
            .h(px(ROW_H))
            .gap(px(6.))
            .pr(px(10.))
            .cursor_pointer()
            .text_size(px(12.5))
            .when(selected, |d| d.bg(gpui::rgb(if dark { 0x26364a } else { 0xdde9f8 })))
            .when(!selected, |d| d.hover(move |s| s.bg(c.hover)))
            .on_click(cx.listener(move |app, _, _, cx| app.open_commit(select_hash.clone(), cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| {
                app.select_commit(menu_commit.hash.clone(), cx);
                app.commit_menu(menu_commit.clone(), e.position, cx);
            }))
            .child(div().flex_none().w(px(graph_width)).h_full().child(painter.size_full()))
            .child(div().flex().flex_1().min_w(px(0.)).items_center().gap(px(4.)).overflow_hidden().children(badges).when(hidden_refs > 0, |d| d.child(div().flex_none().text_size(px(11.)).text_color(c.muted).child(format!("+{hidden_refs}")))).child(div().flex_1().min_w(px(70.)).truncate().when(is_head, |d| d.font_weight(gpui::FontWeight::SEMIBOLD)).child(commit.subject.clone())))
            .when(room >= 460., |d| d.child(div().flex_none().text_size(px(11.5)).text_color(c.muted).child(fmt_date(commit.at))))
            .when(room >= 640., |d| {
                d.child(div().flex_none().w(px(90.)).truncate().text_size(px(11.5)).text_color(c.muted).child(commit.author.clone()))
                    .child(div().flex_none().w(px(56.)).font_family("Cascadia Mono").text_size(px(11.5)).text_color(c.muted).child(short(&commit.hash)))
            })
            .into_any_element()
    }

    fn render_log(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let commits = g.commits.clone();
        let count = commits.len();
        let graph_width = (g.lanes.min(16) as f32 * LANE_W + 12.).max(if g.diff_open { 64. } else { 30. });
        let handle = g.log_scroll.clone();
        let more = count >= 1000 && count % 1000 == 0;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .capture_key_down(cx.listener(|app, e: &KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                "down" => app.step_commit(true, cx),
                "up" => app.step_commit(false, cx),
                _ => {}
            }))
            .when_some(g.file_filter.clone(), |d, path| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.))
                        .px(px(10.))
                        .py(px(3.))
                        .bg(c.hover)
                        .text_size(px(12.))
                        .child(t("History of "))
                        .child(div().flex_1().min_w(px(0.)).truncate().font_family("Cascadia Mono").child(path))
                        .child(self.small_button("git-filter-clear", t("Show all")).on_click(cx.listener(|app, _, _, cx| app.open_file_history(None, cx)))),
                )
            })
            .map(|d| {
                if count == 0 {
                    d.child(div().flex().flex_1().items_center().justify_center().text_size(px(13.)).text_color(c.muted).child(t("No commits yet")))
                } else {
                    d.child(
                        uniform_list(
                            "git-log",
                            count,
                            cx.processor(move |app, range: Range<usize>, _, cx| range.map(|i| app.render_log_row(i, &commits[i], graph_width, cx)).collect::<Vec<_>>()),
                        )
                        .track_scroll(&handle)
                        .flex_1()
                        .w_full(),
                    )
                }
            })
            .when(more, |d| {
                d.child(
                    div()
                        .id("git-load-more")
                        .flex_none()
                        .px(px(10.))
                        .py(px(3.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(c.accent)
                        .on_click(cx.listener(|app, _, _, cx| app.load_more_commits(cx)))
                        .child(format!("Showing the latest {count} commits. Load more")),
                )
            })
            .into_any_element()
    }

    fn render_commit_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(g) = &self.git else { return div().into_any_element() };
        let placeholder = |text: &'static str| div().flex().flex_1().items_center().justify_center().text_size(px(13.)).text_color(c.muted).child(t(text)).into_any_element();
        if g.selected.is_none() {
            return placeholder("Select a commit");
        }
        let Some(d) = &g.detail else { return placeholder("Loading…") };
        let (subject, rest) = d.body.split_once('\n').unwrap_or((&d.body, ""));
        let stash = g.refs.stashes.iter().find(|s| s.reference == d.rev);
        let meta_row = |k: &'static str, v: AnyElement| div().flex().gap(px(6.)).text_size(px(12.)).child(div().flex_none().w(px(66.)).text_color(c.muted).child(t(k))).child(div().flex_1().min_w(px(0.)).child(v));
        let parents = d.parents.clone();
        let parent_links: Vec<AnyElement> = parents
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let hash = p.clone();
                div().id(("git-parent", i)).cursor_pointer().text_color(c.accent).font_family("Cascadia Mono").on_click(cx.listener(move |app, _, _, cx| app.jump_to_commit(hash.clone(), cx))).child(short(p)).into_any_element()
            })
            .collect();
        let files: Vec<AnyElement> = d
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let (dir, base) = split_path(&f.path);
                let (scroll, menu) = (f.path.clone(), f.clone());
                div()
                    .id(("git-commit-file", i))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(24.))
                    .px(px(10.))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |app, _, _, _| app.scroll_diff_to(&scroll)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| app.commit_file_menu(menu.clone(), e.position, cx)))
                    .child(self.status_icon(f.status))
                    .child(div().flex_none().max_w(relative(0.6)).truncate().child(base.to_string()))
                    .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(11.5)).text_color(c.muted).child(f.orig.as_ref().map_or_else(|| dir.to_string(), |o| format!("{o} →"))))
                    .into_any_element()
            })
            .collect();
        let apply_stash = stash.map(|s| (s.reference.clone(), s.reference.clone()));
        div()
            .id("git-commit-detail")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(10.))
                    .border_b_1()
                    .border_color(c.border)
                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).child(stash.map_or_else(|| subject.to_string(), |s| format!("{}: {subject}", s.reference))))
                    .when(!rest.trim().is_empty(), |d| d.child(div().text_size(px(12.5)).text_color(c.muted).child(rest.trim().to_string())))
                    .child(meta_row("Commit", div().font_family("Cascadia Mono").child(d.hash.clone()).into_any_element()))
                    .when(!parents.is_empty(), |x| x.child(meta_row(if parents.len() > 1 { "Parents" } else { "Parent" }, div().flex().gap(px(6.)).children(parent_links).into_any_element())))
                    .child(meta_row("Author", div().child(d.author.clone()).into_any_element()))
                    .child(meta_row("Date", div().child(fmt_date(d.at)).into_any_element()))
                    .when(d.committer != d.author, |x| x.child(meta_row("Committer", div().child(d.committer.clone()).into_any_element())))
                    .when_some(apply_stash, |x, (apply, pop)| {
                        x.child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(self.small_button("stash-apply", t("Apply")).on_click(cx.listener(move |app, _, _, cx| app.git_op(format!("Apply {apply}"), vec![step(&["stash", "apply", "--index", &apply])], false, None, cx))))
                                .child(self.small_button("stash-pop", t("Pop")).on_click(cx.listener(move |app, _, _, cx| app.git_op(format!("Pop {pop}"), vec![step(&["stash", "pop", "--index", &pop])], false, None, cx)))),
                        )
                    }),
            )
            .child(div().flex().justify_between().h(px(26.)).items_center().px(px(10.)).text_size(px(12.)).text_color(c.muted).child(t("Files")).child(d.files.len().to_string()))
            .when(d.parents.len() > 1, |x| x.child(div().px(px(10.)).pb(px(4.)).text_size(px(12.)).text_color(c.muted).child(t("Changes compared with the first parent."))))
            .children(files)
            .into_any_element()
    }

    pub(super) fn render_git_panel(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let g = self.git.as_ref()?;
        let history = g.view == GitView::History;
        let wide = g.diff_open;
        let on_commit = history && g.commit_open;
        let room = f32::from(window.viewport_size().width) - if self.sidebar_hidden { 0. } else { self.sidebar_width };
        let width = if wide { g.wide_width.max(g.list_width).min(room - 60.) } else { g.list_width };
        let changes = git::entries(&g.status).len();
        let repo = g.root.file_name().map_or_else(|| g.root.to_string_lossy().to_string(), |n| n.to_string_lossy().to_string());
        let (add, del) = g.numstat_staged.values().chain(g.numstat_unstaged.values()).fold((0, 0), |s, v| (s.0 + v.0, s.1 + v.1));
        let dark = self.is_dark();
        let tab = |id: &'static str, label: String, on: bool, view: GitView| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(4.))
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(if on { c.text } else { c.muted })
                .border_b_2()
                .border_color(if on { c.accent } else { gpui::transparent_black() })
                .on_click(cx.listener(move |app, _, _, cx| app.set_git_view(view, cx)))
                .child(label)
        };
        let changes_tab = tab("git-tab-changes", if changes > 0 { format!("{} ({changes})", t("Changes")) } else { t("Changes").to_string() }, !history, GitView::Changes);
        let history_tab = tab("git-tab-history", t("History").to_string(), history, GitView::History);
        let all_staged = changes > 0 && g.status.unstaged.is_empty();
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(4.))
            .border_b_1()
            .border_color(c.border)
            .when(on_commit, |d| {
                let label = g.selected.as_deref().map_or_else(String::new, |s| if s.len() == 40 { short(s).to_string() } else { s.to_string() });
                d.child(
                    div()
                        .id("git-commit-back")
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(4.))
                        .px(px(6.))
                        .py(px(3.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(|app, _, _, cx| app.close_commit(cx)))
                        .child(Icon::new(IconName::ArrowLeft).size(px(15.)))
                        .child(t("History")),
                )
                .child(div().flex_none().font_family("Cascadia Mono").text_size(px(12.)).text_color(c.muted).child(label))
                .child(self.icon_button("git-commit-newer", IconName::ChevronUp).p(px(3.)).on_click(cx.listener(|app, _, _, cx| app.step_commit(false, cx))))
                .child(self.icon_button("git-commit-older", IconName::ChevronDown).p(px(3.)).on_click(cx.listener(|app, _, _, cx| app.step_commit(true, cx))))
            })
            .when(!history, |d| {
                let on = g.diff_open;
                d.child(self.small_button("git-view-diff", if on { t("Hide Diff") } else { t("View Diff") }).on_click(cx.listener(move |app, _, _, cx| app.toggle_diff(!on, cx))))
            })
            .when(if history { on_commit } else { g.diff_open }, |d| {
                d.child(self.icon_button("git-diff-style", Icon::empty().path(assets::SPLIT_RIGHT)).p(px(3.)).when(g.split_diff, |x| x.text_color(c.accent)).on_click(cx.listener(|app, _, _, cx| app.toggle_diff_style(cx))))
            })
            .when(!history, |d| {
                d.child(self.icon_button("git-view-opts", IconName::Settings2).p(px(3.)).on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    app.view_menu(e.position, cx);
                })))
            })
            .child(div().flex_1())
            .when(!on_commit && (add > 0 || del > 0), |d| {
                d.child(div().flex().flex_none().gap(px(5.)).text_size(px(12.)).child(div().text_color(add_color(dark)).child(format!("+{add}"))).child(div().text_color(del_color(dark)).child(format!("−{del}"))))
            })
            .when(!history, |d| {
                d.child(
                    self.small_button("git-stage-all", if all_staged { t("Unstage All") } else { t("Stage All") })
                        .when(changes == 0, |x| x.opacity(0.5))
                        .on_click(cx.listener(|app, _, _, cx| app.stage_all_toggle(cx))),
                )
            })
            .child(self.icon_button("git-more", Icon::empty().path(assets::DOTS)).p(px(3.)).on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                app.more_menu(e.position, cx);
            })));
        let bar = g.bar.clone();
        let danger: Hsla = gpui::rgb(0xd93d42).into();
        let list_col = |content: AnyElement, foot: bool, this: &Self, cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(px(if wide { g.files_col_width } else { width }))
                .h_full()
                .when(wide, |d| d.border_r_1().border_color(c.border))
                .child(content)
                .when(foot, |d| d.child(this.render_foot(cx)))
        };
        let content: AnyElement = if history && g.commit_open {
            let detail = self.render_commit_detail(cx);
            if wide {
                let column = list_col(detail, false, self, cx);
                div().flex().flex_1().min_h(px(0.)).child(column).child(self.render_diff(cx)).into_any_element()
            } else {
                // As tall as what it holds, up to a share of the window: the rest is the diff's.
                let files = g.detail.as_ref().map_or(0, |d| d.files.len());
                let wanted = 190. + 24. * files as f32;
                let most = f32::from(window.viewport_size().height) * 0.42;
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .child(div().flex().flex_col().flex_none().h(px(wanted.min(most))).border_b_1().border_color(c.border).child(detail))
                    .child(div().flex().flex_1().min_h(px(0.)).child(self.render_diff(cx)))
                    .into_any_element()
            }
        } else if history {
            div().flex().flex_col().flex_1().min_h(px(0.)).child(self.render_log(cx)).child(self.render_foot(cx)).into_any_element()
        } else {
            let list = self.render_changes_list(cx);
            let column = list_col(list, true, self, cx);
            div().flex().flex_1().min_h(px(0.)).child(column).when(wide, |d| d.child(self.render_diff(cx))).into_any_element()
        };
        let panel = div()
            .id("git-panel")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(width))
            .flex()
            .bg(c.main_bg)
            .occlude()
            .child(div().flex_none().w(px(1.)).h_full().bg(c.border))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(4.))
                            .h(px(36.))
                            .pl(px(8.))
                            .pr(px(4.))
                            .border_b_1()
                            .border_color(c.border)
                            .child(
                                div()
                                    .id("git-worktree-picker")
                                    .flex()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .items_center()
                                    .gap(px(5.))
                                    .px(px(6.))
                                    .py(px(3.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .hover(move |s| s.bg(c.hover))
                                    .on_mouse_down(MouseButton::Left, cx.listener(|app, e: &MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(root) = app.git.as_ref().map(|g| g.root.clone()) {
                                            app.open_worktree_picker(e.position, root, false, window, cx);
                                        }
                                    }))
                                    .child(div().flex_none().text_color(c.muted).child(Icon::empty().path(assets::GIT_BRANCH).size(px(14.))))
                                    .child(div().min_w(px(0.)).truncate().child(repo)),
                            )
                            .child(self.icon_button("git-close", IconName::Close).on_click(cx.listener(|app, _, _, cx| {
                                app.git = None;
                                cx.notify();
                            }))),
                    )
                    .child(div().flex().flex_none().gap(px(2.)).px(px(4.)).border_b_1().border_color(c.border).child(changes_tab).child(history_tab))
                    .child(toolbar)
                    .child(content)
                    .when_some(bar, |d, (text, kind, _)| {
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(6.))
                                .px(px(10.))
                                .py(px(4.))
                                .border_t_1()
                                .border_color(c.border)
                                .text_size(px(12.))
                                .text_color(match kind {
                                    BarKind::Error => danger,
                                    BarKind::Ok => c.running,
                                    BarKind::Busy => c.muted,
                                })
                                .child(div().flex_1().min_w(px(0.)).child(text))
                                .when(kind == BarKind::Error, |d| {
                                    d.child(self.icon_button("git-bar-close", IconName::Close).p(px(2.)).on_click(cx.listener(|app, _, _, cx| {
                                        if let Some(g) = app.git.as_mut() {
                                            g.bar = None;
                                        }
                                        cx.notify();
                                    })))
                                }),
                        )
                    })
                    .when_some(g.loading.clone(), |d, text| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(8.))
                                .bg(c.main_bg.opacity(0.6))
                                .text_size(px(13.))
                                .child(Spinner::new().color(c.accent).xsmall())
                                .child(text),
                        )
                    }),
            );
        // The space it holds beside the terminal is always the list's: a wider panel lies over it.
        let panel = panel.child(self.resize_grip("git-resizer", Drag::Git, -5., cx));
        Some(div().relative().flex_none().w(px(g.list_width)).h_full().child(panel).into_any_element())
    }
}
