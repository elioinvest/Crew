//! Files and images going with a message: the + button, a pasted screenshot, files dropped on the
//! box. Claude and Codex both turn a pasted image path into "[Image #N]", the same as pasting a
//! screenshot into them directly; other files are referenced in the text.

use gpui::{ClipboardEntry, ExternalPaths, ImageFormat, ObjectFit, StyledImage};

use super::*;
use crate::i18n::zh;

#[derive(Clone, PartialEq)]
pub(super) struct Attachment {
    pub(super) path: PathBuf,
    pub(super) is_image: bool,
}

pub(super) fn is_image_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| ["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// As a PNG, for what the agents can't take as an image: a path to anything else stays plain text in
/// their prompt, and the picture never reaches them. Windows hands over screenshots as BMP.
fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut picture = image::load_from_memory(bytes).ok()?.to_rgba8();
    // A screenshot's fourth byte is often unused, and left at zero it would read as fully transparent.
    if picture.pixels().all(|p| p[3] == 0) {
        picture.pixels_mut().for_each(|p| p[3] = 255);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

fn unique_stamp() -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let unique = uuid::Uuid::new_v4().simple().to_string();
    format!("{stamp}-{}", &unique[..6])
}

/// A dropped or picked image the agents can't take, copied with Crew's pasted images as a PNG. The
/// original stays where it is.
fn png_copy(path: &Path, dir: &Path) -> Option<PathBuf> {
    let png = to_png(&std::fs::read(path).ok()?)?;
    let file = dir.join(format!("dropped-{}.png", unique_stamp()));
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&file, png).ok()?;
    Some(file)
}

fn agents_take(path: &Path) -> bool {
    !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bmp"))
}

impl CrewApp {
    fn attachment_dir(&self) -> PathBuf {
        self.paths.data_dir.join("attachments")
    }

    /// Pasted images are only needed for a while: older ones go at startup.
    pub(super) fn clean_attachments(&self, cx: &mut Context<Self>) {
        let dir = self.attachment_dir();
        cx.background_spawn(async move {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(7 * 24 * 3600));
                if old {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        })
        .detach();
    }

