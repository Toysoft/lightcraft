//! Community and project links (Help menu, About dialog, top-bar Discord button).
//!
//! Official Eidos destinations do not exist yet (see the official-URLs TODO list in
//! `docs/eidos-release.md`): every `EIDOS_*` URL below is `None` until Nostos publishes it.
//! Upstream LightCraft links are kept only where they truthfully describe their destination
//! (the original source, its docs, its community) and are always labelled as upstream —
//! never as official Eidos destinations.

/// Official Eidos website. `None` until Nostos publishes it.
pub const EIDOS_WEBSITE: Option<&str> = None;
/// Official Eidos source repository. `None` until Nostos publishes it.
pub const EIDOS_SOURCE: Option<&str> = None;
/// Official Eidos user documentation. `None` until Nostos publishes it.
pub const EIDOS_DOCS: Option<&str> = None;
/// Official Eidos downloads/releases page. `None` until Nostos publishes it.
pub const EIDOS_DOWNLOADS: Option<&str> = None;
/// Official Eidos issue tracker. `None` until Nostos publishes it.
pub const EIDOS_ISSUES: Option<&str> = None;
/// Official Eidos community/support channel. `None` until Nostos publishes it.
pub const EIDOS_COMMUNITY: Option<&str> = None;

/// The upstream app's id in ArtCraft URLs (Eidos was forked from LightCraft).
pub const UPSTREAM_APP: &str = "lightcraft";
/// The ArtCraft community Discord (upstream community; labelled as such).
pub const UPSTREAM_DISCORD: &str = "https://discord.gg/artcraft";
/// The ArtCraft website.
pub const UPSTREAM_SITE: &str = "https://getartcraft.com";
/// The upstream app's page on the ArtCraft website.
pub const UPSTREAM_PAGE: &str = "https://getartcraft.com/apps/lightcraft";
/// The upstream source repository (Eidos's origin).
pub const UPSTREAM_SOURCE: &str = "https://github.com/storytold/lightcraft";
/// The upstream user documentation.
pub const UPSTREAM_DOCS: &str = "https://github.com/storytold/lightcraft/tree/main/docs";
/// The upstream issue tracker.
pub const UPSTREAM_ISSUES: &str = "https://github.com/storytold/lightcraft/issues/new";

/// (UI command id, menu label, URL) for each link, in Help-menu order.
pub const LINKS: &[(&str, &str, &str)] = &[
    ("app.help", "Upstream LightCraft Docs", UPSTREAM_DOCS),
    ("app.discord", "Join the ArtCraft Discord…", UPSTREAM_DISCORD),
    ("app.website", "Upstream LightCraft Page", UPSTREAM_PAGE),
    ("app.github", "Upstream Source on GitHub", UPSTREAM_SOURCE),
    ("app.artcraft", "ArtCraft Website", UPSTREAM_SITE),
    ("app.feedback", "Upstream Issue Tracker…", UPSTREAM_ISSUES),
];

/// The URL behind a link command id.
pub fn url_of(cmd: &str) -> Option<&'static str> {
    LINKS.iter().find(|(id, _, _)| *id == cmd).map(|(_, _, u)| *u)
}

/// Open `url` in the user's browser (through the host's `open_url` service).
pub fn open(app: &mut crate::EidosApp, url: &str) -> Result<serde_json::Value, String> {
    let open = app.services.open_url.as_mut().ok_or("can't open links here")?;
    open(url)?;
    Ok(serde_json::json!({ "url": url }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_https_and_name_their_destination() {
        for (id, label, url) in LINKS {
            assert!(url.starts_with("https://"), "{id}");
            assert!(!label.is_empty());
            assert_eq!(url_of(id), Some(*url));
        }
        assert!(UPSTREAM_PAGE.ends_with(&format!("/apps/{UPSTREAM_APP}")));
        assert!(UPSTREAM_SOURCE.ends_with(&format!("/storytold/{UPSTREAM_APP}")));
    }

    #[test]
    fn official_eidos_urls_are_still_unpublished() {
        // TODO(eidos-urls): fill these in when Nostos publishes each destination, then wire
        // the Help menu and About dialog to them (see docs/eidos-release.md).
        for (name, url) in [
            ("website", EIDOS_WEBSITE),
            ("source", EIDOS_SOURCE),
            ("docs", EIDOS_DOCS),
            ("downloads", EIDOS_DOWNLOADS),
            ("issues", EIDOS_ISSUES),
            ("community", EIDOS_COMMUNITY),
        ] {
            assert!(url.is_none(), "Eidos {name} URL was published: wire it up");
        }
    }
}
