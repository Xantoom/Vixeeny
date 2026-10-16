// SPDX-License-Identifier: GPL-3.0-or-later
//! What tells a game from another program that fills the screen: where the launchers install
//! their games, the programs Windows knows as games (Game Bar), and exclusive full screen.
//! Nothing decides alone: [`is_game`] weighs these signals with the user's own lists.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey,
    RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::core::{HSTRING, PWSTR};

use crate::{MonitorInfo, WindowId, WindowInfo};

/// The launchers: they live where the games are, but are not games.
const LAUNCHERS: &[&str] = &[
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "galaxyclient.exe",
    "upc.exe",
    "ubisoftconnect.exe",
    "eadesktop.exe",
    "battle.net.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "riotclientuxrender.exe",
    "leagueclient.exe",
    "leagueclientux.exe",
    "leagueclientuxrender.exe",
    "playnite.desktopapp.exe",
    "playnite.fullscreenapp.exe",
    "rockstarlauncher.exe",
    "xboxpcapp.exe",
];

/// The signs are read again, when next needed, after this long (a game installed meanwhile).
const READ_AGAIN: Duration = Duration::from_secs(5 * 60);

/// What is known about the program in the foreground.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    /// Its window covers a whole monitor.
    pub full_screen: bool,
    /// It has the screen to itself (exclusive full screen).
    pub exclusive: bool,
    /// A launcher installed it, or Windows knows it as a game.
    pub installed_game: bool,
    /// Windows will not say where its executable is: an anti-cheat protects it.
    pub protected: bool,
}

/// The user's word: programs that are games even without a sign of it, and programs that never
/// are (executable names, lower case).
#[derive(Debug, Clone, Copy)]
pub struct Lists<'a> {
    pub always: &'a [String],
    pub never: &'a [String],
}

/// Whether `exe` (the executable's file name, lower case) is a game, from the `facts` and the
/// user's lists.
pub fn is_game(exe: &str, facts: Facts, lists: Lists<'_>) -> bool {
    if !facts.full_screen || lists.never.iter().any(|e| e == exe) {
        return false;
    }
    if lists.always.iter().any(|e| e == exe) {
        return true;
    }
    !LAUNCHERS.contains(&exe) && (facts.exclusive || facts.installed_game || facts.protected)
}

/// The executable's file name, lower case, as the lists hold it.
pub fn exe_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
}

/// Whether `window` belongs to Vixeeny: this process, or an executable next to it.
pub fn is_ours(window: &WindowInfo) -> bool {
    if window.pid == std::process::id() {
        return true;
    }
    let dir = |p: &Path| p.parent().map(|d| d.to_string_lossy().to_lowercase());
    match (window.exe_path.as_deref(), std::env::current_exe()) {
        (Some(exe), Ok(me)) => dir(Path::new(exe)).is_some() && dir(Path::new(exe)) == dir(&me),
        _ => false,
    }
}

/// A window covering a whole monitor: a game or a video, not a window among others.
pub fn is_full_screen(window: &WindowInfo, monitors: &[MonitorInfo]) -> bool {
    let w = &window.rect;
    monitors.iter().any(|m| {
        let m = &m.rect;
        w.x <= m.x
            && w.y <= m.y
            && w.x + w.width as i32 >= m.x + m.width as i32
            && w.y + w.height as i32 >= m.y + m.height as i32
    })
}

/// Finds the game in the foreground; keeps the signs between calls.
#[derive(Default)]
pub struct Detector {
    signals: GameSignals,
    read_at: Option<Instant>,
}

impl Detector {
    /// The full-screen game in the foreground, if there is one.
    pub fn game_in_front(&mut self, lists: Lists<'_>) -> Option<WindowInfo> {
        let window = crate::foreground_window().ok()??;
        self.game(window, lists)
    }

    /// `window` if it is a full-screen game (the window Windows reported in the foreground).
    pub fn game_at(&mut self, window: WindowId, lists: Lists<'_>) -> Option<WindowInfo> {
        let window = crate::window_info(window).ok()??;
        self.game(window, lists)
    }

    /// A game protected by an anti-cheat hides its executable's path: it is then known by its
    /// file name alone (in `exe_path` too).
    fn game(&mut self, mut window: WindowInfo, lists: Lists<'_>) -> Option<WindowInfo> {
        let protected = window.exe_path.is_none();
        if protected {
            window.exe_path = Some(crate::process_name(window.pid)?);
        }
        let path = window.exe_path.clone()?;
        let exe = exe_name(&path)?;
        let monitors = crate::monitors().ok()?;
        let full_screen = is_full_screen(&window, &monitors);
        if !full_screen || is_ours(&window) {
            return None;
        }
        if self.read_at.is_none_or(|t| t.elapsed() >= READ_AGAIN) {
            self.signals = GameSignals::read();
            self.read_at = Some(Instant::now());
        }
        let facts = Facts {
            full_screen,
            exclusive: exclusive_full_screen(),
            installed_game: !protected && self.signals.installed_game(&path),
            protected,
        };
        is_game(&exe, facts, lists).then_some(window)
    }
}

