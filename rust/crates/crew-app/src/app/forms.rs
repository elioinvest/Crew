//! Two shared pieces: a small form in a dialog, and a searchable list that opens beside a button
//! (branches, worktrees, stashes).

use gpui::{App, MouseDownEvent, Point, anchored, deferred};

use super::overlays::Action;
use super::*;

pub(super) enum Field {
    Text { id: &'static str, label: SharedString, state: Entity<InputState>, required: bool },
    Area { id: &'static str, label: SharedString, state: Entity<TextareaState> },
    Check { id: &'static str, label: SharedString, value: bool },
    Select { id: &'static str, label: SharedString, options: Vec<String>, value: String },
}

/// What a form's fields held when it was submitted.
#[derive(Default)]
pub(super) struct Values(HashMap<&'static str, String>);

impl Values {
    pub(super) fn text(&self, id: &str) -> String {
        self.0.get(id).cloned().unwrap_or_default()
    }

    pub(super) fn on(&self, id: &str) -> bool {
        self.0.get(id).is_some_and(|v| v == "1")
    }
}

type Submit = Rc<dyn Fn(&mut CrewApp, Values, &mut Window, &mut Context<CrewApp>)>;

pub(super) struct Form {
    title: SharedString,
    text: Option<SharedString>,
    fields: Vec<Field>,
    ok: SharedString,
    danger: bool,
    submit: Submit,
}

/// How a form is described before it's opened: fields need the window to exist.
pub(super) enum Spec {
    Text { id: &'static str, label: &'static str, value: String, placeholder: &'static str, required: bool },
    Area { id: &'static str, label: &'static str, placeholder: &'static str },
    Check { id: &'static str, label: String, value: bool },
    Select { id: &'static str, label: &'static str, options: Vec<String>, value: String },
}

pub(super) struct PickRow {
    pub(super) label: String,
    pub(super) sub: Option<String>,
    pub(super) right: Option<String>,
    pub(super) on: bool,
    pub(super) disabled: bool,
    pub(super) action: Option<Action>,
    pub(super) buttons: Vec<PickButton>,
    pub(super) menu: Option<Rc<dyn Fn(&mut CrewApp, Point<Pixels>, &mut Context<CrewApp>)>>,
}

impl Default for PickRow {
    fn default() -> Self {
        Self { label: String::new(), sub: None, right: None, on: false, disabled: false, action: None, buttons: Vec::new(), menu: None }
    }
}

pub(super) struct PickButton {
    pub(super) label: String,
    pub(super) danger: bool,
    /// Keeps the list open.
    pub(super) keep: bool,
    pub(super) action: Action,
}

pub(super) enum PickItem {
    Section(String),
    Note(String),
    Row(PickRow),
}

pub(super) type PickRows = Rc<dyn Fn(&CrewApp, &str, &str) -> Vec<PickItem>>;

pub(super) struct Picker {
    pub(super) at: Point<Pixels>,
    above: bool,
    input: Entity<InputState>,
    rows: PickRows,
    index: usize,
    _subscription: gpui::Subscription,
}

impl CrewApp {
    pub(super) fn open_form(
        &mut self,
        title: impl Into<SharedString>,
        text: Option<String>,
        specs: Vec<Spec>,
        ok: impl Into<SharedString>,
        danger: bool,
        submit: impl Fn(&mut CrewApp, Values, &mut Window, &mut Context<CrewApp>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut first: Option<Entity<InputState>> = None;
        let fields = specs
            .into_iter()
            .map(|s| match s {
                Spec::Text { id, label, value, placeholder, required } => {
                    let state = cx.new(|cx| InputState::new(window, cx).placeholder(t(placeholder)).default_value(value));
                    if first.is_none() {
                        first = Some(state.clone());
                    }
                    Field::Text { id, label: t(label).into(), state, required }
                }
                Spec::Area { id, label, placeholder } => Field::Area { id, label: t(label).into(), state: cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 6).placeholder(t(placeholder))) },
                Spec::Check { id, label, value } => Field::Check { id, label: label.into(), value },
                Spec::Select { id, label, options, value } => Field::Select { id, label: t(label).into(), options, value },
            })
            .collect();
        self.remember_focus(window, cx);
        if let Some(input) = first {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        self.form = Some(Form { title: title.into(), text: text.map(Into::into), fields, ok: ok.into(), danger, submit: Rc::new(submit) });
        cx.notify();
    }

    /// Asks a yes or no question.
    pub(super) fn ask(
        &mut self,
        title: impl Into<SharedString>,
        text: impl Into<String>,
        ok: impl Into<SharedString>,
        danger: bool,
        yes: impl Fn(&mut CrewApp, &mut Window, &mut Context<CrewApp>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_form(title, Some(text.into()), Vec::new(), ok, danger, move |app, _, window, cx| yes(app, window, cx), window, cx);
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        let mut values = Values::default();
        for f in &form.fields {
            match f {
                Field::Text { id, state, required, .. } => {
                    let v = state.read(cx).value().trim().to_string();
                    if *required && v.is_empty() {
                        let state = state.clone();
                        state.update(cx, |s, cx| s.focus(window, cx));
                        return;
                    }
                    values.0.insert(id, v);
                }
                Field::Area { id, state, .. } => {
                    values.0.insert(id, state.read(cx).value().trim_end().to_string());
                }
                Field::Check { id, value, .. } => {
                    values.0.insert(id, if *value { "1".into() } else { String::new() });
                }
                Field::Select { id, value, .. } => {
                    values.0.insert(id, value.clone());
                }
            }
        }
        let submit = form.submit.clone();
        self.form = None;
        submit(self, values, window, cx);
        cx.notify();
    }

    pub(super) fn render_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let form = self.form.as_ref()?;
        let fields: Vec<AnyElement> = form
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| match f {
                Field::Text { label, state, .. } => self.field(label.clone(), Input::new(state).small(), None).into_any_element(),
                Field::Area { label, state, .. } => self.field(label.clone(), Textarea::new(state).small(), None).into_any_element(),
                Field::Check { label, value, .. } => {
                    let on = *value;
                    div()
                        .id(("form-check", i))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if let Some(Field::Check { value, .. }) = app.form.as_mut().and_then(|f| f.fields.get_mut(i)) {
                                *value = !*value;
                            }
                            cx.notify();
                        }))
                        .child(self.check_box(if on { Some(true) } else { Some(false) }))
                        .child(label.clone())
                        .into_any_element()
                }
                Field::Select { label, options, value, .. } => {
                    let entries = options
                        .iter()
                        .map(|o| {
                            let picked = o.clone();
                            overlays::entry(o.clone(), move |app, _, _| {
                                if let Some(Field::Select { value, .. }) = app.form.as_mut().and_then(|f| f.fields.get_mut(i)) {
                                    *value = picked.clone();
                                }
                            })
                        })
                        .collect();
                    self.field(label.clone(), self.select(("form-select", i), value.clone(), entries, cx), None).into_any_element()
                }
            })
            .collect();
        let danger = form.danger;
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.heading(form.title.clone()))
            .when_some(form.text.clone(), |d, text| d.child(div().text_size(px(13.)).text_color(c.muted).child(text)))
            .children(fields)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(4.))
                    .child(self.button("form-cancel", t("Cancel"), false).on_click(cx.listener(|app, _, _, cx| {
                        app.form = None;
                        cx.notify();
                    })))
                    .child(
                        self.button("form-ok", form.ok.clone(), true)
                            .when(danger, |d| d.bg(gpui::rgb(0xd93d42)).border_color(gpui::rgb(0xd93d42)))
                            .on_click(cx.listener(|app, _, window, cx| app.submit_form(window, cx))),
                    ),
            );
        Some(self.modal("form", 460., body))
    }

    /// A checkbox: on, off, or partly (None).
    pub(super) fn check_box(&self, state: Option<bool>) -> AnyElement {
        let c = self.colors;
        let on = state == Some(true);
        let some = state.is_none();
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(15.))
            .rounded(px(4.))
            .border_1()
            .border_color(if on || some { c.accent } else { c.muted })
            .when(on, |d| d.bg(c.accent).text_color(gpui::white()).child(Icon::new(IconName::Check).size(px(11.))))
            .when(some, |d| d.child(div().w(px(8.)).h(px(2.)).bg(c.accent)))
            .into_any_element()
    }

