//! A terminal on screen: paints a crew-term [`Screen`] cell by cell on GPUI's grid-free text system,
//! and turns keys, the mouse and IME input into what the program expects.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use crew_core::agents::is_busy;
use crew_core::links::{self, LinkTarget};
use crew_term::{CellView, CursorKind, Mode, Palette, Screen, TermEvent, Terminal};
use gpui::{
    App, Bounds, ClipboardItem, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    Font, FontFallbacks, FontStyle, FontWeight, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollEvent, SharedString, StrikethroughStyle, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, rgb, size,
};

pub const FONT_FAMILY: &str = "Cascadia Mono";
const FONT_FALLBACKS: [&str; 3] = ["Consolas", "Microsoft YaHei Mono", "Microsoft YaHei"];
const LINE_HEIGHT: f32 = 1.15;
const PADDING_LEFT: f32 = 16.0;
const PADDING_RIGHT: f32 = 6.0;
const PADDING_TOP: f32 = 10.0;
const PADDING_BOTTOM: f32 = 6.0;

fn color(c: u32) -> Hsla {
    rgb(c).into()
}

pub struct TerminalView {
    pub term: Arc<Terminal>,
    focus: FocusHandle,
    font_size: Pixels,
    /// Text the IME is composing, shown at the cursor and not yet sent.
    marked: Option<String>,
    bounds: Bounds<Pixels>,
    cell: gpui::Size<Pixels>,
    scroll_rest: f32,
    selecting: bool,
    exited: bool,
    palette: Palette,
    busy: bool,
    screen_read_pending: bool,
    /// Where the mouse is over this terminal, and the link under it.
    mouse: Option<Point<Pixels>>,
    hover: Option<HoverLink>,
    pressed_link: Option<HoverLink>,
}

/// A link on screen: what it opens, and its cells as (row, first column, column after the last).
#[derive(Clone, PartialEq)]
struct HoverLink {
    target: LinkTarget,
    cells: Vec<(usize, usize, usize)>,
}

pub enum TerminalEvent {
    Exited,
    /// Enter was pressed after it stopped: bring it back.
    Restart,
    /// Whether the agent is mid-task, read off its status line.
    Busy(bool),
    /// A link was clicked, with Ctrl or without.
    OpenLink(LinkTarget, bool),
    /// The mouse went onto an "[Image #N]", with its line's text; or off it.
    HoverImage(Option<(u32, String, Point<Pixels>)>),
}

