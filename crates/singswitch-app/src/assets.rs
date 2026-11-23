//! Icon assets for the app.
//!
//! `gpui_kit::assets::Assets` embeds only the 106 icons its own components use,
//! which left the app with generic chrome (copy, ellipsis, chevrons) and nothing
//! that names a tunnel, an endpoint or a profile. This source adds the Lucide
//! glyphs this domain needs on top of that bundle, rather than pulling all 1830
//! in (+7.3 MB) for the ~20 that are actually used.

use gpui_kit::AssetSource;
use gpui_kit::assets::icon_assets;

// Keep this list honest: an icon added here but never used costs bytes, and one
// used but not listed renders as nothing at all.
icon_assets!(
    DomainIcons,
    [
        Cable,
        CirclePlus,
        CloudDownload,
        FilePlus,
        FolderCog,
        Link,
        Monitor,
        PowerOff,
        Route,
        ScrollText,
        Trash,
        Waypoints,
    ]
);

#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = DomainIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(DomainIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
