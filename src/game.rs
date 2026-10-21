//! "Is a game in focus?" Pure rules, no OS calls: the platform layer reports
//! what the focused app looks like ([`Foreground`]) and [`GameSettings`]
//! decides. While a game is in focus, screen edges don't switch PCs, so a
//! flick to the edge of the screen mid-game never throws the mouse elsewhere.
//!
//! A game is an app that:
//! - fills its whole monitor (borderless or fullscreen, not just maximized), or
//! - runs in exclusive fullscreen, or
//! - has trapped the cursor inside its window (shooters, mouse-look).
//!
//! Browsers and video players never count just for being fullscreen, so
//! fullscreen YouTube, Netflix or VLC doesn't lock you in. They still count if
//! they trap the cursor, which browser games do and videos never do.

use serde::{Deserialize, Serialize};

/// What the focused app looks like right now (reported by the platform).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Foreground {
    /// Program file name, e.g. "valorant.exe".
    pub app: String,
    /// Its window covers its entire monitor (and isn't just maximized).
    pub fullscreen: bool,
    /// Windows says a Direct3D app owns the screen exclusively.
    pub exclusive_fullscreen: bool,
    /// The cursor is confined to part of the desktop (the app trapped it).
    pub cursor_confined: bool,
}

/// `[games]` in glide.toml. Applies to whichever PC the game runs on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GameSettings {
    /// Don't switch PCs at the screen edges while a game is in focus.
    pub stay_on_this_pc: bool,
    /// Programs that always count as games, e.g. ["minecraft.exe"].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub always: Vec<String>,
    /// Programs that never count, e.g. a fullscreen app that isn't a game.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub never: Vec<String>,
}

impl Default for GameSettings {
    fn default() -> Self {
        GameSettings {
            stay_on_this_pc: true,
            always: Vec::new(),
            never: Vec::new(),
        }
    }
}

/// Browsers and video players: fullscreen here means a video, not a game.
const VIDEO_APPS: &[&str] = &[
    // Browsers (fullscreen YouTube, Twitch, Netflix on the web...)
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "arc.exe",
    "zen.exe",
    "librewolf.exe",
    "waterfox.exe",
    "floorp.exe",
    "iexplore.exe",
    // Video players
    "vlc.exe",
    "mpv.exe",
    "mpvnet.exe",
    "mpc-hc.exe",
    "mpc-hc64.exe",
    "mpc-be.exe",
    "mpc-be64.exe",
    "potplayer.exe",
    "potplayer64.exe",
    "potplayermini.exe",
    "potplayermini64.exe",
    "wmplayer.exe",
    "video.ui.exe",
    "microsoft.media.player.exe",
    "smplayer.exe",
    "kmplayer64x.exe",
    "gom.exe",
    "plex.exe",
    "plex htpc.exe",
    "kodi.exe",
    "stremio.exe",
    "netflix.exe",
    // Hosts Store apps (Netflix, Disney+, Movies & TV)
    "applicationframehost.exe",
];

/// Windows' own full-screen surfaces: never games.
const SHELL_APPS: &[&str] = &[
    "explorer.exe",
    "lockapp.exe",
    "searchhost.exe",
    "searchapp.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
];

/// "Minecraft" / "minecraft.exe" / "C:\Games\Minecraft.exe" -> "minecraft.exe".
pub fn normalize_app(name: &str) -> String {
    let file = name
        .trim()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if file.is_empty() || file.ends_with(".exe") {
        file
    } else {
        format!("{file}.exe")
    }
}

/// "valorant.exe" -> "valorant", for display.
pub fn display_name(app: &str) -> &str {
    app.strip_suffix(".exe").unwrap_or(app)
}

pub fn is_video_app(app: &str) -> bool {
    VIDEO_APPS.contains(&app)
}

impl GameSettings {
    /// The game in focus (its program name), or `None` if there isn't one or
    /// this is turned off.
    pub fn game_in_focus(&self, fg: Option<&Foreground>) -> Option<String> {
        let fg = fg?;
        (self.stay_on_this_pc && self.is_game(fg)).then(|| fg.app.clone())
    }

    fn is_game(&self, fg: &Foreground) -> bool {
        let app = normalize_app(&fg.app);
        let listed = |list: &[String]| list.iter().any(|a| normalize_app(a) == app);
        if listed(&self.never) || SHELL_APPS.contains(&app.as_str()) {
            return false;
        }
        if listed(&self.always) {
            return true;
        }
        if is_video_app(&app) {
            return fg.cursor_confined;
        }
        fg.fullscreen || fg.exclusive_fullscreen || fg.cursor_confined
    }

