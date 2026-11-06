//! Icons: the component library's set, plus the few Crew uses that aren't in it.

use std::borrow::Cow;
use std::sync::Arc;

use gpui::{AssetSource, Image, ImageFormat, Result, SharedString};

pub const GIT_BRANCH: &str = "crew/git-branch.svg";
pub const FOLDER_PLUS: &str = "crew/folder-plus.svg";
pub const SPLIT_RIGHT: &str = "crew/split-right.svg";
pub const SPLIT_DOWN: &str = "crew/split-down.svg";
pub const CLAUDE: &str = "crew/claude.svg";
pub const OPENAI: &str = "crew/openai.svg";
pub const SERVER: &str = "crew/server.svg";
pub const CHART: &str = "crew/chart.svg";
pub const FOLDER: &str = "crew/folder.svg";
pub const DOTS: &str = "crew/dots.svg";
pub const SQUARE_PEN: &str = "crew/square-pen.svg";
pub const SETTINGS: &str = "crew/settings.svg";
pub const FILES: &str = "crew/files.svg";
pub const GLOBE: &str = "crew/globe.svg";
pub const PLUG: &str = "crew/plug.svg";
pub const PHONE: &str = "crew/phone.svg";

/// The app's own mark, in colour: icons above are masks that take the text's colour.
pub fn logo() -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../assets/logo.png").to_vec()))
}

pub struct CrewAssets;

impl AssetSource for CrewAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Some(Cow::Borrowed(match path {
            GIT_BRANCH => include_bytes!("../assets/icons/git-branch.svg"),
            FOLDER_PLUS => include_bytes!("../assets/icons/folder-plus.svg"),
            SPLIT_RIGHT => include_bytes!("../assets/icons/split-right.svg"),
            SPLIT_DOWN => include_bytes!("../assets/icons/split-down.svg"),
            CLAUDE => include_bytes!("../assets/icons/claude.svg"),
            OPENAI => include_bytes!("../assets/icons/openai.svg"),
            SERVER => include_bytes!("../assets/icons/server.svg"),
            CHART => include_bytes!("../assets/icons/chart.svg"),
            FOLDER => include_bytes!("../assets/icons/folder.svg"),
            DOTS => include_bytes!("../assets/icons/dots.svg"),
            SQUARE_PEN => include_bytes!("../assets/icons/square-pen.svg"),
            SETTINGS => include_bytes!("../assets/icons/settings.svg"),
            FILES => include_bytes!("../assets/icons/files.svg"),
            GLOBE => include_bytes!("../assets/icons/globe.svg"),
            PLUG => include_bytes!("../assets/icons/plug.svg"),
            PHONE => include_bytes!("../assets/icons/phone.svg"),
            _ => return gpui_kit_assets::Assets.load(path),
        })))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit_assets::Assets.list(path)
    }
}
