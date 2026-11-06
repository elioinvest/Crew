//! A terminal: a program on a pseudo-terminal, its screen kept by alacritty_terminal. Nothing here
//! knows about the UI; it asks for a [`Screen`] to draw and is told when there's something new.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};
use anyhow::{Context as _, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

pub use alacritty_terminal::term::TermMode as Mode;

/// A color as 0xRRGGBB.
pub type Color24 = u32;

/// The theme a terminal draws with.
#[derive(Clone, Debug)]
pub struct Palette {
    pub foreground: Color24,
    pub background: Color24,
    pub cursor: Color24,
    pub cursor_text: Color24,
    pub selection: Color24,
    /// black, red, green, yellow, blue, magenta, cyan, white, then the bright ones.
    pub ansi: [Color24; 16],
}

impl Palette {
    pub fn light() -> Self {
        Self {
            foreground: 0x1f1f1f,
            background: 0xffffff,
            cursor: 0x1f1f1f,
            cursor_text: 0xffffff,
            selection: 0xd9d4cc,
            ansi: LIGHT_ANSI,
        }
    }

    pub fn dark() -> Self {
        Self {
            foreground: 0xe6e3de,
            background: 0x181818,
            cursor: 0xe6e3de,
            cursor_text: 0x181818,
            selection: 0x44403b,
            ansi: DARK_ANSI,
        }
    }

    fn indexed(&self, i: u8) -> Color24 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let v = i - 16;
                let level = |n: u8| if n == 0 { 0 } else { 55 + n as u32 * 40 };
                (level(v / 36) << 16) | (level((v / 6) % 6) << 8) | level(v % 6)
            }
            _ => {
                let g = 8 + (i as u32 - 232) * 10;
                (g << 16) | (g << 8) | g
            }
        }
    }

    fn named(&self, c: NamedColor) -> Color24 {
        use NamedColor::*;
        match c {
            Foreground | BrightForeground => self.foreground,
            Background => self.background,
            Cursor => self.cursor,
            DimForeground => dim(self.foreground, self.background),
            Black => self.ansi[0],
            Red => self.ansi[1],
            Green => self.ansi[2],
            Yellow => self.ansi[3],
            Blue => self.ansi[4],
            Magenta => self.ansi[5],
            Cyan => self.ansi[6],
            White => self.ansi[7],
            BrightBlack => self.ansi[8],
            BrightRed => self.ansi[9],
            BrightGreen => self.ansi[10],
            BrightYellow => self.ansi[11],
            BrightBlue => self.ansi[12],
            BrightMagenta => self.ansi[13],
            BrightCyan => self.ansi[14],
            BrightWhite => self.ansi[15],
            DimBlack => dim(self.ansi[0], self.background),
            DimRed => dim(self.ansi[1], self.background),
            DimGreen => dim(self.ansi[2], self.background),
            DimYellow => dim(self.ansi[3], self.background),
            DimBlue => dim(self.ansi[4], self.background),
            DimMagenta => dim(self.ansi[5], self.background),
            DimCyan => dim(self.ansi[6], self.background),
            DimWhite => dim(self.ansi[7], self.background),
        }
    }
}

// The Electron app's light theme.
const LIGHT_ANSI: [Color24; 16] = [
    0x1f1f1f, 0xc4462d, 0x2f8a46, 0xa06a00, 0x2f62c4, 0x9d3fb3, 0x1a8790, 0xbdb8b0, //
    0x77736d, 0xd9573c, 0x3fa45b, 0xb98200, 0x4a7de0, 0xb55acb, 0x2aa3ad, 0x8d8984,
];
// Its dark theme keeps the terminal's standard colors.
const DARK_ANSI: [Color24; 16] = [
    0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, //
    0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];

fn dim(c: Color24, toward: Color24) -> Color24 {
    let mix = |shift: u32| {
        let a = (c >> shift) & 0xff;
        let b = (toward >> shift) & 0xff;
        ((a * 2 + b) / 3) << shift
    };
    mix(16) | mix(8) | mix(0)
}

