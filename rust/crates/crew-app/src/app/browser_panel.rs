//! The side browser. GPUI has no web view, so each tab is a WebView2 child window laid over the space
//! the panel keeps for it. Being a separate window it draws above everything of ours, so it's hidden
//! while a dialog or a menu would land on it. Background tabs stay loaded; a tab restored from last
//! time loads the first time it's shown, and all tabs share one profile so sign-ins carry across.

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{Receiver, Sender, channel};

use crew_core::browser::{HOME_URL, host_of, normalize_url};
use gpui::{Focusable, Image, ImageFormat, Subscription, Task, canvas, img};
use webview2_com::AcceleratorKeyPressedEventHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder, WebViewExtWindows};

use super::*;

/// The window the web views are children of, kept apart so they can be made while nothing of the
/// app is borrowed: WebView2 pumps the window's messages while it starts, and those run our paint.
#[derive(Clone, Copy)]
struct Parent(RawWindowHandle);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

/// What pages report; their callbacks run outside the app, so it's collected on a timer.
enum Event {
    Title(u64, String),
    Load { id: u64, done: bool, url: String },
    Icon(u64, String),
    NewTab(String),
    Key { key: &'static str, shift: bool },
}

struct Tab {
    id: u64,
    url: String,
    title: String,
    favicon: Option<Arc<Image>>,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    /// Not made until the tab is first shown.
    view: Option<WebView>,
}

pub(super) struct BrowserPanel {
    tabs: Vec<Tab>,
    active: Option<u64>,
    next_id: u64,
    context: Rc<RefCell<WebContext>>,
    parent: Parent,
    creating: bool,
    error: Option<String>,
    input: Entity<InputState>,
    /// Don't overwrite what's being typed in the address bar.
    editing: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    body: Rc<Cell<Option<Bounds<Pixels>>>>,
    applied: Option<(Bounds<Pixels>, bool, Option<u64>)>,
    /// As last drawn: what's beside it needs this to size itself.
    shown_width: Cell<f32>,
    /// Whether a page is showing through the hole; otherwise the hole is painted over.
    page_shown: bool,
    width: f32,
    _input_events: Subscription,
    _input_focus: Subscription,
    _pump: Task<()>,
}

fn build_view(parent: Parent, context: &RefCell<WebContext>, tx: Sender<Event>, id: u64, url: &str, bounds: Rect) -> Result<WebView, String> {
    let (new_tab, title, load, keys) = (tx.clone(), tx.clone(), tx.clone(), tx);
    let mut context = context.borrow_mut();
    let view = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(url)
        .with_bounds(bounds)
        .with_clipboard(true)
        // Pop-ups and target=_blank links open as new tabs.
        .with_new_window_req_handler(move |url, _| {
            let lower = url.to_lowercase();
            if lower.starts_with("http:") || lower.starts_with("https:") {
                let _ = new_tab.send(Event::NewTab(url));
            }
            NewWindowResponse::Deny
        })
        .with_document_title_changed_handler(move |text| {
            let _ = title.send(Event::Title(id, text));
        })
        .with_on_page_load_handler(move |event, url| {
            let _ = load.send(Event::Load { id, done: matches!(event, PageLoadEvent::Finished), url });
        })
        .build_as_child(&parent)
        .map_err(|e| e.to_string())?;
    pass_tab_shortcuts(&view, keys);
    Ok(view)
}

/// Pages get keyboard input first, so pass the tab shortcuts up to the panel.
fn pass_tab_shortcuts(view: &WebView, tx: Sender<Event>) {
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
        let mut code = 0u32;
        unsafe {
            args.KeyEventKind(&mut kind)?;
            args.VirtualKey(&mut code)?;
        }
        let down = |key: i32| unsafe { GetKeyState(key) } < 0;
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN || !down(i32::from(VK_CONTROL.0)) || down(i32::from(VK_MENU.0)) {
            return Ok(());
        }
        let key = match code {
            0x54 => "t",
            0x57 => "w",
            0x4C => "l",
            0x09 => "tab",
            _ => return Ok(()),
        };
        let _ = tx.send(Event::Key { key, shift: down(i32::from(VK_SHIFT.0)) });
        unsafe { args.SetHandled(true)? };
        Ok(())
    }));
    let mut token = 0;
    let _ = unsafe { view.controller().add_AcceleratorKeyPressed(&handler, &mut token) };
}

fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(ImageFormat::Webp),
        [0, 0, 1, 0, ..] => Some(ImageFormat::Ico),
        _ => {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).to_lowercase();
            head.contains("<svg").then_some(ImageFormat::Svg)
        }
    }
}

fn fetch_icon(url: &str) -> Option<Arc<Image>> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(8))).build().into();
    let bytes = agent.get(url).call().ok()?.body_mut().with_config().limit(1 << 20).read_to_vec().ok()?;
    Some(Arc::new(Image::from_bytes(image_format(&bytes)?, bytes)))
}

impl CrewApp {
    fn browser_bounds_rect(bounds: Bounds<Pixels>, scale: f32) -> Rect {
        let at = |v: Pixels| (f32::from(v) * scale).round();
        Rect {
            position: PhysicalPosition::new(at(bounds.origin.x) as i32, at(bounds.origin.y) as i32).into(),
            size: PhysicalSize::new(at(bounds.size.width).max(1.) as u32, at(bounds.size.height).max(1.) as u32).into(),
        }
    }

    fn saved_browser_tabs(&self) -> (Vec<String>, usize) {
        let saved = self.config.raw().get("browserTabs");
        let urls: Vec<String> = saved.and_then(|v| v.get("urls")).and_then(|v| v.as_array()).map_or(Vec::new(), |list| list.iter().filter_map(|u| u.as_str().map(String::from)).collect());
        if urls.is_empty() {
            return (vec![HOME_URL.to_string()], 0);
        }
        let active = saved.and_then(|v| v.get("active")).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        (urls, active)
    }

    fn save_browser_tabs(&mut self) {
        let Some(b) = &self.browser else { return };
        let urls: Vec<&str> = b.tabs.iter().map(|t| t.url.as_str()).collect();
        let active = b.tabs.iter().position(|t| Some(t.id) == b.active).unwrap_or(0);
        let value = json!({ "urls": urls, "active": active });
        self.save_setting("browserTabs", value);
    }

