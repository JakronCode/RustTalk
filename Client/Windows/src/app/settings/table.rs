//! Settings structure as data: groups -> pages -> sections, each section with
//! search keywords. The sidebar, the search and the page renderer all read
//! this table, so a section only has to be added here and in `pages.rs`.

use super::super::icons::regular as ic;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) enum SettingsPage {
    Account,
    Appearance,
    Voice,
    Video,
    Keybinds,
    Notifications,
    Advanced,
    About,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) enum SectionId {
    Profile,
    SavedServers,
    Theme,
    PrimaryColor,
    Density,
    Scaling,
    Chat,
    Devices,
    InputMode,
    MicTest,
    VoiceProcessing,
    Codec,
    Camera,
    Streaming,
    VoiceKeys,
    CameraKeys,
    Desktop,
    Sounds,
    SpokenNames,
    Encryption,
    Links,
    Data,
    Developer,
    Version,
    Updates,
    Feedback,
}

pub(crate) struct Section<S: 'static = SectionId> {
    pub id: S,
    pub title: &'static str,
    /// Lowercase search words, taken from the setting labels in the section.
    pub keywords: &'static [&'static str],
}

pub(crate) struct Page<P: 'static = SettingsPage, S: 'static = SectionId> {
    pub page: P,
    pub title: &'static str,
    pub icon: &'static str,
    pub sections: &'static [Section<S>],
}

pub(crate) struct Group<P: 'static = SettingsPage, S: 'static = SectionId> {
    pub title: &'static str,
    pub pages: &'static [Page<P, S>],
}