/// The name a game is shown by: as its executable describes itself, else the title of its
/// window, else its file name. An engine's own name, left in the description by the game,
/// says nothing of it.
pub fn display_name(path: &str, title: &str) -> String {
    const ENGINES: &[&str] = &["ue4", "ue5", "unrealgame", "unreal engine", "unity"];
    let meta = crate::exe_metadata(path);
    [
        meta.product_name,
        meta.file_description,
        Some(title.to_owned()),
    ]
    .into_iter()
    .flatten()
    .map(|n| n.trim().to_owned())
    .find(|n| !n.is_empty() && !ENGINES.contains(&n.to_lowercase().as_str()))
    .unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    })
}

/// Where games are installed on this computer, read from the launchers and from Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameSignals {
    /// The launchers' game folders (Steam, Epic, GOG, EA, Ubisoft, Battle.net, Riot, Xbox),
    /// normalised by [`normalise`].
    pub folders: Vec<String>,
    /// The executables Windows recognised as games (Game Bar), normalised.
    pub known: Vec<String>,
}

impl GameSignals {
    /// Reads the launchers' settings and the registry (a few milliseconds).
    pub fn read() -> Self {
        let mut folders = Vec::new();
        folders.extend(steam_folders());
        folders.extend(epic_folders());
        folders.extend(xbox_folders());
        for (root, key, value) in [
            (
                HKEY_LOCAL_MACHINE,
                r"SOFTWARE\WOW6432Node\GOG.com\Games",
                "path",
            ),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\GOG.com\Games", "path"),
            (
                HKEY_LOCAL_MACHINE,
                r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs",
                "InstallDir",
            ),
        ] {
            folders.extend(subkey_strings(root, key, value));
        }
        folders.extend(publisher_folders());
        let mut folders: Vec<String> = folders
            .iter()
            .filter(|f| specific_enough(f))
            .map(|f| normalise(f))
            .collect();
        folders.sort();
        folders.dedup();
        let mut known: Vec<String> = subkey_strings(
            HKEY_CURRENT_USER,
            r"System\GameConfigStore\Children",
            "MatchedExeFullPath",
        )
        .iter()
        .map(|p| normalise_file(p))
        .collect();
        known.sort();
        known.dedup();
        Self { folders, known }
    }

    /// Whether `exe` (a full path) is installed by a launcher, known to Windows as a game, or
    /// made with a game engine.
    pub fn installed_game(&self, exe: &str) -> bool {
        let file = normalise_file(exe);
        self.known.binary_search(&file).is_ok()
            || self.folders.iter().any(|f| file.starts_with(f))
            || made_with_an_engine(exe)
    }
}

/// A full-screen Direct3D program has the screen to itself (exclusive mode). Borderless full
/// screen, the most common today, does not show here.
pub fn exclusive_full_screen() -> bool {
    use windows::Win32::UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
    // SAFETY: no arguments.
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN)
}

/// A folder as compared: lower case, backslashes, one trailing backslash.
pub fn normalise(folder: &str) -> String {
    let mut f = normalise_file(folder);
    while f.ends_with('\\') {
        f.pop();
    }
    f.push('\\');
    f
}