impl gpui::EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    pub fn new(term: Arc<Terminal>, palette: Palette, font_size: f32, cx: &mut Context<Self>) -> Self {
        let events = term.events.clone();
        let watched = term.clone();
        // Screen updates arrive in bursts: a frame's worth at a time is all that can be seen.
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                match event {
                    TermEvent::Changed => {
                        cx.background_executor().timer(Duration::from_millis(8)).await;
                        watched.take_changed();
                        let _ = this.update(cx, |v: &mut Self, cx| {
                            v.schedule_screen_read(cx);
                            v.refresh_hover(cx);
                        });
                    }
                    TermEvent::Exited => {
                        let _ = this.update(cx, |v, cx| {
                            v.exited = true;
                            cx.emit(TerminalEvent::Exited);
                        });
                    }
                    TermEvent::Title | TermEvent::Bell => {}
                }
                if this.update(cx, |v, cx| {
                    if let Some(text) = v.term.take_clipboard() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.notify();
                }).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            term,
            focus: cx.focus_handle(),
            font_size: px(font_size),
            marked: None,
            bounds: Bounds::default(),
            cell: size(px(8.), px(16.)),
            scroll_rest: 0.,
            selecting: false,
            exited: false,
            palette,
            busy: false,
            screen_read_pending: false,
            mouse: None,
            hover: None,
            pressed_link: None,
        }
    }

    pub fn set_appearance(&mut self, palette: Palette, font_size: f32, cx: &mut Context<Self>) {
        self.term.set_palette(palette.clone());
        self.palette = palette;
        self.font_size = px(font_size);
        cx.notify();
    }

    /// The screen is read at most this often: output can arrive much faster than it matters.
    fn schedule_screen_read(&mut self, cx: &mut Context<Self>) {
        if self.screen_read_pending {
            return;
        }
        self.screen_read_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let _ = this.update(cx, |v, cx| {
                v.screen_read_pending = false;
                let busy = !v.exited && is_busy(&v.term.screen_text());
                if busy != v.busy {
                    v.busy = busy;
                    cx.emit(TerminalEvent::Busy(busy));
                }
            });
        })
        .detach();
    }

    fn font(&self, bold: bool, italic: bool) -> Font {
        Font {
            family: FONT_FAMILY.into(),
            features: Default::default(),
            fallbacks: Some(FontFallbacks::from_fonts(FONT_FALLBACKS.iter().map(|s| s.to_string()).collect())),
            weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
            style: if italic { FontStyle::Italic } else { FontStyle::Normal },
        }
    }

    fn origin(&self) -> Point<Pixels> {
        self.bounds.origin + point(px(PADDING_LEFT), px(PADDING_TOP))
    }

    /// The cell under a window position, and whether it's on the cell's right half.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, bool) {
        let p = position - self.origin();
        let (cols, rows) = self.term.size();
        let x = (p.x / self.cell.width).max(0.);
        let y = (p.y / self.cell.height).max(0.);
        let col = (x as usize).min(cols.saturating_sub(1) as usize);
        let row = (y as usize).min(rows.saturating_sub(1) as usize);
        (col, row, x.fract() >= 0.5)
    }

    /// The link under a window position: one a program printed, a web address, a path, an image.
    fn link_at(&self, position: Point<Pixels>) -> Option<HoverLink> {
        let p = position - self.origin();
        let (cols, rows) = self.term.size();
        if p.x < px(0.) || p.y < px(0.) || p.x >= self.cell.width * cols as f32 || p.y >= self.cell.height * rows as f32 {
            return None;
        }
        let (col, row, _) = self.cell_at(position);
        if let Some((uri, start, end)) = self.term.hyperlink_at(col, row) {
            return links::target_of_uri(&uri).map(|target| HoverLink { target, cells: vec![(row, start, end)] });
        }
        let (text, places) = self.term.logical_line(row);
        let here = (row as i32, col);
        links::find_links(&text).into_iter().find_map(|link| {
            let first = *places.get(link.start)?;
            let last = *places.get(link.end.checked_sub(1)?)?;
            let end = (last.0, last.1 + last.2 - 1);
            if here < (first.0, first.1) || here > end {
                return None;
            }
            let cells = (first.0.max(0)..=last.0.min(rows as i32 - 1))
                .map(|r| (r as usize, if r == first.0 { first.1 } else { 0 }, if r == last.0 { end.1 + 1 } else { cols as usize }))
                .collect();
            Some(HoverLink { target: link.target, cells })
        })
    }

    /// The link under the mouse, again: the mouse moved, or the text under it did.
    fn refresh_hover(&mut self, cx: &mut Context<Self>) {
        let link = self.mouse.filter(|_| !self.selecting).and_then(|p| self.link_at(p));
        if link == self.hover {
            return;
        }
        let image = |l: &Option<HoverLink>| match l.as_ref().map(|l| &l.target) {
            Some(LinkTarget::Image { n, context }) => Some((*n, context.clone())),
            _ => None,
        };
        let (before, now) = (image(&self.hover), image(&link));
        self.hover = link;
        if before != now {
            let at = self.mouse.unwrap_or_default();
            cx.emit(TerminalEvent::HoverImage(now.map(|(n, context)| (n, context, at))));
        }
        cx.notify();
    }

    fn write_input(&mut self, bytes: &[u8]) {
        self.term.clear_selection();
        self.term.scroll_to_bottom();
        self.term.write(bytes);
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.term.mode().contains(Mode::BRACKETED_PASTE) {
            let clean = text.replace('\x1b', "");
            self.write_input(format!("\x1b[200~{clean}\x1b[201~").as_bytes());
        } else {
            self.write_input(text.as_bytes());
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = k.modifiers;
        if self.exited && k.key == "enter" {
            cx.emit(TerminalEvent::Restart);
            cx.stop_propagation();
            return;
        }
        if m.control && !m.alt && !m.platform {
            // Copy and paste, the way Windows terminals do them.
            let key = k.key.as_str();
            if (key == "c" && (m.shift || self.term.selected_text().is_some())) || (key == "insert") {
                if let Some(text) = self.term.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.term.clear_selection();
                    cx.stop_propagation();
                    return;
                }
            }
            if key == "v" {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.paste(&text);
                }
                cx.stop_propagation();
                return;
            }
        }
        if m.shift && k.key == "insert" {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.paste(&text);
            }
            cx.stop_propagation();
            return;
        }
        // Scrolling the history stays out of the program's way.
        if m.shift && !m.control && !self.term.mode().contains(Mode::ALT_SCREEN) {
            let (_, rows) = self.term.size();
            match k.key.as_str() {
                "pageup" => {
                    self.term.scroll(rows as i32 - 1);
                    cx.stop_propagation();
                    return;
                }
                "pagedown" => {
                    self.term.scroll(-(rows as i32 - 1));
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        if let Some(bytes) = key_bytes(k, self.term.mode()) {
            self.write_input(&bytes);
            cx.stop_propagation();
            cx.notify();
        }
        // Anything else is text: it comes through the input handler, with the IME's help.
    }

    fn mouse_report(&self, button: u8, col: usize, row: usize, down: bool) {
        let c = col + 1;
        let r = row + 1;
        let mode = self.term.mode();
        let seq = if mode.contains(Mode::SGR_MOUSE) {
            format!("\x1b[<{button};{c};{r}{}", if down { 'M' } else { 'm' })
        } else {
            let b = if down { button + 32 } else { 3 + 32 };
            let enc = |v: usize| char::from_u32((v.min(223) + 32) as u32).unwrap_or(' ');
            format!("\x1b[M{}{}{}", char::from_u32(b as u32).unwrap_or(' '), enc(c), enc(r))
        };
        self.term.write(seq.as_bytes());
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.pressed_link = if e.button == MouseButton::Left { self.link_at(e.position) } else { None };
        let (col, row, right) = self.cell_at(e.position);
        let mode = self.term.mode();
        if mode.intersects(Mode::MOUSE_MODE) && !e.modifiers.shift {
            let button = match e.button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => return,
            };
            self.mouse_report(button, col, row, true);
            return;
        }
        if e.button == MouseButton::Left {
            self.term.start_selection(col, row, right, e.click_count == 2);
            self.selecting = true;
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.mouse = Some(e.position);
        if self.selecting {
            let (col, row, right) = self.cell_at(e.position);
            self.term.update_selection(col, row, right);
            cx.notify();
        }
        self.refresh_hover(cx);
    }

    /// Pressed and released on the same link, without dragging out a selection, opens it.
    fn finish_link_click(&mut self, e: &MouseUpEvent, cx: &mut Context<Self>) {
        let pressed = self.pressed_link.take();
        if e.button != MouseButton::Left || self.term.selected_text().is_some() {
            return;
        }
        if let Some(link) = pressed.filter(|p| self.link_at(e.position).as_ref() == Some(p)) {
            cx.emit(TerminalEvent::OpenLink(link.target, e.modifiers.control));
        }
    }

    fn on_mouse_up(&mut self, e: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.finish_link_click(e, cx);
        let mode = self.term.mode();
        if mode.intersects(Mode::MOUSE_MODE) && !e.modifiers.shift && !self.selecting {
            let (col, row, _) = self.cell_at(e.position);
            let button = match e.button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => return,
            };
            self.mouse_report(button, col, row, false);
            return;
        }
        if self.selecting {
            self.selecting = false;
            // A click without a drag selects nothing.
            if self.term.selected_text().is_none() {
                self.term.clear_selection();
            }
            self.refresh_hover(cx);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, e: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = e.delta.pixel_delta(self.cell.height).y / self.cell.height;
        self.scroll_rest += delta;
        let lines = self.scroll_rest.trunc() as i32;
        self.scroll_rest -= lines as f32;
        if lines == 0 {
            return;
        }
        let mode = self.term.mode();
        if mode.intersects(Mode::MOUSE_MODE) && !e.modifiers.shift {
            // A program that owns the mouse gets a report per row, as the terminal would scroll.
            let (col, row, _) = self.cell_at(e.position);
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.abs() {
                self.mouse_report(button, col, row, true);
            }
        } else if mode.contains(Mode::ALT_SCREEN) && mode.contains(Mode::ALTERNATE_SCROLL) && !e.modifiers.shift {
            let seq: &[u8] = match (lines > 0, mode.contains(Mode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            for _ in 0..lines.abs() {
                self.term.write(seq);
            }
        } else {
            self.term.scroll(lines);
        }
        self.refresh_hover(cx);
        cx.notify();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(color(self.palette.background))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_hover(cx.listener(|v, hovered: &bool, _, cx| {
                if !*hovered {
                    v.mouse = None;
                    v.refresh_hover(cx);
                }
            }))
            .when(self.hover.is_some(), |d| d.cursor_pointer())
            .child(TerminalElement { view: cx.entity() })
    }
}

/// The input handler: what the platform and the IME talk to.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _actual: &mut Option<Range<usize>>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _ignore: bool, _w: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _range: Option<Range<usize>>, text: &str, _w: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if !text.is_empty() {
            self.write_input(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _selected: Option<Range<usize>>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        _element: Bounds<Pixels>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The IME's candidate window opens at the cursor.
        let (col, row) = self.term.screen().cursor.unwrap_or((0, 0));
        let origin = self.origin() + point(self.cell.width * col as f32, self.cell.height * row as f32);
        Some(Bounds::new(origin, self.cell))
    }

    fn character_index_for_point(&mut self, _p: Point<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// What a key sends to the program, for keys that aren't plain text.
fn key_bytes(k: &gpui::Keystroke, mode: Mode) -> Option<Vec<u8>> {
    let m = k.modifiers;
    let app = mode.contains(Mode::APP_CURSOR);
    // xterm's modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    let param = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let csi_letter = |letter: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{letter}").into_bytes()
        } else if app {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if param > 1 { format!("\x1b[{n};{param}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let alt = |bytes: &[u8]| -> Vec<u8> {
        if m.alt {
            let mut v = vec![0x1b];
            v.extend_from_slice(bytes);
            v
        } else {
            bytes.to_vec()
        }
    };
    let out = match k.key.as_str() {
        "enter" if m.shift => b"\x1b\r".to_vec(), // a new line in the agents' prompts
        "enter" => alt(b"\r"),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(b"\t"),
        "escape" => alt(b"\x1b"),
        "backspace" if m.control => alt(b"\x08"),
        "backspace" => alt(b"\x7f"),
        "space" if m.control => alt(b"\x00"),
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "home" => csi_letter('H'),
        "end" => csi_letter('F'),
        "insert" => csi_tilde(2),
        "delete" => csi_tilde(3),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "f1" => csi_letter('P'),
        "f2" => csi_letter('Q'),
        "f3" => csi_letter('R'),
        "f4" => csi_letter('S'),
        "f5" => csi_tilde(15),
        "f6" => csi_tilde(17),
        "f7" => csi_tilde(18),
        "f8" => csi_tilde(19),
        "f9" => csi_tilde(20),
        "f10" => csi_tilde(21),
        "f11" => csi_tilde(23),
        "f12" => csi_tilde(24),
        key if m.control && key.len() == 1 => {
            let c = key.as_bytes()[0].to_ascii_lowercase();
            let code = match c {
                b'a'..=b'z' => c - b'a' + 1,
                b'[' => 0x1b,
                b'\\' => 0x1c,
                b']' => 0x1d,
                b'^' | b'6' => 0x1e,
                b'_' | b'-' => 0x1f,
                b'@' | b'2' => 0,
                _ => return None,
            };
            alt(&[code])
        }
        key if m.alt && !m.control && key.len() == 1 => {
            // Plain Alt+letter; AltGr (Ctrl+Alt) stays text.
            let text = k.key_char.as_deref().unwrap_or(key);
            alt(text.as_bytes())
        }
        _ => return None,
    };
    Some(out)
}

// ---------- painting ----------

struct TerminalElement {
    view: Entity<TerminalView>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct BgRect {
    col: usize,
    row: usize,
    cols: usize,
    color: u32,
}

struct TextBatch {
    col: usize,
    row: usize,
    text: String,
    fg: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct Prepared {
    screen: Screen,
    backgrounds: Vec<BgRect>,
    batches: Vec<TextBatch>,
    cell: gpui::Size<Pixels>,
    baseline_font: Font,
    font_size: Pixels,
    focused: bool,
    marked: Option<String>,
    palette: Palette,
}

/// Runs of cells that can be drawn as one piece of text. A double-width character always gets a
/// run of its own, so its neighbours stay on the grid whatever width the font gives it.
fn batches_of(screen: &Screen) -> (Vec<BgRect>, Vec<TextBatch>) {
    let mut backgrounds = Vec::new();
    let mut batches: Vec<TextBatch> = Vec::new();
    for (y, row) in screen.rows.iter().enumerate() {
        let mut x = 0;
        while x < row.len() {
            let c = &row[x];
            if c.bg != screen.background {
                let start = x;
                let color = c.bg;
                while x < row.len() && row[x].bg == color {
                    x += 1;
                }
                backgrounds.push(BgRect { col: start, row: y, cols: x - start, color });
            } else {
                x += 1;
            }
        }
        let mut x = 0;
        while x < row.len() {
            let c: &CellView = &row[x];
            if c.spacer || (c.ch == ' ' && !c.underline && !c.strike) {
                x += 1;
                continue;
            }
            let start = x;
            let mut text = String::new();
            text.push(c.ch);
            x += 1;
            if !c.wide && c.ch.is_ascii() {
                while x < row.len() {
                    let n = &row[x];
                    let same = n.fg == c.fg && n.bold == c.bold && n.italic == c.italic && n.underline == c.underline && n.strike == c.strike;
                    if !same || n.wide || n.spacer || !n.ch.is_ascii() {
                        break;
                    }
                    text.push(n.ch);
                    x += 1;
                }
            }
            let trimmed = if c.underline || c.strike { text.len() } else { text.trim_end().len() };
            text.truncate(trimmed);
            batches.push(TextBatch { col: start, row: y, text, fg: c.fg, bold: c.bold, italic: c.italic, underline: c.underline, strike: c.strike });
        }
    }
    (backgrounds, batches)
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepared;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepared {
        let (font, font_size, focused, marked, palette) = {
            let v = self.view.read(cx);
            (v.font(false, false), v.font_size, v.focus.is_focused(window), v.marked.clone(), v.palette.clone())
        };
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&font);
        let width = text_system.advance(font_id, font_size, 'm').map(|s| s.width).unwrap_or(font_size * 0.6);
        let height = (font_size * LINE_HEIGHT).round();
        let cell = size(width, height);
        let inner_w = bounds.size.width - px(PADDING_LEFT + PADDING_RIGHT);
        let inner_h = bounds.size.height - px(PADDING_TOP + PADDING_BOTTOM);
        let cols = ((inner_w / width) as i32).max(2) as u16;
        let rows = ((inner_h / height) as i32).max(1) as u16;
        let screen = self.view.update(cx, |v, _| {
            v.bounds = bounds;
            v.cell = cell;
            v.term.resize(cols, rows, f32::from(width) as u16, f32::from(height) as u16);
            v.term.screen()
        });
        let (backgrounds, batches) = batches_of(&screen);
        Prepared { screen, backgrounds, batches, cell, baseline_font: font, font_size, focused, marked, palette }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        p: &mut Prepared,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        let origin = bounds.origin + point(px(PADDING_LEFT), px(PADDING_TOP));
        let cell = p.cell;
        let at = |col: usize, row: usize| origin + point(cell.width * col as f32, cell.height * row as f32);

        for b in &p.backgrounds {
            window.paint_quad(fill(Bounds::new(at(b.col, b.row), size(cell.width * b.cols as f32, cell.height)), color(b.color)));
        }

        let text_system = window.text_system().clone();
        let view = self.view.read(cx);
        let fonts = [view.font(false, false), view.font(true, false), view.font(false, true), view.font(true, true)];
        for b in &p.batches {
            let font = fonts[b.bold as usize + 2 * b.italic as usize].clone();
            let fg = color(b.fg);
            let run = TextRun {
                len: b.text.len(),
                font,
                color: fg,
                background_color: None,
                underline: b.underline.then(|| UnderlineStyle { color: Some(fg), thickness: px(1.), wavy: false }),
                strikethrough: b.strike.then(|| StrikethroughStyle { color: Some(fg), thickness: px(1.) }),
            };
            let text: SharedString = b.text.clone().into();
            // Glyphs stay on the grid even if a fallback font is a little wider or narrower.
            let line = text_system.shape_line(text, p.font_size, &[run], Some(cell.width));
            let _ = line.paint(at(b.col, b.row), cell.height, gpui::TextAlign::Left, None, window, cx);
        }

        // The link under the mouse, underlined in its own color.
        if let Some(link) = self.view.read(cx).hover.as_ref() {
            for (row, start, end) in &link.cells {
                let fg = p.screen.rows.get(*row).and_then(|r| r.get(*start)).map_or(p.palette.foreground, |c| c.fg);
                let line = Bounds::new(at(*start, *row) + point(px(0.), cell.height - px(1.)), size(cell.width * (end - start) as f32, px(1.)));
                window.paint_quad(fill(line, color(fg)));
            }
        }

        // The cursor: solid while focused, an outline otherwise, the composing text over it.
        if let Some((col, row)) = p.screen.cursor {
            if p.screen.cursor_shape != CursorKind::Hidden && p.screen.display_offset == 0 {
                let here = at(col, row);
                let under = p.screen.rows.get(row).and_then(|r| r.get(col)).copied();
                let wide = under.is_some_and(|c| c.wide);
                let w = if wide { cell.width * 2. } else { cell.width };
                let cursor = color(p.palette.cursor);
                if let Some(marked) = p.marked.as_ref().filter(|m| !m.is_empty()) {
                    let run = TextRun {
                        len: marked.len(),
                        font: p.baseline_font.clone(),
                        color: color(p.palette.foreground),
                        background_color: Some(color(p.palette.selection)),
                        underline: Some(UnderlineStyle { color: Some(color(p.palette.foreground)), thickness: px(1.), wavy: false }),
                        strikethrough: None,
                    };
                    let line = text_system.shape_line(marked.clone().into(), p.font_size, &[run], None);
                    let _ = line.paint(here, cell.height, gpui::TextAlign::Left, None, window, cx);
                } else if !p.focused {
                    window.paint_quad(gpui::outline(Bounds::new(here, size(w, cell.height)), cursor, gpui::BorderStyle::Solid));
                } else {
                    match p.screen.cursor_shape {
                        CursorKind::Bar => window.paint_quad(fill(Bounds::new(here, size(px(2.), cell.height)), cursor)),
                        CursorKind::Underline => window.paint_quad(fill(
                            Bounds::new(here + point(px(0.), cell.height - px(2.)), size(w, px(2.))),
                            cursor,
                        )),
                        _ => {
                            window.paint_quad(fill(Bounds::new(here, size(w, cell.height)), cursor));
                            if let Some(c) = under.filter(|c| c.ch != ' ') {
                                let s = c.ch.to_string();
                                let run = TextRun {
                                    len: s.len(),
                                    font: fonts[c.bold as usize + 2 * c.italic as usize].clone(),
                                    color: color(p.palette.cursor_text),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                let line = text_system.shape_line(s.into(), p.font_size, &[run], None);
                                let _ = line.paint(here, cell.height, gpui::TextAlign::Left, None, window, cx);
                            }
                        }
                    }
                }
            }
        }
    }
}
