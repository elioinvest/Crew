//! Phone access, the window's part: the pairing page in Settings, and what a phone asks to be done
//! here, as if done in this window. The server itself is crew-service's remote.rs.

use std::collections::VecDeque;

use crew_service::remote::{RemoteAsk, RemoteInfo, RemoteOpen};
use gpui::{Bounds, canvas, fill, point, size};
use gpui_component::tooltip::Tooltip;
use qrcode::{Color, QrCode};

use super::overlays::MenuEntry;
use super::*;

const QR_SIZE: f32 = 150.;

struct Qr {
    url: String,
    width: usize,
    dark: Vec<bool>,
}

#[derive(Default)]
pub(super) struct PhoneState {
    info: Option<RemoteInfo>,
    qr: Option<Rc<Qr>>,
    pub(super) menu_seq: u64,
    /// The menus phones have open; a pick from one runs here.
    pub(super) menus: VecDeque<(u64, Vec<MenuEntry>)>,
}

impl CrewApp {
    pub(super) fn refresh_phone(&mut self, cx: &mut Context<Self>) {
        let info = self.service.remote_info();
        // The server saves these in config.json too: this window's copy mustn't write over them.
        for (key, value) in self.service.remote_settings() {
            self.config.set(&key, value);
        }
        self.show_phone_info(info, cx);
    }

    fn show_phone_info(&mut self, info: RemoteInfo, cx: &mut Context<Self>) {
        if self.phone.qr.as_ref().map(|q| q.url.as_str()) != Some(info.pair_url.as_str()) {
            self.phone.qr = QrCode::new(info.pair_url.as_bytes()).ok().filter(|_| !info.pair_url.is_empty()).map(|code| {
                let dark = code.to_colors().into_iter().map(|c| c == Color::Dark).collect();
                Rc::new(Qr { url: info.pair_url.clone(), width: code.width(), dark })
            });
        }
        self.phone.info = Some(info);
        cx.notify();
    }

    fn set_phone(&mut self, enabled: Option<bool>, renew: bool, cx: &mut Context<Self>) {
        let info = self.service.remote_set(enabled, renew);
        for (key, value) in self.service.remote_settings() {
            self.config.set(&key, value);
        }
        self.show_phone_info(info, cx);
    }

    /// A phone asked to open a session: resume it, or start a new one in the project.
    pub(super) fn on_remote_open(&mut self, open: RemoteOpen, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(host_id) = open.host_id {
            let Some(h) = self.host_by_id(&host_id).cloned() else { return };
            if open.terminal_only {
                self.show_host_terminal(&host_id, window, cx);
            } else {
                let session = open.session_id.and_then(|id| self.host_sessions(&h).into_iter().find(|s| s.id == id));
                self.open_host(&host_id, session, None, None, window, cx);
            }
            return;
        }
        let path = PathBuf::from(&open.project_path);
        let session = open.session_id.as_deref().and_then(|id| self.projects.iter().flat_map(|p| p.sessions.iter()).find(|s| s.id == id)).cloned();
        match session.filter(|_| open.resume) {
            Some(s) => self.open_session(&s, &path, window, cx),
            None => self.start_new_with(&open.agent, path, open.provider, None, window, cx),
        }
        cx.notify();
    }

    pub(super) fn on_remote_ask(&mut self, ask: RemoteAsk, window: &mut Window, cx: &mut Context<Self>) {
        match ask.what.as_str() {
            "chips" | "menu" => self.phone_chips(ask, cx),
            "pick" => {
                let action = self.phone.menus.iter().find(|(id, _)| *id == ask.menu_id).and_then(|(_, items)| items.get(ask.index)).map(|it| it.action.clone());
                let picked = action.is_some();
                if let Some(action) = action {
                    action(self, window, cx);
                }
                self.service.remote_answer(ask.id, if picked { json!(true) } else { Value::Null });
                cx.notify();
            }
            _ => self.service.remote_answer(ask.id, Value::Null),
        }
    }

    pub(super) fn render_phone_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let info = self.phone.info.clone().unwrap_or_default();
        let on = info.enabled;
        let toggle = div()
            .id("cfg-remote")
            .flex()
            .flex_none()
            .items_center()
            .w(px(36.))
            .h(px(20.))
            .p(px(2.))
            .rounded_full()
            .cursor_pointer()
            .bg(if on { c.accent } else { c.border })
            .when(on, |d| d.justify_end())
            .on_click(cx.listener(move |app, _, _, cx| app.set_phone(Some(!on), false, cx)))
            .child(div().size(px(16.)).rounded_full().bg(gpui::white()));
        let mut card = self.settings_card().child(self.settings_row("Let paired phones connect", "The pairing code and addresses appear here once enabled.", toggle));
        if !on {
            return card.into_any_element();
        }
        let addresses = if info.addresses.is_empty() {
            vec![t("No network address found").to_string()]
        } else {
            info.addresses.iter().map(|a| format!("{}:{}{}", a.address, info.port, if a.kind == "tailscale" { " (Tailscale, from anywhere)" } else { " (same Wi-Fi)" })).collect()
        };
        let devices = if info.devices.is_empty() { t("No phone connected").to_string() } else { format!("Connected: {}", info.devices.join(", ")) };
        let code = info.code.clone();
        let qr = self.phone.qr.clone();
        card = card.child(
            div()
                .flex()
                .items_start()
                .gap(px(16.))
                .px(px(16.))
                .py(px(14.))
                .border_t_1()
                .border_color(c.border)
                .children(qr.map(render_qr))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(12.5))
                        .text_color(c.muted)
                        .child(t("Scan this code with the phone camera to open Crew in the phone browser (or with the Crew app). Or open the address below in the phone browser and enter the code:"))
                        .child(
                            div()
                                .id("remote-code")
                                .flex_none()
                                .max_w_full()
                                .self_start()
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(6.))
                                .bg(c.hover)
                                .text_color(c.text)
                                .text_size(px(12.))
                                .font_family("Cascadia Mono")
                                .cursor_pointer()
                                .on_click(cx.listener(move |app, _, _, cx| app.copy_text(code.clone(), cx)))
                                .child(info.code.clone()),
                        )
                        .child(div().flex().flex_col().gap(px(3.)).children(addresses.into_iter().map(|a| div().child(a))))
                        .child(div().child(devices))
                        .when(!info.error.is_empty(), |d| d.child(div().text_color(gpui::rgb(0xd93d42)).child(info.error.clone())))
                        .child(
                            div().flex().mt(px(2.)).child(
                                self.button("remote-new", t("New pairing code"), false)
                                    .tooltip(|window, cx| Tooltip::new(t("Paired phones have to scan the new code")).build(window, cx))
                                    .on_click(cx.listener(|app, _, _, cx| app.set_phone(None, true, cx))),
                            ),
                        ),
                ),
        );
        card.into_any_element()
    }
}

fn render_qr(qr: Rc<Qr>) -> AnyElement {
    // Whole pixels per module and one module of margin, so a camera reads it cleanly.
    let modules = qr.width + 2;
    let cell = (QR_SIZE / modules as f32).floor().max(1.);
    let side = cell * modules as f32;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, gpui::white()));
            for (i, dark) in qr.dark.iter().enumerate() {
                if *dark {
                    let (x, y) = ((i % qr.width + 1) as f32, (i / qr.width + 1) as f32);
                    let origin = bounds.origin + point(px(x * cell), px(y * cell));
                    window.paint_quad(fill(Bounds::new(origin, size(px(cell), px(cell))), gpui::black()));
                }
            }
        },
    )
    .flex_none()
    .size(px(side))
    .into_any_element()
}