fn normalise_file(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

/// A folder deep enough to hold games only: not a drive, not `Program Files` itself.
fn specific_enough(folder: &str) -> bool {
    let parts: Vec<String> = normalise(folder)
        .split('\\')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    match parts.as_slice() {
        [] | [_] => false,
        [_, second] => !matches!(
            second.as_str(),
            "program files" | "program files (x86)" | "programdata" | "users" | "windows"
        ),
        _ => true,
    }
}

/// `steamapps\common` of every Steam library.
fn steam_folders() -> Vec<String> {
    let Some(steam) = string_value(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath") else {
        return Vec::new();
    };
    let steam = PathBuf::from(steam);
    let mut libraries = vec![steam.display().to_string()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")) {
        libraries.extend(steam_libraries(&vdf));
    }
    libraries
        .into_iter()
        .map(|l| {
            Path::new(&l)
                .join(r"steamapps\common")
                .display()
                .to_string()
        })
        .collect()
}

/// The `"path"` entries of `libraryfolders.vdf`.
fn steam_libraries(vdf: &str) -> Vec<String> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            (quoted.next()? == "path").then(|| quoted.next().map(|p| p.replace(r"\\", r"\")))?
        })
        .collect()
}

/// The install folders in the Epic launcher's manifests.
fn epic_folders() -> Vec<String> {
    let data = std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    let Ok(entries) = std::fs::read_dir(data.join(r"Epic\EpicGamesLauncher\Data\Manifests")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|json| json_string(&json, "InstallLocation"))
        .collect()
}

/// The value of `"field": "..."` in a JSON text (its escapes undone).
fn json_string(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let rest = &json[json.find(&key)? + key.len()..];
    let rest = rest
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// `X:\XboxGames` on every drive (where the Xbox app installs PC Game Pass games).
fn xbox_folders() -> Vec<String> {
    ('C'..='Z')
        .map(|d| format!(r"{d}:\XboxGames"))
        .filter(|f| Path::new(f).is_dir())
        .collect()
}

/// The publishers whose installed programs are games (their launchers are told apart by name).
const GAME_PUBLISHERS: &[&str] = &[
    "electronic arts",
    "blizzard entertainment",
    "riot games",
    "rockstar games",
    "ubisoft",
    "kuro technology",
    "kuro games",
    "cognosphere",
    "mihoyo",
    "hoyoverse",
];

/// Whether `exe` (a full path) was made with a game engine: an Unreal Engine game's
/// executable is `<name>-Win64-Shipping.exe`, a Unity game has the engine's library beside it.
/// Games that come with their own launcher are found this way.
fn made_with_an_engine(exe: &str) -> bool {
    let path = Path::new(exe);
    let unreal = path
        .file_stem()
        .is_some_and(|s| s.to_string_lossy().to_lowercase().ends_with("-shipping"));
    unreal
        || path.parent().is_some_and(|dir| {
            ["UnityPlayer.dll", "GameAssembly.dll"]
                .iter()
                .any(|lib| dir.join(lib).is_file())
        })
}

/// The install folders of the programs published by [`GAME_PUBLISHERS`], from the list of
/// installed programs.
fn publisher_folders() -> Vec<String> {
    let mut folders = Vec::new();
    for (root, key) in [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ] {
        for sub in subkeys(root, key) {
            let path = format!(r"{key}\{sub}");
            let publisher = string_value(root, &path, "Publisher").unwrap_or_default();
            let publisher = publisher.to_lowercase();
            if !GAME_PUBLISHERS.iter().any(|p| publisher.contains(p)) {
                continue;
            }
            // Some installers leave the location out: the folder of their icon then.
            let folder = string_value(root, &path, "InstallLocation")
                .map(|f| f.trim_matches('"').to_owned())
                .filter(|f| !f.is_empty())
                .or_else(|| {
                    let icon = string_value(root, &path, "DisplayIcon")?;
                    let icon = icon.trim_matches('"');
                    let file = icon.split(',').next().unwrap_or(icon);
                    Some(Path::new(file).parent()?.to_string_lossy().into_owned())
                });
            folders.extend(folder);
        }
    }
    folders
}

/// A string value of a registry key.
fn string_value(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let mut bytes = 0u32;
    // SAFETY: a size query: no buffer, `bytes` receives the size.
    let status = unsafe {
        RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&raw mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buffer` holds `bytes` bytes; RRF_RT_REG_SZ checks the type.
    let status = unsafe {
        RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]);
    (!text.trim().is_empty()).then_some(text)
}

/// The names of the subkeys of a registry key.
fn subkeys(root: HKEY, key: &str) -> Vec<String> {
    let mut opened = HKEY::default();
    // SAFETY: `opened` receives the key, closed below.
    let status =
        unsafe { RegOpenKeyExW(root, &HSTRING::from(key), None, KEY_READ, &raw mut opened) };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    let mut buffer = [0u16; 256];
    for index in 0.. {
        let mut len = buffer.len() as u32;
        // SAFETY: `buffer` holds `len` characters; the optional out-pointers are not asked for.
        let status = unsafe {
            RegEnumKeyExW(
                opened,
                index,
                Some(PWSTR(buffer.as_mut_ptr())),
                &raw mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..len as usize]));
    }
    // SAFETY: opened above.
    let _ = unsafe { RegCloseKey(opened) };
    names
}

/// `value` of every subkey of `key`.
fn subkey_strings(root: HKEY, key: &str, value: &str) -> Vec<String> {
    subkeys(root, key)
        .iter()
        .filter_map(|sub| string_value(root, &format!(r"{key}\{sub}"), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steam_libraries_are_read_from_their_file() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            steam_libraries(vdf),
            [r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]
        );
    }

    #[test]
    fn the_epic_install_folder_is_read_from_its_manifest() {
        let json = "{\n\t\"FormatVersion\": 0,\n\t\"InstallLocation\": \"C:\\\\Program Files\\\\Epic Games\\\\Fortnite\",\n}";
        assert_eq!(
            json_string(json, "InstallLocation").as_deref(),
            Some(r"C:\Program Files\Epic Games\Fortnite")
        );
        assert_eq!(json_string(json, "Missing"), None);
    }

    #[test]
    fn a_game_is_found_by_its_folder_or_as_windows_knows_it() {
        let signals = GameSignals {
            folders: vec![normalise(r"D:\SteamLibrary\steamapps\common")],
            known: vec![normalise_file(r"C:\Games\Indie\indie.exe")],
        };
        assert!(signals.installed_game(r"D:\SteamLibrary\steamapps\common\Hades\Hades.exe"));
        assert!(signals.installed_game("d:/steamlibrary/steamapps/common/x/x.exe"));
        assert!(signals.installed_game(r"C:\GAMES\Indie\Indie.exe"));
        assert!(!signals.installed_game(r"D:\SteamLibrary\steamapps\commonX\a.exe"));
        assert!(!signals.installed_game(r"C:\Program Files\VideoLAN\VLC\vlc.exe"));
    }

    #[test]
    fn a_game_engine_is_a_sign_of_a_game() {
        assert!(made_with_an_engine(
            r"C:\Program Files\Wuthering Waves\Wuthering Waves Game\Client\Binaries\Win64\Client-Win64-Shipping.exe"
        ));
        assert!(!made_with_an_engine(r"C:\Windows\System32\notepad.exe"));
        let dir = std::env::temp_dir().join("vixeeny-unity-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("UnityPlayer.dll"), b"").unwrap();
        assert!(made_with_an_engine(&dir.join("Game.exe").to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drive_or_program_files_is_not_a_game_folder() {
        assert!(!specific_enough(r"C:\"));
        assert!(!specific_enough(r"C:\Program Files"));
        assert!(!specific_enough(""));
        assert!(specific_enough(r"C:\Riot Games"));
        assert!(specific_enough(r"C:\Program Files\EA Games\Battlefield 6"));
    }

    fn facts(full_screen: bool, exclusive: bool, installed_game: bool) -> Facts {
        Facts {
            full_screen,
            exclusive,
            installed_game,
            protected: false,
        }
    }

    const NONE: Lists<'static> = Lists {
        always: &[],
        never: &[],
    };

    #[test]
    fn a_full_screen_program_needs_a_sign_of_a_game() {
        // A film, a slideshow, the desktop: full screen, no sign.
        assert!(!is_game("vlc.exe", facts(true, false, false), NONE));
        assert!(!is_game("powerpnt.exe", facts(true, false, false), NONE));
        // Installed by a launcher, known to Windows, or exclusive full screen.
        assert!(is_game("hades.exe", facts(true, false, true), NONE));
        assert!(is_game("old.exe", facts(true, true, false), NONE));
        // A game in a window is not recorded.
        assert!(!is_game("hades.exe", facts(false, true, true), NONE));
        // An anti-cheat hides where it is installed: that is a sign too.
        let protected = Facts {
            protected: true,
            ..facts(true, false, false)
        };
        assert!(is_game("client-win64-shipping.exe", protected, NONE));
    }

    #[test]
    fn the_launchers_are_not_games() {
        assert!(!is_game("steam.exe", facts(true, false, true), NONE));
        assert!(!is_game(
            "leagueclientux.exe",
            facts(true, false, true),
            NONE
        ));
    }

    #[test]
    fn the_user_has_the_last_word() {
        let (always, never) = (
            ["indie.exe".to_owned()],
            ["hades.exe".to_owned(), "steam.exe".to_owned()],
        );
        let lists = Lists {
            always: &always,
            never: &never,
        };
        assert!(is_game("indie.exe", facts(true, false, false), lists));
        assert!(!is_game("hades.exe", facts(true, true, true), lists));
        // Still in full screen only.
        assert!(!is_game("indie.exe", facts(false, false, false), lists));
    }

    #[test]
    fn the_lists_hold_file_names_in_lower_case() {
        assert_eq!(
            exe_name(r"C:\Games\Hades\Hades.EXE").as_deref(),
            Some("hades.exe")
        );
    }
}
