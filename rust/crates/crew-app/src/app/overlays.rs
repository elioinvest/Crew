//! What floats over the window: menus, small dialogs and toasts, and the controls forms are made of.

use gpui::{MouseDownEvent, Point, anchored, deferred};

use super::*;

pub(super) type Action = Rc<dyn Fn(&mut CrewApp, &mut Window, &mut Context<CrewApp>)>;

#[derive(Clone)]
pub(super) struct MenuEntry {
    pub(super) label: SharedString,
    danger: bool,
    pub(super) separator: bool,
    pub(super) checked: bool,
    pub(super) hint: SharedString,
    pub(super) action: Action,
}

pub(super) fn entry(label: impl Into<SharedString>, action: impl Fn(&mut CrewApp, &mut Window, &mut Context<CrewApp>) + 'static) -> MenuEntry {
    let label: SharedString = label.into();
    let label = crate::i18n::lookup(&label).map_or(label, Into::into);
    MenuEntry { label, danger: false, separator: false, checked: false, hint: SharedString::default(), action: Rc::new(action) }
}

impl MenuEntry {
    pub(super) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub(super) fn separated(mut self) -> Self {
        self.separator = true;
        self
    }

    pub(super) fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    /// Quiet text beside the label.
    pub(super) fn hint(mut self, text: impl Into<SharedString>) -> Self {
        self.hint = text.into();
        self
    }
}

pub(super) struct Menu {
    pub(super) at: Point<Pixels>,
    entries: Vec<MenuEntry>,
    above: bool,
    opened: Instant,
    /// The row it was opened from, which keeps its highlight and buttons meanwhile.
    owner: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastKind {
    Plain,
    Ok,
    Error,
}

pub(super) struct Toast {
    text: SharedString,
    kind: ToastKind,
    seq: u64,
}

pub(super) enum Dialog {
    Confirm { title: SharedString, text: SharedString, ok: SharedString, danger: bool, action: Action },
    Info { title: SharedString, paragraphs: Vec<SharedString> },
}

impl CrewApp {
    pub(super) fn open_menu(&mut self, at: Point<Pixels>, entries: Vec<MenuEntry>, cx: &mut Context<Self>) {
        self.menu = Some(Menu { at, entries, above: false, opened: Instant::now(), owner: None });
        cx.notify();
    }

    pub(super) fn set_menu_owner(&mut self, owner: String) {
        if let Some(m) = self.menu.as_mut() {
            m.owner = Some(owner);
        }
    }

    pub(super) fn menu_owner(&self) -> Option<&str> {
        self.menu.as_ref().and_then(|m| m.owner.as_deref())
    }

    /// A press outside closes the menu, unless it's the second half of a double click: that would
    /// close a menu the moment it appeared.
    fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.as_ref().is_some_and(|m| m.opened.elapsed() > Duration::from_millis(350)) {
            self.menu = None;
            cx.notify();
        }
    }

    /// Opens over what was clicked: the chips sit at the bottom of the window.
    pub(super) fn open_menu_above(&mut self, at: Point<Pixels>, entries: Vec<MenuEntry>, cx: &mut Context<Self>) {
        self.menu = Some(Menu { at, entries, above: true, opened: Instant::now(), owner: None });
        cx.notify();
    }