    // ---------- the list ----------

    /// Where the keyboard goes back to when the form or picker closes.
    fn remember_focus(&mut self, window: &Window, cx: &App) {
        if self.form.is_none() && self.picker.is_none() {
            self.focus_return = window.focused(cx).map(|h| h.downgrade());
        }
    }

    pub(super) fn open_picker(&mut self, at: Point<Pixels>, above: bool, placeholder: &'static str, rows: PickRows, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(placeholder)));
        self.remember_focus(window, cx);
        input.update(cx, |s, cx| s.focus(window, cx));
        let subscription = cx.subscribe_in(&input, window, |app, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                if let Some(p) = app.picker.as_mut() {
                    p.index = 0;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => app.choose_picked(window, cx),
            _ => {}
        });
        self.picker = Some(Picker { at, above, input, rows, index: 0, _subscription: subscription });
        cx.notify();
    }

    fn picker_items(&self, cx: &App) -> Vec<PickItem> {
        let Some(p) = &self.picker else { return Vec::new() };
        let raw = p.input.read(cx).value().trim().to_string();
        (p.rows)(self, &raw.to_lowercase(), &raw)
    }

    fn selectable(items: &[PickItem]) -> Vec<usize> {
        items.iter().enumerate().filter(|(_, it)| matches!(it, PickItem::Row(r) if r.action.is_some() && !r.disabled)).map(|(i, _)| i).collect()
    }

