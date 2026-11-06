//! The file browser beside the terminal: the active session's folder as a tree, opened a folder at a
//! time, with the files Git ignores dimmed, and finding files by name or by content. A file opens in a
//! pane of its own in the main area, where it can be edited. The panel follows the active session's
//! folder and shares the right side with the Git panel and the browser.

use crew_core::files::{self, Content, Hit};
use gpui::{ObjectFit, StyledImage};
use gpui_component::input::{Editor, EditorState, Position};

use super::overlays::{Action, ToastKind};
use super::*;
use crate::i18n::zh;

const FILES_WIDTH: f32 = 320.;
const FILES_MIN: f32 = 220.;

#[derive(Clone)]
struct TreeRow {
    name: String,
    rel: String,
    dir: bool,
    ignored: bool,
    open: bool,
    depth: usize,
    /// "Empty", under an open folder with nothing in it.
    placeholder: bool,
}

pub(super) struct FilesPanel {
    pub(super) root: PathBuf,
    expanded: HashSet<String>,
    by_content: bool,
    query: Entity<InputState>,
    selected: Option<String>,
    rows: Vec<TreeRow>,
    error: Option<String>,
    hits: Vec<Hit>,
    /// Which search the results are from, so a slow old one can't replace a newer one.
    search_seq: u64,
}

enum Shown {
    Editing { editor: Entity<EditorState>, saved: String, crlf: bool, dirty: bool, _subscription: gpui::Subscription },
    Image(PathBuf),
    Message(String),
    Loading,
}

pub(super) struct FileView {
    pub(super) root: PathBuf,
    pub(super) rel: String,
    shown: Shown,
}

/// The highlighter's name for a file's type: its extension, or its name for files like Makefile.
fn language_of(rel: &str) -> &'static str {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    match name.as_str() {
        "dockerfile" => return "bash",
        "makefile" => return "make",
        ".gitignore" | ".env" => return "bash",
        ".npmrc" | ".editorconfig" => return "ini",
        _ => {}
    }
    match name.rsplit('.').next().unwrap_or("") {
        "rs" => "rust",
        "js" | "cjs" | "mjs" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "json" | "jsonc" => "json",
        "html" | "htm" | "vue" | "svelte" | "svg" | "xml" => "html",
        "css" | "scss" => "css",
        "md" => "markdown",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        "cs" => "csharp",
        "sh" | "bash" => "bash",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "sql" => "sql",
        _ => "text",
    }
}

impl CrewApp {
    fn active_folder(&self) -> Option<PathBuf> {
        self.active().map(|o| o.cwd.clone())
    }