    pub(super) fn toast(&mut self, text: impl Into<SharedString>, kind: ToastKind, cx: &mut Context<Self>) {
        let seq = self.toast.as_ref().map_or(1, |t| t.seq + 1);
        self.toast = Some(Toast { text: text.into(), kind, seq });
        let wait = if kind == ToastKind::Error { 8 } else { 4 };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(wait)).await;
            let _ = this.update(cx, |app, cx| {
                if app.toast.as_ref().is_some_and(|t| t.seq == seq) {
                    app.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        text: impl Into<SharedString>,
        ok: impl Into<SharedString>,
        action: impl Fn(&mut CrewApp, &mut Window, &mut Context<CrewApp>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(Dialog::Confirm { title: title.into(), text: text.into(), ok: ok.into(), danger: true, action: Rc::new(action) });
        cx.notify();
    }

    /// Asks before something that isn't destructive.
    pub(super) fn offer(
        &mut self,
        title: impl Into<SharedString>,
        text: impl Into<SharedString>,
        ok: impl Into<SharedString>,
        action: impl Fn(&mut CrewApp, &mut Window, &mut Context<CrewApp>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(Dialog::Confirm { title: title.into(), text: text.into(), ok: ok.into(), danger: false, action: Rc::new(action) });
        cx.notify();
    }

    pub(super) fn info(&mut self, title: impl Into<SharedString>, paragraphs: Vec<SharedString>, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::Info { title: title.into(), paragraphs });
        cx.notify();
    }

    /// Dims the window behind a card. Clicks stop here, so nothing underneath reacts.
    pub(super) fn modal(&self, id: &'static str, width: f32, body: impl IntoElement) -> AnyElement {
        let c = self.colors;
        div()
            .id(id)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .occlude()
            .child(
                div()
                    .id("modal-card")
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .w(px(width))
                    .max_h(relative(0.9))
                    .overflow_y_scroll()
                    .p(px(20.))
                    .rounded(px(12.))
                    .bg(c.card)
                    .border_1()
                    .border_color(c.border)
                    .text_size(px(13.5))
                    .text_color(c.text)
                    .child(body),
            )
            .into_any_element()
    }

    pub(super) fn heading(&self, text: impl Into<SharedString>) -> gpui::Div {
        div().text_size(px(17.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(self.colors.text).child(text.into())
    }

    pub(super) fn button(&self, id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
        let c = self.colors;
        div()
            .id(id)
            .flex_none()
            .px(px(14.))
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .cursor_pointer()
            .text_size(px(13.))
            .when(primary, |d| d.bg(c.accent).border_color(c.accent).text_color(gpui::white()))
            .when(!primary, |d| d.border_color(c.border).text_color(c.text).hover(move |s| s.bg(c.hover)))
            .child(label.into())
    }

    pub(super) fn field(&self, label: impl Into<SharedString>, control: impl IntoElement, hint: Option<SharedString>) -> gpui::Div {
        let c = self.colors;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .gap(px(4.))
            .child(div().text_size(px(12.5)).text_color(c.muted).child(label.into()))
            .child(control)
            .when_some(hint, |d, hint| d.child(div().text_size(px(12.)).text_color(c.muted).child(hint)))
    }

    /// A button showing the current choice; pressing it lists the others.
    pub(super) fn select(&self, id: impl Into<gpui::ElementId>, value: impl Into<SharedString>, entries: Vec<MenuEntry>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = self.colors;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(c.border)
            .cursor_pointer()
            .text_size(px(13.))
            .hover(move |s| s.bg(c.hover))
            .on_mouse_down(MouseButton::Left, cx.listener(move |app, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                app.open_menu(e.position, entries.clone(), cx);
            }))
            .child(div().flex_1().min_w(px(0.)).truncate().child(value.into()))
            .child(div().flex_none().text_color(c.muted).child(Icon::new(IconName::ChevronDown).size(px(14.))))
    }

    pub(super) fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let menu = self.menu.as_ref()?;
        let any_checked = menu.entries.iter().any(|e| e.checked);
        let corner = if menu.above { gpui::Anchor::BottomLeft } else { gpui::Anchor::TopLeft };
        let rows = menu.entries.iter().enumerate().map(|(i, e)| {
            let action = e.action.clone();
            div()
                .when(e.separator, |d| d.mt(px(4.)).pt(px(4.)).border_t_1().border_color(c.border))
                .child(
                    div()
                        .id(("menu-entry", i))
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .text_color(if e.danger { gpui::rgb(0xd93d42).into() } else { c.text })
                        .hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.menu = None;
                            action(app, window, cx);
                            cx.notify();
                        }))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(any_checked, |d| d.child(div().flex_none().w(px(14.)).when(e.checked, |d| d.child(Icon::new(IconName::Check).size(px(14.))))))
                        .child(div().flex_none().child(e.label.clone()))
                        .when(!e.hint.is_empty(), |d| d.child(div().flex_1().min_w(px(0.)).max_w(px(320.)).truncate().text_size(px(12.)).text_color(c.muted).child(e.hint.clone()))),
                )
        });
        Some(
            div()
                .id("menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|app, _, _, cx| app.dismiss_menu(cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|app, _, _, cx| app.dismiss_menu(cx)))
                .child(deferred(
                    anchored().anchor(corner).position(menu.at).snap_to_window_with_margin(px(8.)).child(
                        div()
                            .id("menu")
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .flex()
                            .flex_col()
                            .min_w(px(180.))
                            .max_w(px(420.))
                            .p(px(4.))
                            .rounded(px(10.))
                            .bg(c.card)
                            .border_1()
                            .border_color(c.border)
                            .shadow_md()
                            .children(rows),
                    ),
                ))
                .into_any_element(),
        )
    }

    pub(super) fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let body = match self.dialog.as_ref()? {
            Dialog::Confirm { title, text, ok, danger, action } => {
                let action = action.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(self.heading(title.clone()))
                    .child(div().child(text.clone()))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(self.button("dialog-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                                app.dialog = None;
                                cx.notify();
                            })))
                            .child(self.button("dialog-ok", ok.clone(), true).when(*danger, |d| d.bg(gpui::rgb(0xd93d42)).border_color(gpui::rgb(0xd93d42))).on_click(cx.listener(move |app, _, window, cx| {
                                app.dialog = None;
                                action(app, window, cx);
                                cx.notify();
                            }))),
                    )
            }
            Dialog::Info { title, paragraphs } => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(self.heading(title.clone()))
                .children(paragraphs.iter().map(|p| div().text_color(c.text).child(p.clone())))
                .child(div().flex().justify_end().child(self.button("dialog-ok", t("OK"), true).on_click(cx.listener(|app, _, _, cx| {
                    app.dialog = None;
                    cx.notify();
                })))),
        };
        Some(self.modal("dialog", 460., body))
    }

    pub(super) fn render_toast(&self) -> Option<AnyElement> {
        let c = self.colors;
        let toast = self.toast.as_ref()?;
        Some(
            div()
                .absolute()
                .bottom(px(40.))
                .left(px(0.))
                .right(px(0.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .max_w(px(620.))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(c.card)
                        .border_1()
                        .border_color(match toast.kind {
                            ToastKind::Plain => c.border,
                            ToastKind::Ok => c.running,
                            ToastKind::Error => gpui::rgb(0xd93d42).into(),
                        })
                        .shadow_md()
                        .text_size(px(13.))
                        .text_color(c.text)
                        .child(toast.text.clone()),
                )
                .into_any_element(),
        )
    }
}