    fn choose_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.picker_items(cx);
        let choices = Self::selectable(&items);
        let wanted = self.picker.as_ref().map_or(0, |p| p.index);
        let at = if choices.contains(&wanted) { Some(wanted) } else { choices.first().copied() };
        if let Some(PickItem::Row(row)) = at.and_then(|i| items.into_iter().nth(i)) {
            if let Some(action) = row.action {
                self.picker = None;
                action(self, window, cx);
            }
        }
        cx.notify();
    }

    fn picker_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let step: i64 = match event.keystroke.key.as_str() {
            "down" => 1,
            "up" => -1,
            _ => return,
        };
        let items = self.picker_items(cx);
        let choices = Self::selectable(&items);
        if let (Some(p), false) = (self.picker.as_mut(), choices.is_empty()) {
            let now = choices.iter().position(|i| *i == p.index).unwrap_or(0) as i64;
            p.index = choices[(now + step).rem_euclid(choices.len() as i64) as usize];
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn render_picker(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.colors;
        let p = self.picker.as_ref()?;
        let items = self.picker_items(cx);
        let choices = Self::selectable(&items);
        let active = if choices.contains(&p.index) { Some(p.index) } else { choices.first().copied() };
        let height = f32::from(window.viewport_size().height);
        let y = f32::from(p.at.y);
        let list_height = (if p.above { y } else { height - y } - 100.).clamp(120., 420.);
        let rows: Vec<AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(i, item)| match item {
                PickItem::Section(text) => div().px(px(10.)).pt(px(8.)).pb(px(2.)).text_size(px(11.5)).text_color(c.muted).child(text).into_any_element(),
                PickItem::Note(text) => div().px(px(10.)).py(px(8.)).text_size(px(13.)).text_color(c.muted).child(text).into_any_element(),
                PickItem::Row(row) => {
                    let (action, menu) = (row.action.clone(), row.menu.clone());
                    let clickable = action.is_some() && !row.disabled;
                    div()
                        .id(("pick-row", i))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .when(active == Some(i), |d| d.bg(c.hover))
                        .when(row.disabled, |d| d.opacity(0.5))
                        .when(clickable, |d| d.cursor_pointer())
                        .on_click(cx.listener(move |app, _, window, cx| {
                            if let Some(action) = action.as_ref().filter(|_| clickable) {
                                app.picker = None;
                                action(app, window, cx);
                                cx.notify();
                            }
                        }))
                        .when_some(menu, |d, menu| {
                            d.on_mouse_down(MouseButton::Right, cx.listener(move |app, e: &MouseDownEvent, _, cx| menu(app, e.position, cx)))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .child(div().truncate().text_size(px(13.)).text_color(c.text).when(row.on, |d| d.font_weight(gpui::FontWeight::SEMIBOLD)).child(row.label))
                                .when_some(row.sub, |d, sub| d.child(div().truncate().text_size(px(11.5)).text_color(c.muted).child(sub))),
                        )
                        .when_some(row.right, |d, right| d.child(div().flex_none().text_size(px(11.5)).text_color(c.muted).child(right)))
                        .children(row.buttons.into_iter().enumerate().map(|(n, b)| {
                            let (action, keep) = (b.action, b.keep);
                            self.button(SharedString::from(format!("pick-{i}-{n}")), b.label, false)
                                .px(px(7.))
                                .py(px(1.))
                                .when(b.danger, |d| d.text_color(gpui::rgb(0xd93d42)))
                                .occlude()
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    if !keep {
                                        app.picker = None;
                                    }
                                    action(app, window, cx);
                                    cx.notify();
                                }))
                        }))
                        .into_any_element()
                }
            })
            .collect();
        let card = div()
            .id("picker")
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(Self::picker_key))
            .flex()
            .flex_col()
            .w(px(480.))
            .max_w(px(f32::from(window.viewport_size().width) - 16.))
            .rounded(px(10.))
            .bg(c.card)
            .border_1()
            .border_color(c.border)
            .shadow_md()
            .child(div().p(px(6.)).border_b_1().border_color(c.border).child(Input::new(&p.input).small().appearance(false)))
            .child(div().id("picker-list").flex().flex_col().max_h(px(list_height)).overflow_y_scroll().p(px(4.)).children(rows));
        let anchor = if p.above { gpui::Anchor::BottomLeft } else { gpui::Anchor::TopLeft };
        Some(
            div()
                .id("picker-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|app, _, _, cx| {
                    app.picker = None;
                    cx.notify();
                }))
                .child(deferred(anchored().anchor(anchor).position(p.at).snap_to_window_with_margin(px(8.)).child(card)))
                .into_any_element(),
        )
    }
}
