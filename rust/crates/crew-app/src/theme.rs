//! Crew's colours.

use gpui::{Hsla, rgb, rgba};

#[derive(Clone, Copy)]
pub struct Colors {
    pub side_bg: Hsla,
    pub main_bg: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub running: Hsla,
    pub card: Hsla,
    pub card_edge: Hsla,
}

fn c(v: u32) -> Hsla {
    rgb(v).into()
}

impl Colors {
    pub fn light() -> Self {
        Self {
            side_bg: c(0xf9f8f6),
            main_bg: c(0xffffff),
            text: c(0x1f1f1f),
            muted: c(0x8a8580),
            hover: c(0xefede9),
            border: c(0xebe8e3),
            accent: c(0xd97757),
            running: c(0x3fa45b),
            card: c(0xffffff),
            card_edge: rgba(0x1f1f1f14).into(),
        }
    }

    pub fn dark() -> Self {
        Self {
            side_bg: c(0x1f1f1f),
            main_bg: c(0x181818),
            text: c(0xe6e3de),
            muted: c(0x8d8984),
            hover: c(0x2a2a2a),
            border: c(0x2c2c2c),
            accent: c(0xd97757),
            running: c(0x3fa45b),
            card: c(0x2b2b2b),
            card_edge: rgba(0xffffff0f).into(),
        }
    }
}
