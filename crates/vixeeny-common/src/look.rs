// SPDX-License-Identifier: GPL-3.0-or-later
//! How the host wants every window to look: light or dark, the system accent colour, animations.

/// The default accent of Windows 11.
pub const DEFAULT_ACCENT: [u8; 3] = [0x00, 0x78, 0xd4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    pub accent: [u8; 3],
    /// Off when the user turned animations off in Windows.
    pub animations: bool,
}

impl Look {
    pub fn new(dark: bool, accent: Option<[u8; 3]>, animations: bool) -> Self {
        Self {
            dark,
            accent: accent.unwrap_or(DEFAULT_ACCENT),
            animations,
        }
    }

    /// Dark, default accent, animations on (tests, and the places that know nothing better).
    pub const fn dark() -> Self {
        Self {
            dark: true,
            accent: DEFAULT_ACCENT,
            animations: true,
        }
    }

    /// The window background, for the title bar to match.
    pub const fn caption(self) -> [u8; 3] {
        if self.dark {
            [0x14, 0x14, 0x16]
        } else {
            [0xf6, 0xf6, 0xf7]
        }
    }
}

impl Default for Look {
    fn default() -> Self {
        Self::dark()
    }
}

/// Whether the dark theme applies: the `general.theme` setting, or the OS's choice for `system`.
pub fn dark_theme(setting: &str, os_prefers_dark: bool) -> bool {
    match setting {
        "dark" => true,
        "light" => false,
        _ => os_prefers_dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_follows_the_setting_or_the_os() {
        assert!(dark_theme("dark", false));
        assert!(!dark_theme("light", true));
        assert!(dark_theme("system", true));
        assert!(!dark_theme("system", false));
        assert!(dark_theme("whatever", true));
    }
}
