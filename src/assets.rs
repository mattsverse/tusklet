use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "icons/check.svg" => Some(Cow::Borrowed(include_bytes!("../assets/check.svg"))),
            "tusklet.png" => Some(Cow::Borrowed(include_bytes!("../assets/tusklet.png"))),
            _ => None,
        })
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(["icons/check.svg", "tusklet.png"]
            .into_iter()
            .filter(|name| name.starts_with(path))
            .map(Into::into)
            .collect())
    }
}