fn rgb24(c: Rgb) -> Color24 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// What to run.
#[derive(Clone, Debug, Default)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Use only `env`, instead of adding it to this process's environment.
    pub replace_env: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// The screen changed.
    Changed,
    /// The title changed.
    Title,
    Bell,
    Exited,
}

/// One cell, resolved to colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellView {
    pub ch: char,
    pub fg: Color24,
    pub bg: Color24,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// A double-width character starts here; the next cell is its other half.
    pub wide: bool,
    /// The second half of a double-width character: nothing to draw.
    pub spacer: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Screen {
    pub rows: Vec<Vec<CellView>>,
    pub cols: usize,
    /// (column, row) in the rows above, when the cursor is on screen.
    pub cursor: Option<(usize, usize)>,
    pub cursor_shape: CursorKind,
    /// How far up the history the view is scrolled.
    pub display_offset: usize,
    pub history: usize,
    pub background: Color24,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorKind {
    #[default]
    Block,
    Bar,
    Underline,
    Hidden,
}

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

type Writer = Arc<FairMutex<Box<dyn Write + Send>>>;

pub type OutputTap = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// One color and weight of text on a line: the text, its color (None is the default), bold.
pub type TextRun = (String, Option<String>, bool);

struct Shared {
    writer: Writer,
    palette: FairMutex<Palette>,
    size: FairMutex<(u16, u16, u16, u16)>,
    title: FairMutex<Option<String>>,
    clipboard: FairMutex<Option<String>>,
    events: async_channel::Sender<TermEvent>,
    // Set while a "changed" is waiting to be picked up, so bursts of output send just one.
    changed_pending: AtomicBool,
    /// Sees what the program prints, after it's on the screen.
    tap: FairMutex<Option<OutputTap>>,
    /// While a phone shows it: the terminal at the phone's size, and the window's size for after.
    phone: FairMutex<Option<(u16, u16)>>,
    desk: FairMutex<(u16, u16, u16, u16)>,
}

impl Shared {
    fn write(&self, bytes: &[u8]) {
        let mut w = self.writer.lock();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }
    fn notify(&self, e: TermEvent) {
        let _ = self.events.try_send(e);
    }
    fn changed(&self) {
        if !self.changed_pending.swap(true, Ordering::AcqRel) {
            self.notify(TermEvent::Changed);
        }
    }
    fn tapped(&self, bytes: &[u8]) {
        let tap = self.tap.lock().clone();
        if let Some(tap) = tap {
            tap(bytes);
        }
    }
}

#[derive(Clone)]
struct Proxy(Arc<Shared>);

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.0.write(text.as_bytes()),
            Event::Title(t) => {
                *self.0.title.lock() = Some(t);
                self.0.notify(TermEvent::Title);
            }
            Event::ResetTitle => {
                *self.0.title.lock() = None;
                self.0.notify(TermEvent::Title);
            }
            Event::Bell => self.0.notify(TermEvent::Bell),
            Event::ClipboardStore(_, text) => *self.0.clipboard.lock() = Some(text),
            Event::ColorRequest(index, format) => {
                let p = self.0.palette.lock();
                let c = match index {
                    i if i == NamedColor::Foreground as usize => p.foreground,
                    i if i == NamedColor::Background as usize => p.background,
                    i if i == NamedColor::Cursor as usize => p.cursor,
                    i if i < 256 => p.indexed(i as u8),
                    _ => p.foreground,
                };
                let rgb = Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 };
                self.0.write(format(rgb).as_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let (cols, rows, w, h) = *self.0.size.lock();
                let size = WindowSize { num_lines: rows, num_cols: cols, cell_width: w, cell_height: h };
                self.0.write(format(size).as_bytes());
            }
            _ => {}
        }
    }
}

/// What a terminal that isn't on a local pseudo-terminal (an SSH shell) does when it's resized or closed.
pub struct RemoteControl {
    pub resize: Box<dyn Fn(u16, u16) + Send + Sync>,
    pub close: Box<dyn Fn() + Send + Sync>,
}