pub(crate) const fn s<S>(id: S, title: &'static str, keywords: &'static [&'static str]) -> Section<S> {
    Section {
        id,
        title,
        keywords,
    }
}

use SectionId as S;

#[rustfmt::skip]
pub(crate) const GROUPS: &[Group] = &[
    Group {
        title: "User Settings",
        pages: &[
            Page {
                page: SettingsPage::Account, title: "My Account", icon: ic::USER_CIRCLE,
                sections: &[
                    s(S::Profile, "Profile", &["profile", "username", "name", "avatar", "picture", "status", "online", "away", "do not disturb", "about me", "bio"]),
                    s(S::SavedServers, "Saved servers", &["saved servers", "bookmarks", "favorites", "address", "remove server"]),
                ],
            },
            Page {
                page: SettingsPage::Appearance, title: "Appearance", icon: ic::PAINT_BRUSH,
                sections: &[
                    s(S::Theme, "Theme", &["theme", "dark", "light", "midnight", "mode"]),
                    s(S::PrimaryColor, "Primary color", &["primary color", "accent", "colour", "highlight", "buttons", "links", "selection", "custom color"]),
                    s(S::Density, "Density", &["density", "cozy", "compact", "spacing", "layout", "avatars"]),
                    s(S::Scaling, "Scaling", &["ui scale", "zoom", "size", "chat font size", "text size", "font"]),
                    s(S::Chat, "Chat", &["chat", "messages", "show timestamps", "time", "animated gifs", "gif", "animation"]),
                ],
            },
        ],
    },
    Group {
        title: "App Settings",
        pages: &[
            Page {
                page: SettingsPage::Voice, title: "Voice & Audio", icon: ic::MICROPHONE,
                sections: &[
                    s(S::Devices, "Devices", &["input device", "output device", "microphone", "mic", "speakers", "headphones", "headset", "output volume", "input volume", "gain", "audio"]),
                    s(S::InputMode, "Input mode", &["input mode", "push to talk", "ptt", "voice activity", "voice activation", "vad", "sensitivity", "threshold", "mic", "key"]),
                    s(S::MicTest, "Mic test", &["mic test", "microphone", "mic", "level", "meter"]),
                    s(S::VoiceProcessing, "Voice processing", &["noise suppression", "rnnoise", "noise", "automatic gain control", "agc", "gain", "echo cancellation", "echo", "speakers", "mic", "microphone"]),
                    s(S::Codec, "Codec", &["codec", "opus", "audio quality", "bitrate", "kbit"]),
                ],
            },
            Page {
                page: SettingsPage::Video, title: "Video", icon: ic::VIDEO_CAMERA,
                sections: &[
                    s(S::Camera, "Camera", &["camera", "webcam", "default camera", "refresh devices", "mirror my camera", "background blur", "blur"]),
                    s(S::Streaming, "Streaming", &["stream quality", "streaming", "screen share", "screen", "share audio with screen", "sound", "hardware encoding", "h.264", "gpu"]),
                ],
            },
            Page {
                page: SettingsPage::Keybinds, title: "Keybinds", icon: ic::KEYBOARD,
                sections: &[
                    s(S::VoiceKeys, "Voice", &["keybinds", "shortcuts", "hotkeys", "push to talk", "ptt", "push-to-talk key", "background", "global", "mute", "deafen", "key"]),
                    s(S::CameraKeys, "Camera", &["keybinds", "shortcuts", "hotkeys", "toggle camera", "video"]),
                ],
            },
            Page {
                page: SettingsPage::Notifications, title: "Notifications", icon: ic::BELL,
                sections: &[
                    s(S::Desktop, "Desktop", &["desktop notifications", "notifications", "pop-up", "popup", "flash taskbar", "taskbar", "show message text", "preview", "chat", "messages"]),
                    s(S::Sounds, "Sounds", &["sounds", "audio", "user joins", "user leaves", "join", "leave", "new message", "chat", "mention", "mute", "unmute", "deafen"]),
                    s(S::SpokenNames, "Spoken names", &["say names", "spoken names", "announce", "speech", "text to speech", "tts", "voice", "natural voices", "download", "language", "english", "deutsch", "german"]),
                ],
            },
            Page {
                page: SettingsPage::Advanced, title: "Advanced", icon: ic::SLIDERS_HORIZONTAL,
                sections: &[
                    s(S::Encryption, "Encryption", &["encryption", "tls", "certificate", "fingerprint", "known servers", "forget", "trust", "security"]),
                    s(S::Links, "Links", &["links", "urls", "browser", "trusted sites", "trust", "always open", "revoke", "threat feed", "malware", "phishing", "malicious", "urlhaus", "blocklist", "security"]),
                    s(S::Data, "Data", &["data", "settings file", "config", "show in folder", "reset all settings", "defaults"]),
                    s(S::Developer, "Developer", &["developer", "debug overlay", "statistics", "log level", "logs"]),
                ],
            },
        ],
    },
    Group {
        title: "Info",
        pages: &[Page {
            page: SettingsPage::About, title: "About RustTalk", icon: ic::INFO,
            sections: &[
                s(S::Version, "Version", &["version", "protocol", "build", "about"]),
                s(S::Updates, "Updates", &["updates", "check for updates", "install update", "release notes", "github"]),
                s(S::Feedback, "Feedback", &["feedback", "report a bug", "bug", "issue", "github"]),
            ],
        }],
    },
];

pub(crate) fn page(p: SettingsPage) -> &'static Page {
    GROUPS
        .iter()
        .flat_map(|g| g.pages)
        .find(|pg| pg.page == p)
        .expect("every SettingsPage is in GROUPS")
}

pub(crate) struct PageHit<P: 'static = SettingsPage, S: 'static = SectionId> {
    pub page: &'static Page<P, S>,
    pub sections: Vec<&'static Section<S>>,
}

pub(crate) struct GroupHit<P: 'static = SettingsPage, S: 'static = SectionId> {
    pub group: &'static Group<P, S>,
    pub pages: Vec<PageHit<P, S>>,
}

/// Groups, pages and sections matching `query`. Every whitespace-separated
/// term must appear (case-insensitive) in the section's title or keywords,
/// or in its page or group title. An empty query returns everything.
#[cfg(test)]
pub(crate) fn search(query: &str) -> Vec<GroupHit> {
    search_in(GROUPS, query)
}

