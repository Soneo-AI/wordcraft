//! WordCraft inside another app's panel (KoAssist): the host decides which parts of the chrome
//! show. With [`WordApp::embedded`](crate::WordApp::embedded) set, the title bar (AutoSave, the
//! command search, the community link, the account) is left out, File and the backstage are
//! not offered (the host keeps its own files and versions), the ribbon shows the host's choice of
//! tabs plus the contextual ones, groups the host doesn't want are skipped, the tab strip's
//! Share button gives way to Undo and Redo (the Quick Access Toolbar lives in the hidden title
//! bar), and the status bar keeps page, words, language and zoom but not the view switches.
//!
//! Everything here is additive: without the option the app looks and behaves as before.

use serde::Deserialize;

/// What the host wants shown; all fields optional, an empty list means "as WordCraft has it".
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Embedded {
    /// Ribbon tabs to show, by their English names (`Home`, `Insert`, `Layout`, …); empty = all
    /// but File. Contextual tabs (Picture Format, Table Design, …) always come when they apply.
    #[serde(default)]
    pub tabs: Vec<String>,
    /// Ribbon groups to leave out, by their English titles (`Clipboard`, `Voice`, …).
    #[serde(default)]
    pub hidden_groups: Vec<String>,
    /// Whether the status bar shows at all.
    #[serde(default = "yes")]
    pub status_bar: bool,
}

fn yes() -> bool {
    true
}

impl Embedded {
    /// Whether a (non-contextual) ribbon tab is offered. File never is: the host owns files.
    pub fn shows_tab(&self, name: &str) -> bool {
        if name == "File" {
            return false;
        }
        self.tabs.is_empty() || self.tabs.iter().any(|t| t == name)
    }

    /// Whether a ribbon group (by its English title) is drawn.
    pub fn shows_group(&self, title: &str) -> bool {
        !self.hidden_groups.iter().any(|g| g == title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_is_never_offered_and_an_empty_list_means_all() {
        let e = Embedded::default();
        assert!(!e.shows_tab("File"));
        assert!(e.shows_tab("Home"));
        assert!(e.shows_tab("Mailings"));
        assert!(e.shows_group("Clipboard"));
    }

    #[test]
    fn the_host_picks_tabs_and_drops_groups() {
        let e: Embedded = serde_json::from_value(serde_json::json!({
            "tabs": ["Home", "Insert"],
            "hidden_groups": ["Clipboard", "Voice"],
        }))
        .unwrap();
        assert!(e.shows_tab("Home"));
        assert!(!e.shows_tab("Mailings"));
        assert!(!e.shows_tab("File"));
        assert!(!e.shows_group("Clipboard"));
        assert!(e.shows_group("Font"));
        assert!(e.status_bar, "the status bar stays unless the host turns it off");
    }
}