    pub(super) fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.files.is_some() {
            self.files = None;
            cx.notify();
            return;
        }
        let folder = self.active_folder().or_else(|| self.file_view.as_ref().map(|f| f.root.clone()));
        if let Some(folder) = folder {
            self.open_files(folder, window, cx);
        }
    }

    fn open_files(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("Find files")));
        self._subscriptions.push(cx.subscribe(&query, |app, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                app.search_soon(cx);
            }
        }));
        self.close_other_right_panels();
        self.files = Some(FilesPanel { root: folder, expanded: HashSet::new(), by_content: false, query, selected: None, rows: Vec::new(), error: None, hits: Vec::new(), search_seq: 0 });
        self.reload_tree(cx);
        cx.notify();
    }

    /// An open panel follows the active session's folder.
    pub(super) fn files_follow_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.active_folder() else { return };
        if self.files.as_ref().is_some_and(|f| folder_text(&f.root) != folder_text(&folder)) {
            self.open_files(folder, window, cx);
        }
    }

    fn reload_tree(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &self.files else { return };
        let (root, expanded) = (f.root.clone(), f.expanded.clone());
        let load = cx.background_spawn(async move {
            fn rows_of(root: &Path, rel: &str, depth: usize, expanded: &HashSet<String>, out: &mut Vec<TreeRow>) -> Result<(), String> {
                let items = files::list(root, rel)?;
                if items.is_empty() && depth > 0 {
                    out.push(TreeRow { name: t("Empty").to_string(), rel: String::new(), dir: false, ignored: false, open: false, depth, placeholder: true });
                }
                for item in items {
                    let open = item.dir && expanded.contains(&item.rel);
                    out.push(TreeRow { name: item.name, rel: item.rel.clone(), dir: item.dir, ignored: item.ignored, open, depth, placeholder: false });
                    if open {
                        let _ = rows_of(root, &item.rel, depth + 1, expanded, out);
                    }
                }
                Ok(())
            }
            let mut rows = Vec::new();
            let result = rows_of(&root, "", 0, &expanded, &mut rows);
            (root, rows, result.err())
        });
        cx.spawn(async move |this, cx| {
            let (root, rows, error) = load.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(f) = app.files.as_mut().filter(|f| f.root == root) {
                    f.rows = rows;
                    f.error = error;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn search_soon(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.files.as_mut() else { return };
        f.search_seq += 1;
        let seq = f.search_seq;
        let (root, query, by_content) = (f.root.clone(), f.query.read(cx).value().trim().to_string(), f.by_content);
        if query.is_empty() {
            f.hits.clear();
            self.reload_tree(cx);
            cx.notify();
            return;
        }
        let wait = if by_content { 300 } else { 150 };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            let current = this.update(cx, |app, _| app.files.as_ref().is_some_and(|f| f.search_seq == seq)).unwrap_or(false);
            if !current {
                return;
            }
            let hits = cx
                .background_spawn(async move { if by_content { files::search_content(&root, &query) } else { files::search_names(&root, &query) } })
                .await;
            let _ = this.update(cx, |app, cx| {
                if let Some(f) = app.files.as_mut().filter(|f| f.search_seq == seq) {
                    f.hits = hits;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_files_mode(&mut self, by_content: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.files.as_mut() else { return };
        f.by_content = by_content;
        let query = f.query.clone();
        query.update(cx, |s, cx| {
            s.set_placeholder(if by_content { t("Find in files") } else { t("Find files") }, window, cx);
            s.focus(window, cx);
        });
        self.search_soon(cx);
    }

    fn file_is_dirty(&self) -> bool {
        matches!(self.file_view.as_ref().map(|f| &f.shown), Some(Shown::Editing { dirty: true, .. }))
    }

    /// Before showing another file or closing this one: save, discard or stay.
    fn leave_edits(&mut self, then: Action, cx: &mut Context<Self>) {
        if !self.file_is_dirty() {
            self.pending_after_save = None;
            return self.run_after_leaving(then, cx);
        }
        self.pending_after_save = Some(then);
        cx.notify();
    }

    fn run_after_leaving(&mut self, then: Action, cx: &mut Context<Self>) {
        self.deferred_action = Some(then);
        cx.notify();
    }

    /// Runs what was waiting on the file, now that a window is at hand.
    pub(super) fn run_deferred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(action) = self.deferred_action.take() {
            action(self, window, cx);
        }
    }

    fn show_file(&mut self, root: PathBuf, rel: String, line: Option<usize>, cx: &mut Context<Self>) {
        self.leave_edits(
            Rc::new(move |app: &mut CrewApp, window: &mut Window, cx: &mut Context<CrewApp>| {
                app.load_file(root.clone(), rel.clone(), line, window, cx);
            }),
            cx,
        );
    }

    fn load_file(&mut self, root: PathBuf, rel: String, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.files.as_mut() {
            f.selected = Some(rel.clone());
        }
        self.file_view = Some(FileView { root: root.clone(), rel: rel.clone(), shown: Shown::Loading });
        self.show_file_pane(window, cx);
        let (read_root, read_rel) = (root, rel.clone());
        let read = cx.background_spawn(async move { files::read(&read_root, &read_rel) });
        cx.spawn_in(window, async move |this, cx| {
            let content = read.await;
            let _ = this.update_in(cx, |app, window, cx| {
                if app.file_view.as_ref().is_none_or(|f| f.rel != rel) {
                    return;
                }
                let shown = match content {
                    Content::Unavailable(why) => Shown::Message(why),
                    Content::Image(path) => Shown::Image(path),
                    Content::Text(text) => {
                        let crlf = text.contains("\r\n");
                        let saved = text.replace("\r\n", "\n");
                        let (initial, language) = (saved.clone(), language_of(&rel));
                        let editor = cx.new(|cx| EditorState::new(window, cx).language(language).default_value(initial));
                        if let Some(n) = line {
                            editor.update(cx, |e, cx| e.set_cursor_position(Position::new(n.saturating_sub(1) as u32, 0), window, cx));
                        }
                        editor.update(cx, |e, cx| e.focus(window, cx));
                        let subscription = cx.subscribe(&editor, |app, state, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::Change) {
                                let now = state.read(cx).value().to_string();
                                if let Some(FileView { shown: Shown::Editing { saved, dirty, .. }, .. }) = app.file_view.as_mut() {
                                    *dirty = now != *saved;
                                }
                                cx.notify();
                            }
                        });
                        Shown::Editing { editor, saved, crlf, dirty: false, _subscription: subscription }
                    }
                };
                if let Some(f) = app.file_view.as_mut() {
                    f.shown = shown;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows the file pane: where it is already, else in an empty pane, else split beside the focused one.
    fn show_file_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.layout.focused_session();
        match self.layout.leaf_of(FILE_KEY) {
            Some(leaf) => self.layout.focus = leaf,
            None => {
                let empty = self.layout.leaves().into_iter().find(|l| l.1.is_none()).map(|l| l.0);
                match empty {
                    Some(leaf) => self.layout.focus = leaf,
                    None => self.layout.split(Dir::Row),
                }
                self.layout.show(FILE_KEY);
            }
        }
        self.focus_changed(previous, window, cx);
    }

    pub(super) fn save_file(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(FileView { root, rel, shown: Shown::Editing { editor, saved, crlf, dirty, .. } }) = self.file_view.as_mut() else { return true };
        if !*dirty {
            return true;
        }
        let text = editor.read(cx).value().to_string();
        let out = if *crlf { text.replace('\n', "\r\n") } else { text.clone() };
        let name = rel.clone();
        match files::write(root, rel, &out) {
            Ok(()) => {
                *saved = text;
                *dirty = false;
                self.toast(if zh() { format!("已保存 {name}") } else { format!("Saved {name}") }, ToastKind::Plain, cx);
                true
            }
            Err(why) => {
                self.toast(if zh() { format!("无法保存 {name}：{why}") } else { format!("Couldn't save {name}: {why}") }, ToastKind::Error, cx);
                false
            }
        }
    }

    pub(super) fn close_file_pane(&mut self, cx: &mut Context<Self>) {
        self.leave_edits(
            Rc::new(|app: &mut CrewApp, window: &mut Window, cx: &mut Context<CrewApp>| {
                let previous = app.layout.focused_session();
                app.file_view = None;
                if let Some(leaf) = app.layout.leaf_of(FILE_KEY) {
                    app.layout.forget(FILE_KEY);
                    app.layout.close(leaf);
                }
                app.focus_changed(previous, window, cx);
            }),
            cx,
        );
    }

    pub(super) fn file_title(&self) -> Option<String> {
        self.file_view.as_ref().map(|f| f.rel.rsplit('/').next().unwrap_or(&f.rel).to_string())
    }

    pub(super) fn file_path_text(&self) -> Option<String> {
        self.file_view.as_ref().map(|f| f.root.join(f.rel.replace('/', "\\")).to_string_lossy().to_string())
    }

    pub(super) fn render_file_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let Some(f) = &self.file_view else { return div().into_any_element() };
        let dirty = self.file_is_dirty();
        let full = f.root.join(&f.rel);
        let message = |text: String| div().flex().flex_1().items_center().justify_center().text_size(px(13.)).text_color(c.muted).child(text);
        let body = match &f.shown {
            Shown::Editing { editor, .. } => div().flex_1().min_h(px(0.)).child(Editor::new(editor).h_full().appearance(false).font_family("Cascadia Mono").text_size(px(self.config.f32("fontSize", 14.)))).into_any_element(),
            Shown::Image(path) => div().flex().flex_1().min_h(px(0.)).p(px(16.)).items_center().justify_center().child(img(path.clone()).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
            Shown::Message(why) => message(why.clone()).into_any_element(),
            Shown::Loading => message(t("Loading…").to_string()).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(c.main_bg)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(5.))
                    .border_b_1()
                    .border_color(c.border)
                    .text_size(px(12.5))
                    .child(div().flex_1().min_w(px(0.)).truncate().text_color(c.muted).child(f.rel.clone()))
                    .when(dirty, |d| {
                        d.child(div().flex_none().text_color(c.accent).child("●")).child(self.button("file-save", t("Save"), true).py(px(2.)).on_click(cx.listener(|app, _, _, cx| {
                            app.save_file(cx);
                        })))
                    })
                    .child(self.icon_button("file-open-external", IconName::ExternalLink).p(px(3.)).on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&full)))),
            )
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_save_changes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.pending_after_save.as_ref()?;
        let name = self.file_title().unwrap_or_default();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.heading(if zh() { format!("保存对 {name} 的更改？") } else { format!("Save changes to {name}?") }))
            .child(t("Your changes are lost if you don't save them."))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(self.button("leave-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                        app.pending_after_save = None;
                        cx.notify();
                    })))
                    .child(self.button("leave-discard", t("Don't save"), false).on_click(cx.listener(|app, _, window, cx| {
                        if let Some(then) = app.pending_after_save.take() {
                            then(app, window, cx);
                        }
                        cx.notify();
                    })))
                    .child(self.button("leave-save", t("Save"), true).on_click(cx.listener(|app, _, window, cx| {
                        if app.save_file(cx) {
                            if let Some(then) = app.pending_after_save.take() {
                                then(app, window, cx);
                            }
                        } else {
                            app.pending_after_save = None;
                        }
                        cx.notify();
                    }))),
            );
        Some(self.modal("save-changes", 440., body))
    }

    pub(super) fn files_panel_width(&self) -> f32 {
        self.files_width.unwrap_or_else(|| self.config.f32("filesWidth2", FILES_WIDTH).max(FILES_MIN))
    }

    pub(super) fn render_files_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let f = self.files.as_ref()?;
        let title = f.root.file_name().map_or_else(|| f.root.to_string_lossy().to_string(), |n| n.to_string_lossy().to_string());
        let searching = !f.query.read(cx).value().trim().is_empty();
        let mode = |id: &'static str, label: &'static str, on: bool, by_content: bool| {
            div()
                .id(id)
                .flex()
                .flex_1()
                .justify_center()
                .py(px(5.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(12.5))
                .text_color(if on { c.text } else { c.muted })
                .when(on, |d| d.bg(c.hover).font_weight(gpui::FontWeight::SEMIBOLD))
                .on_click(cx.listener(move |app, _, window, cx| app.set_files_mode(by_content, window, cx)))
                .child(t(label))
        };
        let name_mode = mode("files-mode-name", "Name", !f.by_content, false);
        let content_mode = mode("files-mode-content", "Content", f.by_content, true);
        let root = f.root.clone();
        let list: Vec<AnyElement> = if searching {
            f.hits
                .iter()
                .enumerate()
                .map(|(i, hit)| {
                    let selected = f.selected.as_deref() == Some(hit.rel.as_str());
                    let (open_root, rel, line) = (root.clone(), hit.rel.clone(), hit.line);
                    let name = hit.rel.rsplit('/').next().unwrap_or(&hit.rel).to_string();
                    let place = hit.line.map_or_else(|| hit.rel.clone(), |n| format!("{}:{n}", hit.rel));
                    div()
                        .id(("files-hit", i))
                        .flex()
                        .flex_col()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(c.hover))
                        .hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(move |app, _, _, cx| app.show_file(open_root.clone(), rel.clone(), line, cx)))
                        .child(div().truncate().text_color(c.text).child(name))
                        .child(div().truncate().text_size(px(11.5)).text_color(c.muted).child(place))
                        .when_some(hit.text.clone(), |d, text| d.child(div().truncate().text_size(px(11.5)).font_family("Cascadia Mono").text_color(c.muted).child(text)))
                        .into_any_element()
                })
                .collect()
        } else {
            f.rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let indent = px(8. + row.depth as f32 * 14.);
                    if row.placeholder {
                        return div().pl(indent + px(18.)).py(px(3.)).italic().text_color(c.muted).child(row.name.clone()).into_any_element();
                    }
                    let selected = f.selected.as_deref() == Some(row.rel.as_str());
                    let (dir, rel, open_root) = (row.dir, row.rel.clone(), root.clone());
                    div()
                        .id(("files-row", i))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .pl(indent)
                        .pr(px(8.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(c.hover))
                        .hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if dir {
                                if let Some(f) = app.files.as_mut() {
                                    if !f.expanded.remove(&rel) {
                                        f.expanded.insert(rel.clone());
                                    }
                                }
                                app.reload_tree(cx);
                            } else {
                                app.show_file(open_root.clone(), rel.clone(), None, cx);
                            }
                        }))
                        .child(div().flex_none().w(px(12.)).text_color(c.muted).when(row.dir, |d| d.child(Icon::new(if row.open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)))))
                        .child(div().flex_none().text_color(c.muted).child(if row.dir { Icon::empty().path(assets::FOLDER) } else { Icon::new(IconName::File) }.size(px(15.))))
                        .child(div().min_w(px(0.)).truncate().text_color(if row.ignored { c.muted } else { c.text }).when(row.ignored, |d| d.italic()).child(row.name.clone()))
                        .into_any_element()
                })
                .collect()
        };
        let empty_text = if searching { Some(t("No matches").to_string()) } else { f.error.clone() };
        Some(
            div()
                .relative()
                .flex()
                .flex_none()
                .h_full()
                .child(div().flex_none().w(px(1.)).h_full().bg(c.border))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(self.files_panel_width()))
                        .h_full()
                        .bg(c.main_bg)
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(4.))
                                .h(px(44.))
                                .pl(px(12.))
                                .pr(px(4.))
                                .border_b_1()
                                .border_color(c.border)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(div().truncate().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                                        .child(div().truncate().text_size(px(11.5)).text_color(c.muted).child(f.root.to_string_lossy().to_string())),
                                )
                                .child(self.icon_button("files-refresh", IconName::RefreshCw).on_click(cx.listener(|app, _, _, cx| {
                                    app.reload_tree(cx);
                                    app.search_soon(cx);
                                })))
                                .child(self.icon_button("files-close", IconName::Close).on_click(cx.listener(|app, _, _, cx| {
                                    app.files = None;
                                    cx.notify();
                                }))),
                        )
                        .child(div().flex().flex_none().gap(px(4.)).px(px(8.)).pt(px(8.)).child(name_mode).child(content_mode))
                        .child(div().flex_none().p(px(8.)).child(Input::new(&f.query).small()))
                        .child(
                            div()
                                .id("files-tree")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h(px(0.))
                                .overflow_y_scroll()
                                .px(px(4.))
                                .pb(px(12.))
                                .text_size(px(13.))
                                .map(|d| {
                                    if list.is_empty() {
                                        d.children(empty_text.map(|text| div().p(px(10.)).text_color(c.muted).child(text)))
                                    } else {
                                        d.children(list)
                                    }
                                }),
                        ),
                )
                .child(self.resize_grip("files-resizer", Drag::Files, -5., cx))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_by_name_and_extension() {
        assert_eq!(language_of("src/main.rs"), "rust");
        assert_eq!(language_of("a/B.TSX"), "typescript");
        assert_eq!(language_of("Makefile"), "make");
        assert_eq!(language_of(".env"), "bash");
        assert_eq!(language_of("notes.unknown"), "text");
    }
}