pub(crate) fn search_in<P: 'static, S: 'static>(groups: &'static [Group<P, S>], query: &str) -> Vec<GroupHit<P, S>> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    groups
        .iter()
        .filter_map(|group| {
            let pages: Vec<PageHit<P, S>> = group
                .pages
                .iter()
                .filter_map(|page| {
                    let sections: Vec<&Section<S>> = page
                        .sections
                        .iter()
                        .filter(|sec| {
                            terms.iter().all(|t| {
                                [group.title, page.title, sec.title]
                                    .iter()
                                    .any(|title| title.to_lowercase().contains(t.as_str()))
                                    || sec.keywords.iter().any(|k| k.contains(t.as_str()))
                            })
                        })
                        .collect();
                    (!sections.is_empty()).then_some(PageHit { page, sections })
                })
                .collect();
            (!pages.is_empty()).then_some(GroupHit { group, pages })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn hits(query: &str) -> Vec<(SettingsPage, SectionId)> {
        search(query)
            .iter()
            .flat_map(|g| {
                g.pages
                    .iter()
                    .flat_map(|p| p.sections.iter().map(|s| (p.page.page, s.id)))
            })
            .collect()
    }

    fn all_sections() -> impl Iterator<Item = &'static Section> {
        GROUPS.iter().flat_map(|g| g.pages).flat_map(|p| p.sections)
    }

    #[test]
    fn section_ids_are_unique() {
        let mut seen = HashSet::new();
        for sec in all_sections() {
            assert!(seen.insert(sec.id), "duplicate section {:?}", sec.id);
        }
    }

    #[test]
    fn every_page_is_listed_once_with_sections() {
        let all = [
            SettingsPage::Account,
            SettingsPage::Appearance,
            SettingsPage::Voice,
            SettingsPage::Video,
            SettingsPage::Keybinds,
            SettingsPage::Notifications,
            SettingsPage::Advanced,
            SettingsPage::About,
        ];
        for p in all {
            let count = GROUPS
                .iter()
                .flat_map(|g| g.pages)
                .filter(|pg| pg.page == p)
                .count();
            assert_eq!(count, 1, "{p:?}");
            assert!(!page(p).sections.is_empty(), "{p:?} has no sections");
        }
        assert!(GROUPS.iter().all(|g| !g.pages.is_empty()));
    }

    #[test]
    fn keywords_are_lowercase_and_titles_set() {
        for sec in all_sections() {
            assert!(!sec.title.is_empty());
            assert!(!sec.keywords.is_empty(), "{:?} has no keywords", sec.id);
            for k in sec.keywords {
                assert_eq!(*k, k.to_lowercase(), "keyword {k:?} in {:?}", sec.id);
            }
        }
    }

    #[test]
    fn empty_query_shows_everything() {
        let total = all_sections().count();
        assert_eq!(hits("").len(), total);
        assert_eq!(hits("   ").len(), total);
        assert_eq!(search("").len(), GROUPS.len());
    }

    #[test]
    fn chat_finds_appearance_and_notifications() {
        let h = hits("chat");
        assert!(h.contains(&(SettingsPage::Appearance, SectionId::Chat)));
        assert!(h.contains(&(SettingsPage::Notifications, SectionId::Desktop)));
        assert!(h.contains(&(SettingsPage::Notifications, SectionId::Sounds)));
        assert!(!h.iter().any(|(p, _)| *p == SettingsPage::Video));
    }

    #[test]
    fn mic_finds_voice_input_sections() {
        let h = hits("mic");
        for id in [
            SectionId::Devices,
            SectionId::InputMode,
            SectionId::MicTest,
            SectionId::VoiceProcessing,
        ] {
            assert!(h.contains(&(SettingsPage::Voice, id)), "{id:?}");
        }
        assert!(!h.contains(&(SettingsPage::Voice, SectionId::Codec)));
    }

    #[test]
    fn matching_is_case_insensitive_and_uses_titles() {
        assert_eq!(hits("CHAT"), hits("chat"));
        assert_eq!(
            hits("Primary"),
            vec![(SettingsPage::Appearance, SectionId::PrimaryColor)]
        );
        // A page title matches all of its sections.
        assert_eq!(
            hits("keybinds").len(),
            page(SettingsPage::Keybinds).sections.len()
        );
    }

    #[test]
    fn all_terms_must_match() {
        assert_eq!(
            hits("mic test"),
            vec![(SettingsPage::Voice, SectionId::MicTest)]
        );
        assert!(hits("mic zzz").is_empty());
    }

    #[test]
    fn groups_without_matches_are_hidden() {
        let groups = search("timestamps");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group.title, "User Settings");
        assert_eq!(groups[0].pages.len(), 1);
        assert_eq!(groups[0].pages[0].page.page, SettingsPage::Appearance);
        assert!(search("no such setting").is_empty());
    }
}