    fn add_attachments(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.layout.focused_session() else { return };
        let Some(o) = self.open.iter_mut().find(|o| o.key == key && o.agent != "ssh") else { return };
        let dir = self.paths.data_dir.join("attachments");
        for path in paths {
            let path = if agents_take(&path) { path } else { png_copy(&path, &dir).unwrap_or(path) };
            if !o.attachments.iter().any(|a| a.path == path) {
                let is_image = is_image_path(&path);
                o.attachments.push(Attachment { path, is_image });
            }
        }
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = cx.update(|cx| {
                let Some(window) = cx.active_window() else { return };
                let _ = window.update(cx, |_, window, cx| {
                    let _ = this.update(cx, |app, cx| app.add_attachments(paths, window, cx));
                });
            });
        })
        .detach();
    }

    /// Ctrl+V of a screenshot, or of files copied in Explorer. Plain text is left to the box.
    pub(super) fn paste_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.active().is_none_or(|o| o.agent == "ssh") {
            return false;
        }
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut paths = Vec::new();
        let mut text = None;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()),
                ClipboardEntry::Image(image) => {
                    let converted = if matches!(image.format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Webp) { None } else { to_png(&image.bytes) };
                    let ext = match image.format {
                        _ if converted.is_some() => "png",
                        ImageFormat::Jpeg => "jpg",
                        ImageFormat::Gif => "gif",
                        ImageFormat::Webp => "webp",
                        ImageFormat::Bmp => "bmp",
                        _ => "png",
                    };
                    let dir = self.attachment_dir();
                    let file = dir.join(format!("pasted-{}.{ext}", unique_stamp()));
                    let bytes = converted.as_deref().unwrap_or(&image.bytes);
                    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, bytes).is_ok() {
                        paths.push(file);
                    }
                }
                ClipboardEntry::String(s) => text = Some(s.text().clone()),
            }
        }
        if paths.is_empty() {
            return false;
        }
        // Text copied together with an image (from a browser, say) still goes in the box.
        if let Some(text) = text.filter(|_| paths.iter().any(|p| p.starts_with(self.attachment_dir()))) {
            self.composer.update(cx, |c, cx| c.insert(text, window, cx));
        }
        self.add_attachments(paths, window, cx);
        true
    }

    pub(super) fn drop_attachments(&mut self, dropped: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.add_attachments(dropped.paths().to_vec(), window, cx);
    }

    /// Sends the session's files with the message, then Enter. Runs on its own: images need a
    /// moment each for the agent to take them in.
    pub(super) fn send_with_attachments(&mut self, key: u64, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = self.open.iter_mut().find(|o| o.key == key) else { return };
        let files = std::mem::take(&mut o.attachments);
        let codex = o.agent == "codex";
        let sent: Vec<PathBuf> = files.iter().filter(|f| f.is_image).map(|f| f.path.clone()).collect();
        if !sent.is_empty() {
            o.sent_images.push(sent);
        }
        let term = o.view.read(cx).term.clone();
        self.composer.update(cx, |c, cx| c.set_value("", window, cx));
        let refs: Vec<String> = files
            .iter()
            .filter(|f| !f.is_image)
            .map(|f| {
                let p = f.path.to_string_lossy();
                let quoted = if p.contains(char::is_whitespace) { format!("\"{p}\"") } else { p.to_string() };
                if codex { quoted } else { format!("@{quoted}") }
            })
            .collect();
        let joined = [refs.join(" "), text.trim().to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        let body = self.with_skills(key, &joined);
        let body = agents_panel::with_agent_note(&body, &self.roster).replace("\r\n", "\r").replace('\n', "\r");
        let images: Vec<String> = files.iter().filter(|f| f.is_image).map(|f| f.path.to_string_lossy().to_string()).collect();
        cx.spawn(async move |_, cx| {
            let ex = cx.background_executor().clone();
            for image in &images {
                term.write(format!("\x1b[200~{image}\x1b[201~").as_bytes());
                ex.timer(Duration::from_millis(350)).await;
            }
            if !body.is_empty() {
                let gap = if images.is_empty() { "" } else { " " };
                term.write(format!("\x1b[200~{gap}{body}\x1b[201~").as_bytes());
            }
            ex.timer(Duration::from_millis(300)).await;
            term.write(b"\r");
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_attach_button(&self, o: &OpenSession, cx: &mut Context<Self>) -> Option<AnyElement> {
        (o.agent != "ssh").then(|| self.icon_button("composer-attach", IconName::Plus).on_click(cx.listener(|app, _, _, cx| app.pick_attachments(cx))).into_any_element())
    }

    pub(super) fn render_attachments(&self, o: &OpenSession, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        if o.attachments.is_empty() {
            return None;
        }
        let key = o.key;
        let cards: Vec<AnyElement> = o
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let group = SharedString::from(format!("attachment-{i}"));
                let gone = a.path.clone();
                let remove = div()
                    .id(("attachment-remove", i))
                    .absolute()
                    .top(px(if a.is_image { 4. } else { 10. }))
                    .right(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded_full()
                    .when(a.is_image, |d| d.bg(gpui::black().opacity(0.6)).text_color(gpui::white()))
                    .when(!a.is_image, |d| d.text_color(c.muted))
                    .cursor_pointer()
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(o) = app.open.iter_mut().find(|o| o.key == key) {
                            o.attachments.retain(|x| x.path != gone);
                        }
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::Close).size(px(12.)));
                let card = div().id(("attachment", i)).group(group).relative().flex_none().overflow_hidden().rounded(px(12.)).border_1().border_color(c.border).bg(c.side_bg);
                let name = a.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().to_string());
                if a.is_image {
                    let shown = a.path.clone();
                    card.size(px(72.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.image_view = Some(shown.clone());
                            cx.notify();
                        }))
                        .child(img(a.path.clone()).size_full().object_fit(ObjectFit::Cover))
                        .child(remove)
                        .into_any_element()
                } else {
                    card.flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(40.))
                        .max_w(px(240.))
                        .pl(px(10.))
                        .pr(px(28.))
                        .text_size(px(13.))
                        .text_color(c.text)
                        .child(div().flex_none().text_color(c.muted).child(Icon::new(IconName::File).size(px(15.))))
                        .child(div().min_w(px(0.)).truncate().child(name))
                        .child(remove)
                        .into_any_element()
                }
            })
            .collect();
        Some(div().flex().flex_wrap().gap(px(8.)).mb(px(10.)).children(cards).into_any_element())
    }

    /// A clicked thumbnail, over the app.
    pub(super) fn render_image_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.image_view.clone()?;
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().to_string());
        let (open, show, copy) = (path.clone(), path.clone(), path.to_string_lossy().to_string());
        let unreadable = if zh() { t("Couldn't load this image. It may have been moved or deleted.") } else { "Couldn't load this image. It may have been moved or deleted." };
        let bar_button = |id: &'static str, label: &'static str| {
            div().id(id).px(px(10.)).py(px(4.)).rounded(px(7.)).cursor_pointer().text_size(px(13.)).text_color(gpui::white()).hover(|s| s.bg(gpui::white().opacity(0.15))).child(if zh() { t(label) } else { label })
        };
        Some(
            div()
                .id("image-view")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .flex_col()
                .bg(gpui::rgba(0x141414d1))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.image_view = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.))
                        .h(px(52.))
                        .px(px(20.))
                        .text_color(gpui::white())
                        .child(div().flex_1().min_w(px(0.)).truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(name))
                        .child(bar_button("image-view-open", "Open").on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.open_with_system(&open);
                        })))
                        .child(bar_button("image-view-show", "Show in folder").on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.reveal_path(&show);
                        })))
                        .child(bar_button("image-view-copy", "Copy path").on_click(cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.copy_text(copy.clone(), cx);
                        })))
                        .child(self.icon_button("image-view-close", IconName::Close).text_color(gpui::white()).on_click(cx.listener(|app, _, _, cx| {
                            cx.stop_propagation();
                            app.image_view = None;
                            cx.notify();
                        }))),
                )
                .child(div().flex_1().min_h(px(0.)).p(px(20.)).child(img(path).size_full().object_fit(ObjectFit::ScaleDown).with_fallback(move || div().size_full().flex().items_center().justify_center().text_size(px(14.)).text_color(gpui::white()).child(unreadable).into_any_element())))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BMP the way the Windows clipboard gives it: bottom-up rows, 24 or 32 bits a pixel.
    fn bmp(bits: u16, pixel: &[u8]) -> Vec<u8> {
        let row = (2 * pixel.len()).next_multiple_of(4);
        let size = row * 2;
        let mut out = b"BM".to_vec();
        out.extend(((54 + size) as u32).to_le_bytes());
        out.extend([0; 4]);
        out.extend(54u32.to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend(2i32.to_le_bytes());
        out.extend(2i32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(bits.to_le_bytes());
        out.extend([0; 24]);
        for _ in 0..2 {
            out.extend(pixel);
            out.extend(pixel);
            out.resize(out.len() + row - 2 * pixel.len(), 0);
        }
        out
    }

    fn colour_of(png: &[u8]) -> [u8; 4] {
        assert_eq!(&png[1..4], b"PNG");
        image::load_from_memory(png).unwrap().to_rgba8().get_pixel(0, 0).0
    }

    #[test]
    fn a_screenshot_becomes_an_opaque_png() {
        // Blue, green, red in the file.
        assert_eq!(colour_of(&to_png(&bmp(24, &[10, 20, 30])).unwrap()), [30, 20, 10, 255]);
        assert_eq!(colour_of(&to_png(&bmp(32, &[10, 20, 30, 0])).unwrap()), [30, 20, 10, 255], "an unused fourth byte isn't transparency");
        assert!(to_png(b"not a picture").is_none());
    }

    #[test]
    fn only_what_the_agents_cant_take_is_copied() {
        let dir = std::env::temp_dir().join(format!("crew-attach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("shot.BMP");
        std::fs::write(&original, bmp(24, &[1, 2, 3])).unwrap();
        assert!(!agents_take(&original));
        assert!(agents_take(Path::new("a.png")) && agents_take(Path::new("a.JPG")) && agents_take(Path::new("a.webp")));
        let copy = png_copy(&original, &dir.join("attachments")).unwrap();
        assert_eq!(copy.extension().unwrap(), "png");
        assert_eq!(colour_of(&std::fs::read(&copy).unwrap()), [3, 2, 1, 255]);
        assert!(original.exists(), "the original stays");
        let broken = dir.join("broken.bmp");
        std::fs::write(&broken, b"nope").unwrap();
        assert!(png_copy(&broken, &dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