enum Backend {
    Pty { master: FairMutex<Box<dyn MasterPty + Send>>, child: FairMutex<Box<dyn portable_pty::Child + Send + Sync>> },
    Remote(RemoteControl),
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Proxy>>>,
    shared: Arc<Shared>,
    backend: Backend,
    /// Parses what [`Terminal::feed`] is given. A local terminal's reader thread keeps its own.
    parser: FairMutex<Processor>,
    last_output: AtomicI64,
    exited: Arc<AtomicBool>,
    pub events: async_channel::Receiver<TermEvent>,
}

impl Terminal {
    pub fn spawn(command: &Command, cols: u16, rows: u16, palette: Palette) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("couldn't open a pseudo-terminal")?;

        let mut cmd = CommandBuilder::new(&command.program);
        cmd.args(&command.args);
        // Without one, the pty would start in the home folder rather than where Crew is.
        match &command.cwd {
            Some(cwd) => cmd.cwd(cwd),
            None => {
                if let Ok(here) = std::env::current_dir() {
                    cmd.cwd(here);
                }
            }
        }
        if command.replace_env {
            cmd.env_clear();
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &command.env {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("couldn't start {}", command.program))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: Writer = Arc::new(FairMutex::new(pair.master.take_writer()?));
        let (tx, rx) = async_channel::bounded(64);
        let shared = Arc::new(Shared {
            writer,
            palette: FairMutex::new(palette),
            size: FairMutex::new((cols, rows, 8, 16)),
            title: FairMutex::new(None),
            clipboard: FairMutex::new(None),
            events: tx,
            changed_pending: AtomicBool::new(false),
            tap: FairMutex::new(None),
            phone: FairMutex::new(None),
            desk: FairMutex::new((cols, rows, 8, 16)),
        });
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &Size { cols: cols as usize, rows: rows as usize },
            Proxy(shared.clone()),
        )));
        let exited = Arc::new(AtomicBool::new(false));

        // Output: parse it onto the screen as it comes.
        {
            let term = term.clone();
            let shared = shared.clone();
            let exited = exited.clone();
            std::thread::Builder::new().name("crew-term-read".into()).spawn(move || {
                let mut parser: Processor = Processor::new();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            parser.advance(&mut *term.lock(), &buf[..n]);
                            shared.changed();
                            shared.tapped(&buf[..n]);
                        }
                    }
                }
                exited.store(true, Ordering::Release);
                shared.notify(TermEvent::Exited);
            })?;
        }

        Ok(Self {
            term,
            shared,
            backend: Backend::Pty { master: FairMutex::new(pair.master), child: FairMutex::new(child) },
            parser: FairMutex::new(Processor::new()),
            last_output: AtomicI64::new(0),
            exited,
            events: rx,
        })
    }

    /// A terminal whose program runs elsewhere: what it prints comes in through [`Terminal::feed`],
    /// what's typed goes to `writer`.
    pub fn remote(cols: u16, rows: u16, palette: Palette, writer: Box<dyn Write + Send>, control: RemoteControl) -> Self {
        let (tx, rx) = async_channel::bounded(64);
        let shared = Arc::new(Shared {
            writer: Arc::new(FairMutex::new(writer)),
            palette: FairMutex::new(palette),
            size: FairMutex::new((cols, rows, 8, 16)),
            title: FairMutex::new(None),
            clipboard: FairMutex::new(None),
            events: tx,
            changed_pending: AtomicBool::new(false),
            tap: FairMutex::new(None),
            phone: FairMutex::new(None),
            desk: FairMutex::new((cols, rows, 8, 16)),
        });
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &Size { cols: cols as usize, rows: rows as usize }, Proxy(shared.clone()))));
        Self {
            term,
            shared,
            backend: Backend::Remote(control),
            parser: FairMutex::new(Processor::new()),
            last_output: AtomicI64::new(0),
            exited: Arc::new(AtomicBool::new(false)),
            events: rx,
        }
    }

    /// Output from the remote program. A character split across chunks is put together by the parser.
    pub fn feed(&self, bytes: &[u8]) {
        self.last_output.store(now_ms(), Ordering::Release);
        self.parser.lock().advance(&mut *self.term.lock(), bytes);
        self.shared.changed();
        self.shared.tapped(bytes);
    }

    /// Sees what the program prints from now on, on the thread that reads it.
    pub fn set_tap(&self, tap: OutputTap) {
        *self.shared.tap.lock() = Some(tap);
    }

    /// Text of Crew's own on the screen, which isn't output from the host.
    pub fn note(&self, text: &str) {
        self.parser.lock().advance(&mut *self.term.lock(), text.as_bytes());
        self.shared.changed();
    }

    pub fn mark_exited(&self) {
        self.exited.store(true, Ordering::Release);
        self.shared.notify(TermEvent::Exited);
    }

    /// When output last arrived, in milliseconds since the epoch; 0 if none has.
    pub fn last_output(&self) -> i64 {
        self.last_output.load(Ordering::Acquire)
    }

    /// The row the cursor is on, counted from the top of the scrollback.
    pub fn cursor_row(&self) -> usize {
        let t = self.term.lock();
        (t.grid().history_size() as i32 + t.grid().cursor.point.line.0).max(0) as usize
    }

    /// The first row on screen, counted from the top of the scrollback.
    pub fn screen_top(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    /// Lines from a row counted from the top of the scrollback, soft-wrapped lines joined.
    pub fn lines_from(&self, from: usize) -> Vec<String> {
        let t = self.term.lock();
        let grid = t.grid();
        let history = grid.history_size() as i32;
        let cols = grid.columns();
        let mut lines: Vec<String> = Vec::new();
        let mut joined = false;
        for y in (from as i32 - history).max(-history)..grid.screen_lines() as i32 {
            let row = &grid[Line(y)];
            let mut s = String::with_capacity(cols);
            for x in 0..cols {
                let c = &row[Column(x)];
                if !c.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    s.push(c.c);
                }
            }
            let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            if !wraps {
                s.truncate(s.trim_end().len());
            }
            match (joined, lines.last_mut()) {
                (true, Some(last)) => last.push_str(&s),
                _ => lines.push(s),
            }
            joined = wraps;
        }
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        lines
    }

    /// The text on the cursor's line.
    pub fn cursor_line(&self) -> String {
        let t = self.term.lock();
        let grid = t.grid();
        let row = &grid[grid.cursor.point.line];
        let chars: Vec<char> = (0..grid.columns()).map(|x| row[Column(x)].c).collect();
        // Up to the cursor at least: the space after a prompt counts.
        let used = chars.iter().rposition(|c| *c != ' ').map_or(0, |i| i + 1).max(grid.cursor.point.column.0);
        chars[..used.min(chars.len())].iter().collect()
    }

    /// A full-screen program (an editor, a pager, a menu) is showing.
    pub fn alt_screen(&self) -> bool {
        self.term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

    /// Call when a [`TermEvent::Changed`] is handled, so the next change sends another.
    pub fn take_changed(&self) {
        self.shared.changed_pending.store(false, Ordering::Release);
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub fn title(&self) -> Option<String> {
        self.shared.title.lock().clone()
    }

    /// Text a program asked to put on the clipboard (OSC 52), once.
    pub fn take_clipboard(&self) -> Option<String> {
        self.shared.clipboard.lock().take()
    }

    pub fn write(&self, bytes: &[u8]) {
        self.shared.write(bytes);
    }

    /// The window's size for it. While a phone shows the terminal the phone's size stays, and this
    /// is what it goes back to.
    pub fn resize(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        *self.shared.desk.lock() = (cols, rows, cell_width, cell_height);
        if self.shared.phone.lock().is_some() {
            return;
        }
        self.apply_size(cols, rows, cell_width, cell_height);
    }

    /// While a phone shows the terminal it takes the phone's size, so the program lays its screen out
    /// for the phone; None goes back to the window's.
    pub fn set_phone_size(&self, size: Option<(u16, u16)>) {
        *self.shared.phone.lock() = size;
        let (cols, rows, w, h) = *self.shared.desk.lock();
        match size {
            Some((c, r)) => self.apply_size(c, r, w, h),
            None => self.apply_size(cols, rows, w, h),
        }
    }

    fn apply_size(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        {
            let mut s = self.shared.size.lock();
            if s.0 == cols && s.1 == rows {
                return;
            }
            *s = (cols, rows, cell_width, cell_height);
        }
        match &self.backend {
            Backend::Pty { master, .. } => {
                let _ = master.lock().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            }
            Backend::Remote(control) => (control.resize)(cols, rows),
        }
        self.term.lock().resize(Size { cols: cols as usize, rows: rows as usize });
    }

    pub fn size(&self) -> (u16, u16) {
        let s = self.shared.size.lock();
        (s.0, s.1)
    }

    pub fn kill(&self) {
        match &self.backend {
            Backend::Pty { child, .. } => {
                let _ = child.lock().kill();
            }
            Backend::Remote(control) => (control.close)(),
        }
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.shared.changed();
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.shared.changed();
    }

    pub fn start_selection(&self, col: usize, row: usize, right_half: bool, word: bool) {
        let mut t = self.term.lock();
        let p = view_point(&t, col, row);
        let side = if right_half { Side::Right } else { Side::Left };
        let kind = if word { SelectionType::Semantic } else { SelectionType::Simple };
        t.selection = Some(Selection::new(kind, p, side));
        drop(t);
        self.shared.changed();
    }

    pub fn update_selection(&self, col: usize, row: usize, right_half: bool) {
        let mut t = self.term.lock();
        let p = view_point(&t, col, row);
        if let Some(s) = t.selection.as_mut() {
            s.update(p, if right_half { Side::Right } else { Side::Left });
        }
        drop(t);
        self.shared.changed();
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
        self.shared.changed();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// The visible lines as plain text, for reading what an agent shows (busy, model, ...).
    pub fn screen_text(&self) -> Vec<String> {
        let t = self.term.lock();
        let grid = t.grid();
        let cols = grid.columns();
        (0..grid.screen_lines() as i32)
            .map(|y| {
                let row = &grid[Line(y)];
                let mut s = String::with_capacity(cols);
                for x in 0..cols {
                    let c = &row[Column(x)];
                    if !c.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        s.push(c.c);
                        s.extend(c.zerowidth().unwrap_or(&[]));
                    }
                }
                s.truncate(s.trim_end().len());
                s
            })
            .collect()
    }

    /// The whole line around a row on screen, rows joined where they wrapped, and where each of its
    /// characters is: (row on screen, which can be off it; column; cells wide).
    pub fn logical_line(&self, row: usize) -> (String, Vec<(i32, usize, usize)>) {
        let t = self.term.lock();
        let grid = t.grid();
        let cols = grid.columns();
        let offset = grid.display_offset() as i32;
        let (history, screen) = (grid.history_size() as i32, grid.screen_lines() as i32);
        let wraps = |y: i32| grid[Line(y)][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let mut y = (row as i32 - offset).min(screen - 1);
        while y > -history && wraps(y - 1) {
            y -= 1;
        }
        let (mut text, mut places) = (String::new(), Vec::new());
        loop {
            for x in 0..cols {
                let c = &grid[Line(y)][Column(x)];
                if c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let place = (y + offset, x, if c.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 });
                text.push(c.c);
                places.push(place);
                for z in c.zerowidth().unwrap_or(&[]) {
                    text.push(*z);
                    places.push(place);
                }
            }
            if y + 1 >= screen || !wraps(y) {
                return (text, places);
            }
            y += 1;
        }
    }

    /// The link a program printed on a cell (OSC 8): its address, and the columns it covers on that row.
    pub fn hyperlink_at(&self, col: usize, row: usize) -> Option<(String, usize, usize)> {
        let t = self.term.lock();
        let grid = t.grid();
        let cols = grid.columns();
        let line = Line(row as i32 - grid.display_offset() as i32);
        if col >= cols || line.0 >= grid.screen_lines() as i32 {
            return None;
        }
        let cells = &grid[line];
        let link = cells[Column(col)].hyperlink()?;
        let same = |x: usize| cells[Column(x)].hyperlink().as_ref() == Some(&link);
        let mut start = col;
        while start > 0 && same(start - 1) {
            start -= 1;
        }
        let mut end = col + 1;
        while end < cols && same(end) {
            end += 1;
        }
        Some((link.uri().to_string(), start, end))
    }

    /// The screen and the history just above it, line for line; the last lines are the screen.
    pub fn raw_lines(&self, history: usize) -> Vec<String> {
        let t = self.term.lock();
        let grid = t.grid();
        let cols = grid.columns();
        let first = -(grid.history_size().min(history) as i32);
        (first..grid.screen_lines() as i32)
            .map(|y| {
                let row = &grid[Line(y)];
                let mut s = String::with_capacity(cols);
                for x in 0..cols {
                    let c = &row[Column(x)];
                    if !c.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        s.push(c.c);
                        s.extend(c.zerowidth().unwrap_or(&[]));
                    }
                }
                s.truncate(s.trim_end().len());
                s
            })
            .collect()
    }

    /// The screen and history as lines of colored text, and the cursor: its column, and its line
    /// within those lines.
    pub fn text_runs(&self, history: usize) -> (Vec<Vec<TextRun>>, (usize, usize)) {
        let t = self.term.lock();
        let grid = t.grid();
        let cols = grid.columns();
        let back = grid.history_size().min(history) as i32;
        let lines = (-back..grid.screen_lines() as i32)
            .map(|y| {
                let row = &grid[Line(y)];
                let mut runs: Vec<TextRun> = Vec::new();
                for x in 0..cols {
                    let c = &row[Column(x)];
                    if c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                        continue;
                    }
                    let (color, bold) = (phone_color(c.fg), c.flags.contains(Flags::BOLD));
                    if !runs.last().is_some_and(|last| last.1 == color && last.2 == bold) {
                        runs.push((String::new(), color, bold));
                    }
                    if let Some(last) = runs.last_mut() {
                        last.0.push(c.c);
                        last.0.extend(c.zerowidth().unwrap_or(&[]));
                    }
                }
                // Trailing blanks carry nothing.
                while runs.last().is_some_and(|r| r.0.trim().is_empty()) {
                    runs.pop();
                }
                if let Some(last) = runs.last_mut() {
                    last.0.truncate(last.0.trim_end().len());
                }
                runs
            })
            .collect();
        let cursor = grid.cursor.point;
        (lines, (cursor.column.0, (back + cursor.line.0).max(0) as usize))
    }

    /// The colours for what's drawn from now on, when the theme changes.
    pub fn set_palette(&self, palette: Palette) {
        *self.shared.palette.lock() = palette;
    }

    pub fn screen(&self) -> Screen {
        let t = self.term.lock();
        let palette = self.shared.palette.lock().clone();
        let rows_n = t.screen_lines();
        let cols = t.columns();
        let mode = *t.mode();
        let history = t.grid().history_size();
        let content = t.renderable_content();
        let colors = content.colors;
        let resolve = |c: Color| -> Color24 {
            match c {
                Color::Spec(rgb) => rgb24(rgb),
                Color::Named(n) => colors[n].map(rgb24).unwrap_or_else(|| palette.named(n)),
                Color::Indexed(i) => colors[i as usize].map(rgb24).unwrap_or_else(|| palette.indexed(i)),
            }
        };
        let offset = content.display_offset as i32;
        let selection = content.selection;
        let background = palette.background;
        let blank = CellView {
            ch: ' ',
            fg: palette.foreground,
            bg: background,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            wide: false,
            spacer: false,
            selected: false,
        };
        let mut rows = vec![vec![blank; cols]; rows_n];
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        for indexed in content.display_iter {
            let y = indexed.point.line.0 + offset;
            let x = indexed.point.column.0;
            if y < 0 || y as usize >= rows_n || x >= cols {
                continue;
            }
            let cell = indexed.cell;
            let mut fg = resolve(cell.fg);
            let mut bg = resolve(cell.bg);
            let f = cell.flags;
            if f.contains(Flags::BOLD) {
                if let Color::Named(n) = cell.fg {
                    if (n as usize) < 8 {
                        fg = palette.ansi[n as usize + 8];
                    }
                }
            }
            if f.contains(Flags::DIM) {
                fg = dim(fg, background);
            }
            if f.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if f.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let selected = selection.is_some_and(|s| s.contains(indexed.point));
            if selected {
                bg = palette.selection;
            }
            rows[y as usize][x] = CellView {
                ch: if cell.c == '\0' { ' ' } else { cell.c },
                fg,
                bg,
                bold: f.contains(Flags::BOLD),
                italic: f.contains(Flags::ITALIC),
                underline: f.intersects(Flags::ALL_UNDERLINES),
                strike: f.contains(Flags::STRIKEOUT),
                wide: f.contains(Flags::WIDE_CHAR),
                spacer: f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                selected,
            };
        }
        let y = cursor_point.line.0 + offset;
        let cursor = (mode.contains(TermMode::SHOW_CURSOR) && y >= 0 && (y as usize) < rows_n)
            .then_some((cursor_point.column.0, y as usize));
        Screen {
            rows,
            cols,
            cursor,
            cursor_shape: match cursor_shape {
                CursorShape::Block | CursorShape::HollowBlock => CursorKind::Block,
                CursorShape::Beam => CursorKind::Bar,
                CursorShape::Underline => CursorKind::Underline,
                CursorShape::Hidden => CursorKind::Hidden,
            },
            display_offset: offset as usize,
            history,
            background,
        }
    }
}