    /// Shows the panel, restoring the last tabs; with an address, opens it in a new tab.
    pub(super) fn open_browser(&mut self, url: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.is_none() {
            let Ok(handle) = HasWindowHandle::window_handle(&*window) else { return };
            let parent = Parent(handle.as_raw());
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Enter a URL or search")));
            let events = cx.subscribe_in(&input, window, |app, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let typed = input.read(cx).value().to_string();
                    if let Some(b) = app.browser.as_mut() {
                        let shown = b.tabs.iter().find(|t| Some(t.id) == b.active).map_or("", |t| t.url.as_str());
                        b.editing = typed != shown;
                    }
                }
                InputEvent::PressEnter { .. } => app.browser_go(window, cx),
                _ => {}
            });
            // Clicking the address selects it, so typing replaces it.
            let focus_handle = input.read(cx).focus_handle(cx);
            let input_focus = cx.on_focus(&focus_handle, window, |_, window, cx| window.dispatch_action(Box::new(gpui_component::input::SelectAll), cx));
            let pump = cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    if this.update_in(cx, |app, window, cx| app.browser_pump(window, cx)).is_err() {
                        break;
                    }
                }
            });
            let (tx, rx) = channel();
            let data = self.paths.data_dir.join("browser-webview2");
            let width = self.config.f32("browserWidth", 0.);
            self.browser = Some(BrowserPanel {
                tabs: Vec::new(),
                active: None,
                next_id: 1,
                context: Rc::new(RefCell::new(WebContext::new(Some(data)))),
                parent,
                creating: false,
                error: None,
                input,
                editing: false,
                tx,
                rx,
                body: Rc::new(Cell::new(None)),
                applied: None,
                shown_width: Cell::new(0.),
                page_shown: false,
                width,
                _input_events: events,
                _input_focus: input_focus,
                _pump: pump,
            });
            self.save_setting("browserOpen", json!(true));
            let (urls, active) = self.saved_browser_tabs();
            let first = active.min(urls.len() - 1);
            for u in &urls {
                self.browser_add_tab(u.clone());
            }
            if let Some(b) = self.browser.as_mut() {
                b.active = b.tabs.get(first).map(|t| t.id);
            }
            self.browser_show_url(window, cx);
        }
        if let Some(url) = url {
            self.browser_new_tab(url, window, cx);
        }
        cx.notify();
    }

    fn browser_add_tab(&mut self, url: String) -> Option<u64> {
        let b = self.browser.as_mut()?;
        let id = b.next_id;
        b.next_id += 1;
        b.tabs.push(Tab { id, url, title: String::new(), favicon: None, loading: false, can_back: false, can_forward: false, view: None });
        Some(id)
    }

    pub(super) fn browser_new_tab(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.browser_add_tab(url) {
            self.browser_switch(id, window, cx);
        }
    }

    /// Closing the panel unloads every tab but remembers them for the next open.
    pub(super) fn close_browser(&mut self, cx: &mut Context<Self>) {
        self.save_browser_tabs();
        self.browser = None;
        self.save_setting("browserOpen", json!(false));
        cx.notify();
    }

    fn browser_switch(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.browser.as_mut() else { return };
        b.active = Some(id);
        b.editing = false;
        self.browser_show_url(window, cx);
        self.save_browser_tabs();
        cx.notify();
    }

    fn browser_close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.browser.as_mut() else { return };
        let Some(i) = b.tabs.iter().position(|t| t.id == id) else { return };
        b.tabs.remove(i);
        if b.tabs.is_empty() {
            // The tabs a next open restores: none left, so it starts from the home page.
            self.save_setting("browserTabs", json!({ "urls": [], "active": 0 }));
            self.browser = None;
            self.save_setting("browserOpen", json!(false));
            cx.notify();
            return;
        }
        if b.active == Some(id) {
            let next = b.tabs[i.min(b.tabs.len() - 1)].id;
            self.browser_switch(next, window, cx);
        } else {
            self.save_browser_tabs();
            cx.notify();
        }
    }

    fn browser_show_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.browser.as_ref() else { return };
        if b.editing {
            return;
        }
        let url = b.tabs.iter().find(|t| Some(t.id) == b.active).map_or(String::new(), |t| t.url.clone());
        b.input.clone().update(cx, |s, cx| {
            if s.value() != url.as_str() {
                s.set_value(url, window, cx);
            }
        });
        if let Some(b) = self.browser.as_mut() {
            b.editing = false;
        }
    }

    pub(super) fn browser_shown_width(&self) -> f32 {
        self.browser.as_ref().map_or(0., |b| b.shown_width.get())
    }

    pub(super) fn set_browser_width(&mut self, width: f32) {
        if let Some(b) = self.browser.as_mut() {
            b.width = width;
        }
    }

    pub(super) fn save_browser_width(&mut self) {
        if let Some(width) = self.browser.as_ref().map(|b| b.width) {
            self.save_setting("browserWidth", json!(width));
        }
    }

    fn browser_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.browser.as_mut() else { return };
        let typed = b.input.read(cx).value().to_string();
        let Some(url) = normalize_url(&typed) else { return };
        b.editing = false;
        match b.tabs.iter_mut().find(|t| Some(t.id) == b.active) {
            Some(tab) => {
                tab.url = url;
                if let Some(view) = &tab.view {
                    let _ = view.load_url(&tab.url);
                    let _ = view.focus();
                }
                window.focus(&self.root_focus, cx);
            }
            None => {
                self.browser_new_tab(url, window, cx);
                return;
            }
        }
        self.browser_show_url(window, cx);
        self.save_browser_tabs();
        cx.notify();
    }

    fn browser_focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.browser.as_ref() else { return };
        if let Some(view) = b.tabs.iter().find(|t| Some(t.id) == b.active).and_then(|t| t.view.as_ref()) {
            let _ = view.focus_parent();
        }
        let input = b.input.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Ctrl+T / Ctrl+W / Ctrl+Tab / Ctrl+L while the browser has the keyboard.
    fn browser_shortcut(&mut self, key: &str, shift: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.browser.as_ref() else { return false };
        match key {
            "t" => {
                self.browser_new_tab(HOME_URL.to_string(), window, cx);
                self.browser_focus_address(window, cx);
            }
            "w" => {
                if let Some(id) = b.active {
                    self.browser_close_tab(id, window, cx);
                }
            }
            "tab" => {
                if b.tabs.len() < 2 {
                    return false;
                }
                let n = b.tabs.len();
                let i = b.tabs.iter().position(|t| Some(t.id) == b.active).unwrap_or(0);
                let next = b.tabs[(i + if shift { n - 1 } else { 1 }) % n].id;
                self.browser_switch(next, window, cx);
            }
            "l" => self.browser_focus_address(window, cx),
            _ => return false,
        }
        true
    }

    /// Only from the address bar: in the terminal or composer these keys belong to the agent.
    pub(super) fn browser_key(&mut self, key: &str, shift: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focused = self.browser.as_ref().is_some_and(|b| b.input.read(cx).focus_handle(cx).is_focused(window));
        focused && self.browser_shortcut(key, shift, window, cx)
    }

    /// Esc in the address bar puts the page's address back and hands the keyboard to the page.
    pub(super) fn browser_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.browser.as_mut() else { return false };
        if !b.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        b.editing = false;
        if let Some(view) = b.tabs.iter().find(|t| Some(t.id) == b.active).and_then(|t| t.view.as_ref()) {
            let _ = view.focus();
        }
        window.focus(&self.root_focus, cx);
        self.browser_show_url(window, cx);
        true
    }

    /// A click on the app takes the keyboard back from the page.
    pub(super) fn browser_release_keyboard(&self) {
        if let Some(b) = &self.browser {
            if let Some(view) = b.tabs.iter().find(|t| Some(t.id) == b.active).and_then(|t| t.view.as_ref()) {
                let _ = view.focus_parent();
            }
        }
    }

    fn browser_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.browser.as_mut() else { return };
        let events: Vec<Event> = b.rx.try_iter().collect();
        let mut changed = !events.is_empty();
        let mut save = false;
        for event in events {
            let Some(b) = self.browser.as_mut() else { return };
            match event {
                Event::NewTab(url) => self.browser_new_tab(url, window, cx),
                Event::Key { key, shift } => {
                    self.browser_shortcut(key, shift, window, cx);
                }
                Event::Title(id, title) => {
                    if let Some(tab) = b.tabs.iter_mut().find(|t| t.id == id) {
                        tab.title = title;
                    }
                }
                Event::Load { id, done, url } => {
                    let tx = b.tx.clone();
                    if let Some(tab) = b.tabs.iter_mut().find(|t| t.id == id) {
                        tab.loading = !done;
                        if !url.is_empty() && tab.url != url {
                            tab.url = url;
                            save = true;
                        }
                        if !done {
                            tab.favicon = None;
                        } else if let Some(view) = &tab.view {
                            let script = "(document.querySelector(\"link[rel~='icon']\")||{}).href||(location.origin+'/favicon.ico')";
                            let _ = view.evaluate_script_with_callback(script, move |json| {
                                if let Ok(icon) = serde_json::from_str::<String>(&json) {
                                    let _ = tx.send(Event::Icon(id, icon));
                                }
                            });
                        }
                    }
                }
                Event::Icon(id, url) => {
                    if url.to_lowercase().starts_with("http") {
                        let work = cx.background_spawn(async move { fetch_icon(&url) });
                        cx.spawn(async move |this, cx| {
                            let icon = work.await;
                            let _ = this.update(cx, |app, cx| {
                                if let Some(tab) = app.browser.as_mut().and_then(|b| b.tabs.iter_mut().find(|t| t.id == id)) {
                                    tab.favicon = icon;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                }
            }
        }
        // A click on the page moves the keyboard without GPUI knowing, and the address bar has to
        // lose focus to select its text on the next click.
        let Some(b) = self.browser.as_mut() else { return };
        let RawWindowHandle::Win32(own) = b.parent.0 else { return };
        let holder = unsafe { GetFocus() };
        if !holder.is_invalid() && holder.0 as isize != own.hwnd.get() && b.input.read(cx).focus_handle(cx).is_focused(window) {
            b.editing = false;
            window.focus(&self.root_focus, cx);
            self.browser_show_url(window, cx);
        }
        // Pages also change address without loading (single-page sites), and the history moves on.
        let Some(b) = self.browser.as_mut() else { return };
        let active = b.active;
        if let Some(tab) = b.tabs.iter_mut().find(|t| Some(t.id) == active) {
            if let Some(view) = &tab.view {
                if let Ok(url) = view.url() {
                    if !url.is_empty() && url != tab.url {
                        tab.url = url;
                        save = true;
                    }
                }
                let (back, forward) = (view.can_go_back().unwrap_or(false), view.can_go_forward().unwrap_or(false));
                if (back, forward) != (tab.can_back, tab.can_forward) {
                    (tab.can_back, tab.can_forward) = (back, forward);
                    changed = true;
                }
            }
        }
        if save {
            changed = true;
            self.browser_show_url(window, cx);
            self.save_browser_tabs();
        }
        if changed {
            cx.notify();
        }
    }

    /// Whether something of ours would land under a page.
    fn browser_covered(&self, body: Bounds<Pixels>) -> bool {
        let over = |x: Pixels, width: f32| f32::from(x) + width > f32::from(body.origin.x);
        self.menu.as_ref().is_some_and(|m| over(m.at.x, 320.))
            || self.picker.as_ref().is_some_and(|p| over(p.at.x, 520.))
            || self.dialog.is_some()
            || self.form.is_some()
            || self.prompt_shown.is_some()
            || self.settings.is_some()
            || self.usage_page.is_some()
            || self.host_editor.is_some()
            || self.provider_editor.is_some()
            || self.price_editor.is_some()
            || self.agent_editor.is_some()
            || self.run_result.is_some()
            || self.pending_after_save.is_some()
            || !self.approvals.is_empty()
    }

    /// After each frame: the web view follows the space kept for it, and the shown tab gets made.
    pub(super) fn browser_sync(&mut self, cx: &mut Context<Self>, scale: f32) {
        let Some(b) = self.browser.as_ref() else { return };
        let Some(body) = b.body.get() else { return };
        let visible = !self.browser_covered(body);
        let Some(b) = self.browser.as_mut() else { return };
        let state = (body, visible, b.active);
        if b.applied != Some(state) {
            for tab in &b.tabs {
                if let Some(view) = &tab.view {
                    let show = visible && Some(tab.id) == b.active;
                    if show {
                        let _ = view.set_bounds(Self::browser_bounds_rect(body, scale));
                    }
                    let _ = view.set_visible(show);
                }
            }
            b.applied = Some(state);
        }
        let shown = visible && b.tabs.iter().any(|t| Some(t.id) == b.active && t.view.is_some());
        if shown != b.page_shown {
            b.page_shown = shown;
            cx.notify();
        }
        if b.creating || b.error.is_some() {
            return;
        }
        let Some(tab) = b.tabs.iter_mut().find(|t| Some(t.id) == b.active && t.view.is_none()) else { return };
        tab.loading = true;
        b.creating = true;
        let (id, url, parent, context, tx) = (tab.id, tab.url.clone(), b.parent, b.context.clone(), b.tx.clone());
        let rect = Self::browser_bounds_rect(body, scale);
        cx.spawn(async move |this, cx| {
            let built = build_view(parent, &context, tx, id, &url, rect);
            let _ = this.update(cx, |app, cx| {
                let Some(b) = app.browser.as_mut() else { return };
                b.creating = false;
                b.applied = None;
                match built {
                    Ok(view) => {
                        if let Some(tab) = b.tabs.iter_mut().find(|t| t.id == id) {
                            tab.view = Some(view);
                        }
                    }
                    Err(why) => b.error = Some(why),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_browser_tab(&self, tab: &Tab, active: bool, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let id = tab.id;
        let title = if tab.title.is_empty() { host_of(&tab.url) } else { tab.title.clone() };
        let icon: AnyElement = if tab.loading {
            Spinner::new().color(c.accent).xsmall().into_any_element()
        } else if let Some(image) = &tab.favicon {
            img(image.clone()).size(px(14.)).flex_none().into_any_element()
        } else {
            Icon::empty().path(assets::GLOBE).size(px(14.)).into_any_element()
        };
        let basis = 190f32.min(620. / count.max(1) as f32).max(56.);
        div()
            .id(("br-tab", id))
            .flex()
            .items_center()
            .gap(px(6.))
            .w(px(basis))
            .min_w(px(56.))
            .h(px(32.))
            .pl(px(9.))
            .pr(px(4.))
            .rounded_t(px(7.))
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(if active { c.text } else { c.muted })
            .when(active, |d| d.bg(c.hover))
            .hover(move |s| s.bg(c.hover))
            .on_click(cx.listener(move |app, _, window, cx| app.browser_switch(id, window, cx)))
            // Middle-click closes, as in browsers.
            .on_mouse_down(MouseButton::Middle, cx.listener(move |app, _, window, cx| app.browser_close_tab(id, window, cx)))
            .child(div().flex_none().text_color(c.muted).child(icon))
            .child(div().flex_1().min_w(px(0.)).truncate().child(title))
            .child(self.icon_button(("br-tab-close", id), IconName::Close).p(px(2.)).on_click(cx.listener(move |app, _, window, cx| {
                cx.stop_propagation();
                app.browser_close_tab(id, window, cx);
            })))
            .into_any_element()
    }

    pub(super) fn render_browser(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let b = self.browser.as_ref()?;
        let room = f32::from(window.viewport_size().width) - if self.sidebar_hidden { 0. } else { self.sidebar_width };
        // It shrinks before the terminal disappears.
        let beside = self.git.as_ref().map_or(0., |g| g.list_width) + if self.files.is_some() { self.files_width.unwrap_or(0.) } else { 0. };
        let width = if b.width > 0. { b.width } else { room * 0.45 }.clamp(320., (room - beside - 320.).max(320.));
        b.shown_width.set(width);
        let active = b.tabs.iter().find(|t| Some(t.id) == b.active);
        let tabs: Vec<AnyElement> = b.tabs.iter().map(|t| self.render_browser_tab(t, Some(t.id) == b.active, b.tabs.len(), cx)).collect();
        let (back, forward) = active.map_or((false, false), |t| (t.can_back, t.can_forward));
        let with_view = |app: &CrewApp, f: &dyn Fn(&WebView)| {
            if let Some(view) = app.browser.as_ref().and_then(|b| b.tabs.iter().find(|t| Some(t.id) == b.active)).and_then(|t| t.view.as_ref()) {
                f(view);
            }
        };
        let body_cell = b.body.clone();
        let body = canvas(move |bounds, _, _| body_cell.set(Some(bounds)), |_, _, _, _| {}).size_full();
        Some(
            div()
                .relative()
                .flex()
                .flex_none()
                .w(px(width))
                .h_full()
                .child(div().flex_none().w(px(1.)).h_full().bg(c.border))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .bg(c.main_bg)
                                .items_end()
                                .gap(px(2.))
                                .pt(px(4.))
                                .px(px(6.))
                                .border_b_1()
                                .border_color(c.border)
                                .child(div().flex().min_w(px(0.)).gap(px(2.)).overflow_hidden().children(tabs))
                                .child(self.icon_button("br-new-tab", IconName::Plus).mb(px(4.)).on_click(cx.listener(|app, _, window, cx| {
                                    app.browser_new_tab(HOME_URL.to_string(), window, cx);
                                    app.browser_focus_address(window, cx);
                                })))
                                .child(div().flex_1().min_w(px(12.)))
                                .child(self.icon_button("br-close", IconName::Close).mb(px(4.)).on_click(cx.listener(|app, _, _, cx| app.close_browser(cx)))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .bg(c.main_bg)
                                .items_center()
                                .gap(px(4.))
                                .px(px(6.))
                                .py(px(4.))
                                .border_b_1()
                                .border_color(c.border)
                                .child(self.icon_button("br-back", IconName::ChevronLeft).when(!back, |d| d.opacity(0.35)).on_click(cx.listener(move |app, _, _, _| {
                                    if back {
                                        with_view(app, &|v| drop(v.go_back()));
                                    }
                                })))
                                .child(self.icon_button("br-forward", IconName::ChevronRight).when(!forward, |d| d.opacity(0.35)).on_click(cx.listener(move |app, _, _, _| {
                                    if forward {
                                        with_view(app, &|v| drop(v.go_forward()));
                                    }
                                })))
                                .child(self.icon_button("br-reload", IconName::RefreshCw).on_click(cx.listener(move |app, _, _, _| with_view(app, &|v| drop(v.reload())))))
                                .child(div().flex_1().min_w(px(0.)).child(Input::new(&b.input).small()))
                                .child(self.icon_button("br-external", IconName::ExternalLink).on_click(cx.listener(|app, _, _, cx| {
                                    if let Some(url) = app.browser.as_ref().and_then(|b| b.tabs.iter().find(|t| Some(t.id) == b.active)).map(|t| t.url.clone()) {
                                        cx.open_url(&url);
                                    }
                                }))),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h(px(0.))
                                .when(!b.page_shown, |d| d.bg(c.main_bg))
                                .child(body)
                                .when_some(b.error.clone(), |d, why| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .p(px(24.))
                                            .text_size(px(13.))
                                            .text_color(c.muted)
                                            .child(format!("{} {why}", t("The browser couldn't start. It needs the Microsoft Edge WebView2 Runtime."))),
                                    )
                                }),
                        ),
                )
                .child(self.resize_grip("browser-resizer", Drag::Browser, -5., cx))
                .into_any_element(),
        )
    }
}
