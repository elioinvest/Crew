//! Dragging in the sidebar: a project above or below another to order them, a session onto a pane to
//! show it there.

use gpui::{Div, DragMoveEvent, Stateful};

use super::*;

#[derive(Clone)]
pub(super) struct DraggedProject {
    path: PathBuf,
    name: String,
    colors: Colors,
}

#[derive(Clone)]
pub(super) struct DraggedSession {
    session: Session,
    project: PathBuf,
    colors: Colors,
}

/// What follows the mouse: the name of what's being dragged.
struct DragLabel {
    text: String,
    colors: Colors,
}

impl Render for DragLabel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors;
        div()
            .px(px(10.))
            .py(px(4.))
            .max_w(px(260.))
            .truncate()
            .rounded(px(7.))
            .border_1()
            .border_color(c.border)
            .bg(c.card)
            .text_size(px(13.))
            .text_color(c.text)
            .child(self.text.clone())
    }
}

impl CrewApp {
    /// Lets a project row be dragged, and shows where a dragged one would land.
    pub(super) fn project_drag_handlers(&self, row: Stateful<Div>, project: &Project, cx: &mut Context<Self>) -> Stateful<Div> {
        let dragged = DraggedProject { path: project.path.clone(), name: project.name.clone(), colors: self.colors };
        let (over, onto) = (project.path.clone(), project.path.clone());
        let accent = self.colors.accent;
        let line = self.project_drop.as_ref().filter(|(p, _)| same_folder(p, &project.path)).map(|(_, after)| *after);
        row.relative()
            .on_drag(dragged, |d: &DraggedProject, _, _, cx| {
                let (text, colors) = (d.name.clone(), d.colors);
                cx.new(|_| DragLabel { text, colors })
            })
            .on_drag_move::<DraggedProject>(cx.listener(move |app, e: &DragMoveEvent<DraggedProject>, _, cx| {
                let position = e.event.position;
                let here = if e.bounds.contains(&position) { Some(position.y > e.bounds.origin.y + e.bounds.size.height / 2.) } else { None };
                match here {
                    Some(after) => {
                        let changed = !app.project_drop.as_ref().is_some_and(|(p, a)| same_folder(p, &over) && *a == after);
                        if changed {
                            app.project_drop = Some((over.clone(), after));
                            cx.notify();
                        }
                    }
                    None => {
                        if app.project_drop.as_ref().is_some_and(|(p, _)| same_folder(p, &over)) {
                            app.project_drop = None;
                            cx.notify();
                        }
                    }
                }
            }))
            .on_drop::<DraggedProject>(cx.listener(move |app, d: &DraggedProject, _, cx| {
                let after = app.project_drop.take().is_some_and(|(_, a)| a);
                app.move_project(&d.path, &onto, after, cx);
            }))
            .when_some(line, |d, after| {
                d.child(div().absolute().left(px(0.)).right(px(0.)).h(px(2.)).bg(accent).map(|l| if after { l.bottom(px(0.)) } else { l.top(px(0.)) }))
            })
    }

    /// Moves project `from` next to project `to` (after it, or before it), and saves the order.
    fn move_project(&mut self, from: &Path, to: &Path, after: bool, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = self.projects.iter().map(|p| p.path.clone()).filter(|p| !same_folder(p, from)).collect();
        let Some(i) = paths.iter().position(|p| same_folder(p, to)) else {
            cx.notify();
            return;
        };
        paths.insert(if after { i + 1 } else { i }, from.to_path_buf());
        let order: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
        self.save_setting("projectOrder", json!(order));
        self.rescan(cx);
    }

    /// Lets a session row be dragged onto a pane.
    pub(super) fn session_drag_handler(&self, row: Stateful<Div>, session: &Session, project: &Path) -> Stateful<Div> {
        let dragged = DraggedSession { session: session.clone(), project: project.to_path_buf(), colors: self.colors };
        row.on_drag(dragged, |d: &DraggedSession, _, _, cx| {
            let (text, colors) = (d.session.title.clone().unwrap_or_else(|| t("New session").to_string()), d.colors);
            cx.new(|_| DragLabel { text, colors })
        })
    }

    /// Lets a pane take a dragged session.
    pub(super) fn pane_drop_handlers(&self, pane: Stateful<Div>, leaf: u64, cx: &mut Context<Self>) -> Stateful<Div> {
        let accent = self.colors.accent;
        pane.drag_over::<DraggedSession>(move |style, _, _, _| style.border_2().border_color(accent))
            .on_drop::<DraggedSession>(cx.listener(move |app, d: &DraggedSession, window, cx| app.open_in_pane(leaf, d, window, cx)))
    }

    /// Dropping a session that's in another pane swaps the two panes' sessions.
    fn open_in_pane(&mut self, leaf: u64, d: &DraggedSession, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.layout.focused_session();
        let open = self.open.iter().find(|o| o.id == d.session.id && o.agent == d.session.agent).map(|o| o.key);
        match open {
            Some(key) => {
                self.layout.place(key, leaf);
                self.focus_changed(previous, window, cx);
            }
            None => {
                self.layout.focus = leaf;
                self.open_session(&d.session, &d.project, window, cx);
            }
        }
        cx.notify();
    }

    /// The split shape is remembered across launches; the panes come back empty.
    pub(super) fn save_layout_if_changed(&mut self) {
        let shape = self.layout.shape();
        if shape != self.saved_layout {
            self.saved_layout = shape.clone();
            self.save_setting("paneLayout", shape);
        }
    }
}