/// The colors a phone shows, the same on any theme of the window's.
fn phone_color(c: Color) -> Option<String> {
    const BASE16: [&str; 16] = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5", //
        "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
    ];
    let indexed = |i: u8| match i {
        0..=15 => BASE16[i as usize].to_string(),
        16..=231 => {
            let n = i - 16;
            let v = |k: u8| if k == 0 { 0 } else { 55 + k as u32 * 40 };
            format!("#{:02x}{:02x}{:02x}", v(n / 36), v((n / 6) % 6), v(n % 6))
        }
        _ => {
            let g = 8 + (i as u32 - 232) * 10;
            format!("#{g:02x}{g:02x}{g:02x}")
        }
    };
    match c {
        Color::Spec(rgb) => Some(format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)),
        Color::Indexed(i) => Some(indexed(i)),
        Color::Named(n) => {
            let i = n as usize;
            (i < 16).then(|| BASE16[i].to_string())
        }
    }
}

fn view_point(t: &Term<Proxy>, col: usize, row: usize) -> Point {
    let offset = t.grid().display_offset() as i32;
    Point::new(Line(row as i32 - offset), Column(col.min(t.columns().saturating_sub(1))))
}

impl Terminal {
    /// Ends the program and everything it started, for when Crew quits: killing only the program can
    /// leave its children running with no window to reach them.
    pub fn kill_tree(&self) {
        #[cfg(windows)]
        if let Backend::Pty { child, .. } = &self.backend {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            if let Some(pid) = child.lock().process_id() {
                let _ = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).creation_flags(CREATE_NO_WINDOW).output();
            }
        }
        self.kill();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(t: &Terminal, what: &str) -> bool {
        for _ in 0..200 {
            t.take_changed();
            if t.screen_text().iter().any(|l| l.contains(what)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    fn shell(line: &str) -> Command {
        Command {
            program: "cmd.exe".into(),
            args: vec!["/d".into(), "/s".into(), "/c".into(), line.into()],
            ..Default::default()
        }
    }

    fn interactive() -> Command {
        Command { program: "cmd.exe".into(), ..Default::default() }
    }

    #[test]
    fn runs_a_program_and_reads_its_screen() {
        let t = Terminal::spawn(&shell("echo hello-from-pty"), 80, 24, Palette::light()).unwrap();
        assert!(wait_for(&t, "hello-from-pty"), "{:?}", t.screen_text());
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let t = Terminal::spawn(&interactive(), 80, 24, Palette::light()).unwrap();
        assert!(wait_for(&t, ">"));
        t.write("chcp 65001 >nul & echo 你好 done\r\n".as_bytes());
        assert!(wait_for(&t, "done"), "{:?}", t.screen_text());
        for _ in 0..40 {
            let s = t.screen();
            if let Some(row) = s.rows.iter().rev().find(|r| r.iter().any(|c| c.ch == '你')) {
                let i = row.iter().position(|c| c.ch == '你').unwrap();
                assert!(row[i].wide, "wide flag");
                assert!(row[i + 1].spacer, "spacer follows");
                assert_eq!(row[i + 2].ch, '好');
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("no wide char on screen: {:?}", t.screen_text());
    }

    #[test]
    fn resize_changes_the_grid() {
        let t = Terminal::spawn(&interactive(), 80, 24, Palette::dark()).unwrap();
        t.resize(100, 30, 8, 16);
        let s = t.screen();
        assert_eq!(s.cols, 100);
        assert_eq!(s.rows.len(), 30);
        assert_eq!(s.background, 0x181818);
    }

    #[test]
    fn a_phone_takes_over_the_size_until_it_leaves() {
        let t = Terminal::spawn(&interactive(), 80, 24, Palette::light()).unwrap();
        t.resize(100, 30, 8, 16);
        t.set_phone_size(Some((40, 20)));
        assert_eq!(t.size(), (40, 20));
        // The window resizing meanwhile doesn't take it back, and is remembered.
        t.resize(110, 32, 8, 16);
        assert_eq!(t.size(), (40, 20));
        t.set_phone_size(None);
        assert_eq!(t.size(), (110, 32));
    }

    #[test]
    fn the_tap_sees_output_and_runs_carry_color() {
        let seen = Arc::new(FairMutex::new(Vec::new()));
        let t = Terminal::remote(40, 5, Palette::light(), Box::new(std::io::sink()), RemoteControl { resize: Box::new(|_, _| {}), close: Box::new(|| {}) });
        let store = seen.clone();
        t.set_tap(Arc::new(move |b| store.lock().extend_from_slice(b)));
        t.feed(b"plain \x1b[31;1mred\x1b[0m \x1b[38;5;196mx\x1b[38;2;1;2;3my\x1b[0m\r\nnext");
        assert!(seen.lock().starts_with(b"plain"));
        let (lines, cursor) = t.text_runs(10);
        assert_eq!(lines[0][0], ("plain ".to_string(), None, false));
        assert_eq!(lines[0][1], ("red".to_string(), Some("#cd3131".to_string()), true));
        assert_eq!(lines[0][3], ("x".to_string(), Some("#ff0000".to_string()), false));
        assert_eq!(lines[0][4], ("y".to_string(), Some("#010203".to_string()), false));
        assert_eq!(cursor, (4, 1));
        assert_eq!(t.raw_lines(10)[1], "next");
        assert_eq!(t.raw_lines(10).len(), 5);
    }

    #[test]
    fn finds_lines_and_printed_links() {
        let t = Terminal::remote(10, 5, Palette::light(), Box::new(std::io::sink()), RemoteControl { resize: Box::new(|_, _| {}), close: Box::new(|| {}) });
        t.feed("好abcdefghijklm\r\n\x1b]8;;https://a.com\x1b\\link\x1b]8;;\x1b\\ x".as_bytes());
        let (text, places) = t.logical_line(1);
        assert_eq!(text.trim_end(), "好abcdefghijklm");
        assert_eq!(places[0], (0, 0, 2));
        assert_eq!(places[1], (0, 2, 1));
        assert_eq!(places[9], (1, 0, 1), "the wrapped part is on the next row");
        assert_eq!(t.logical_line(0).0, text);
        assert_eq!(t.hyperlink_at(1, 2), Some(("https://a.com".to_string(), 0, 4)));
        assert_eq!(t.hyperlink_at(5, 2), None);
    }

    #[test]
    fn selection_returns_text() {
        let t = Terminal::spawn(&shell("echo select-me-now"), 80, 24, Palette::light()).unwrap();
        assert!(wait_for(&t, "select-me-now"));
        let lines = t.screen_text();
        let y = lines.iter().position(|l| l.contains("select-me-now")).unwrap();
        let x = lines[y].find("select").unwrap();
        t.start_selection(x, y, false, false);
        t.update_selection(x + 8, y, true);
        assert_eq!(t.selected_text().as_deref(), Some("select-me"));
    }
}