    /// Add `app` to the always/never list (normalized, no duplicates), and
    /// take it off the other one.
    pub fn add(&mut self, app: &str, always: bool) {
        let app = normalize_app(app);
        if app.is_empty() {
            return;
        }
        let (into, other) = if always {
            (&mut self.always, &mut self.never)
        } else {
            (&mut self.never, &mut self.always)
        };
        other.retain(|a| normalize_app(a) != app);
        if !into.iter().any(|a| normalize_app(a) == app) {
            into.push(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fg(app: &str) -> Foreground {
        Foreground {
            app: app.into(),
            ..Default::default()
        }
    }

    fn game(s: &GameSettings, f: Foreground) -> bool {
        s.game_in_focus(Some(&f)).is_some()
    }

    #[test]
    fn fullscreen_or_trapped_cursor_means_a_game() {
        let s = GameSettings::default();
        assert!(game(
            &s,
            Foreground {
                fullscreen: true,
                ..fg("eldenring.exe")
            }
        ));
        assert!(game(
            &s,
            Foreground {
                exclusive_fullscreen: true,
                ..fg("cs2.exe")
            }
        ));
        assert!(
            game(
                &s,
                Foreground {
                    cursor_confined: true,
                    ..fg("valorant.exe")
                }
            ),
            "windowed, mouse trapped"
        );
        assert!(!game(&s, fg("code.exe")), "an ordinary window");
        assert_eq!(s.game_in_focus(None), None, "nothing in focus");
    }

    #[test]
    fn fullscreen_video_is_not_a_game() {
        let s = GameSettings::default();
        for app in [
            "chrome.exe",
            "msedge.exe",
            "firefox.exe",
            "vlc.exe",
            "mpv.exe",
            "applicationframehost.exe",
        ] {
            assert!(
                !game(
                    &s,
                    Foreground {
                        fullscreen: true,
                        exclusive_fullscreen: true,
                        ..fg(app)
                    }
                ),
                "{app}"
            );
        }
        // A browser game that grabs the mouse still counts.
        assert!(game(
            &s,
            Foreground {
                fullscreen: true,
                cursor_confined: true,
                ..fg("chrome.exe")
            }
        ));
    }

    #[test]
    fn windows_own_screens_never_count() {
        let s = GameSettings::default();
        for app in ["explorer.exe", "lockapp.exe", "searchhost.exe"] {
            assert!(
                !game(
                    &s,
                    Foreground {
                        fullscreen: true,
                        cursor_confined: true,
                        ..fg(app)
                    }
                ),
                "{app}"
            );
        }
    }

    #[test]
    fn lists_override_detection() {
        let mut s = GameSettings::default();
        s.add("Minecraft", true);
        s.add("C:\\Tools\\Presenter.EXE", false);
        assert_eq!(s.always, ["minecraft.exe"]);
        assert_eq!(s.never, ["presenter.exe"]);
        assert!(game(&s, fg("minecraft.exe")), "always, even windowed");
        assert!(
            !game(
                &s,
                Foreground {
                    fullscreen: true,
                    ..fg("presenter.exe")
                }
            ),
            "never, even fullscreen"
        );
        s.add("vlc", true);
        assert!(
            game(
                &s,
                Foreground {
                    fullscreen: true,
                    ..fg("vlc.exe")
                }
            ),
            "a video player the user calls a game"
        );

        s.add("presenter", true);
        assert!(s.never.is_empty(), "moved, not duplicated");
        assert_eq!(s.always, ["minecraft.exe", "vlc.exe", "presenter.exe"]);
    }

    #[test]
    fn turned_off_means_never() {
        let s = GameSettings {
            stay_on_this_pc: false,
            ..Default::default()
        };
        assert!(!game(
            &s,
            Foreground {
                fullscreen: true,
                ..fg("eldenring.exe")
            }
        ));
    }

    #[test]
    fn names_are_normalized() {
        assert_eq!(normalize_app("  Minecraft "), "minecraft.exe");
        assert_eq!(normalize_app("C:/Games/X/Game.exe"), "game.exe");
        assert_eq!(normalize_app(""), "");
        assert_eq!(display_name("valorant.exe"), "valorant");
    }
}
