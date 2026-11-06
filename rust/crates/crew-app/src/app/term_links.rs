//! What a click on a link in a terminal does. Web links open in the side browser (Ctrl+click: default
//! browser). File links and Windows paths open images in a viewer over the app and other files in
//! their default program. "[Image #N]" shows the image on hover and opens it on click.

use std::path::Path;

use crew_core::images;
use crew_core::links::{LinkTarget, file_path_of};
use gpui::{ObjectFit, Point, StyledImage, Task, deferred};

use super::attachments::is_image_path;
use super::overlays::ToastKind;
use super::*;

#[derive(Default)]
pub(super) struct ImageHover {
    /// What's asked for: a later answer to an earlier hover doesn't count.
    wanted: Option<(u64, u32, String)>,
    shown: Option<(PathBuf, Point<Pixels>)>,
}

impl CrewApp {
    pub(super) fn on_terminal_link(&mut self, key: u64, target: LinkTarget, ctrl: bool, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Web(url) if ctrl => cx.open_url(&url),
            LinkTarget::Web(url) => self.open_browser(Some(url), window, cx),
            LinkTarget::File(link) => {
                let path = PathBuf::from(file_path_of(&link));
                if is_image_path(&path) {
                    self.image_view = Some(path);
                } else if Path::new(&path).exists() {
                    cx.open_with_system(&path);
                }
            }
            LinkTarget::Image { n, context } => {
                let finding = self.image_for(key, n, context, cx);
                cx.spawn(async move |this, cx| {
                    let found = finding.await;
                    let _ = this.update(cx, |app, cx| {
                        match found {
                            Some(path) => app.image_view = Some(path),
                            None => app.toast(t("The image is not in the session yet"), ToastKind::Plain, cx),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    pub(super) fn on_terminal_image_hover(&mut self, key: u64, hover: Option<(u32, String, Point<Pixels>)>, cx: &mut Context<Self>) {
        let Some((n, context, at)) = hover else {
            self.image_hover = ImageHover::default();
            cx.notify();
            return;
        };
        let wanted = (key, n, context.clone());
        self.image_hover = ImageHover { wanted: Some(wanted.clone()), shown: None };
        let finding = self.image_for(key, n, context, cx);
        cx.spawn(async move |this, cx| {
            let found = finding.await;
            let _ = this.update(cx, |app, cx| {
                if app.image_hover.wanted.as_ref() == Some(&wanted) {
                    app.image_hover.shown = found.map(|path| (path, at));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The transcript has it once the agent wrote it down; until then, the images this box sent.
    /// Claude counts them across the session, Codex per message (the latest message with that many).
    fn image_for(&self, key: u64, n: u32, context: String, cx: &mut Context<Self>) -> Task<Option<PathBuf>> {
        let Some(o) = self.by_key(key) else { return Task::ready(None) };
        let at = (n as usize).saturating_sub(1);
        let sent = if o.agent == "codex" { o.sent_images.iter().rev().find(|m| m.len() >= n as usize).and_then(|m| m.get(at)).cloned() } else { o.sent_images.iter().flatten().nth(at).cloned() };
        let (paths, session) = (self.paths.clone(), o.id.clone());
        cx.background_spawn(async move { images::find(&paths, &session, n, &context).or(sent) })
    }

    /// The small picture beside the mouse.
    pub(super) fn render_image_hover(&self, window: &Window) -> Option<AnyElement> {
        let c = self.colors;
        let (path, at) = self.image_hover.shown.clone()?;
        let left = at.x.min(window.viewport_size().width - px(300.)) + px(12.);
        let top = (at.y - px(230.)).max(px(8.));
        Some(
            deferred(
                div()
                    .absolute()
                    .left(left)
                    .top(top)
                    .p(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.main_bg)
                    .child(img(path).w(px(280.)).h(px(210.)).rounded(px(6.)).object_fit(ObjectFit::ScaleDown)),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}
