// SPDX-License-Identifier: GPL-3.0-or-later
//! Replacing the installed files by the new ones, keeping the old ones until the new version has
//! started (rollback). Each version lists its files in [`MANIFEST`]: a file the previous version
//! listed and the new one does not is removed (kept in the backup like the replaced ones). Files
//! of no list (settings, the `portable.flag` marker) are never touched.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What `install` did, enough to undo it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// Files that existed and were moved to the backup folder.
    pub replaced: Vec<PathBuf>,
    /// Files that did not exist before.
    pub added: Vec<PathBuf>,
    /// Files of the previous version the new one no longer has, moved to the backup folder.
    #[serde(default)]
    pub removed: Vec<PathBuf>,
    /// Written under the update lock (from 0.20): the updates before took none.
    #[serde(default)]
    pub locked: bool,
}

/// The list of the files of a version, one relative path per line (`/` separated).
pub const MANIFEST: &str = "files.txt";

/// The files `dir`'s manifest lists; none without one. A line that would leave `dir` is skipped.
fn listed(dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(dir.join(MANIFEST)) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| {
            path.components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        })
        .collect()
}

const JOURNAL: &str = "journal.json";

/// Marks a portable copy (kept in its folder, never brought by an update).
const PORTABLE_FLAG: &str = "portable.flag";

/// Unpacks a zip into `dest`. Entries that would land outside of it are an error. Each file
/// is on the disk when this returns: the version is then marked ready, and a power cut must
/// not leave it ready but half written.
pub fn extract(zip: impl io::Read + io::Seek, dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(zip).map_err(io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsafe path in the archive: {}",
                    entry.name().unwrap_or_default()
                ),
            ));
        };
        // The portable archive marks its folder as portable: an installed copy is not.
        if relative == Path::new(PORTABLE_FLAG) {
            continue;
        }
        let out = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out)?;
        io::copy(&mut entry, &mut file)?;
        file.sync_all()?;
    }
    Ok(())
}

fn files_under(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&dir))? {
            let entry = entry?;
            let relative = dir.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(relative);
            } else {
                found.push(relative);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Moves a file, copying across volumes when a rename is not possible.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Puts the files of `staged` into `target`; whatever they replace goes to `backup`. On failure
/// everything done so far is undone before the error is returned. The journal, every move it
/// plans, is on the disk before the first move: an install cut short (power cut, process
/// killed) can be undone later by [`rollback`].
pub fn install(staged: &Path, target: &Path, backup: &Path) -> io::Result<Journal> {
    std::fs::create_dir_all(backup)?;
    let files = files_under(staged)?;
    // Read before the new manifest replaces it.
    let removed: Vec<PathBuf> = listed(target)
        .into_iter()
        .filter(|old| !files.contains(old) && target.join(old).is_file())
        .collect();
    let (replaced, added) = files
        .iter()
        .cloned()
        .partition(|relative| target.join(relative).exists());
    let journal = Journal {
        replaced,
        added,
        removed,
        locked: true,
    };
    let text = serde_json::to_string(&journal).map_err(io::Error::other)?;
    let mut file = std::fs::File::create(backup.join(JOURNAL))?;
    io::Write::write_all(&mut file, text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let moved = (|| {
        for relative in &journal.removed {
            move_file(&target.join(relative), &backup.join(relative))?;
        }
        for relative in &files {
            let to = target.join(relative);
            if journal.replaced.contains(relative) {
                move_file(&to, &backup.join(relative))?;
            }
            move_file(&staged.join(relative), &to)?;
        }
        Ok(())
    })();
    match moved {
        Ok(()) => Ok(journal),
        Err(e) => {
            let _ = undo(&journal, target, backup);
            Err(e)
        }
    }
}

/// Undoes what the journal planned, as far as it was done: a file that never reached the
/// backup was never moved.
fn undo(journal: &Journal, target: &Path, backup: &Path) -> io::Result<()> {
    for relative in &journal.added {
        let _ = std::fs::remove_file(target.join(relative));
    }
    for relative in journal.replaced.iter().chain(&journal.removed) {
        let saved = backup.join(relative);
        if saved.is_file() {
            let _ = std::fs::remove_file(target.join(relative));
            move_file(&saved, &target.join(relative))?;
        }
    }
    Ok(())
}

/// Whether `backup` holds an install not yet known to work (or cut short): [`rollback`]
/// undoes it, [`commit`] keeps it.
pub fn unconfirmed(backup: &Path) -> bool {
    backup.join(JOURNAL).is_file()
}

/// The journal in `backup` was written by an update before 0.20 (it took no lock), less than
/// `within` ago.
pub fn older_and_recent(backup: &Path, within: std::time::Duration) -> bool {
    let path = backup.join(JOURNAL);
    let older = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Journal>(&text).ok())
        .is_some_and(|journal| !journal.locked);
    let recent = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < within);
    older && recent
}

/// Restores the previous version from `backup` (after a failed start of the new one, or an
/// install cut short).
pub fn rollback(target: &Path, backup: &Path) -> io::Result<()> {
    let text = std::fs::read_to_string(backup.join(JOURNAL))?;
    let journal: Journal = serde_json::from_str(&text).map_err(io::Error::other)?;
    undo(&journal, target, backup)?;
    std::fs::remove_dir_all(backup)
}

/// Keeps the new version once it runs. The journal goes first: the old files still in use
/// (by programs of the previous version) may stay until the next start, and must then not be
/// taken for an install to undo.
pub fn commit(backup: &Path) -> io::Result<()> {
    match std::fs::remove_file(backup.join(JOURNAL)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    match std::fs::remove_dir_all(backup) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn put(root: &Path, name: &str, text: &str) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e}"));
        }
        std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
    }

    fn read(root: &Path, name: &str) -> Option<String> {
        std::fs::read_to_string(root.join(name)).ok()
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let (staged, target, backup) = (
            dir.path().join("staged"),
            dir.path().join("app"),
            dir.path().join("backup"),
        );
        put(&target, "vixeeny-daemon.exe", "old daemon");
        put(&target, "portable.flag", "");
        put(&staged, "vixeeny-daemon.exe", "new daemon");
        put(&staged, "licenses/THIRD-PARTY.txt", "new licences");
        (dir, staged, target, backup)
    }

    #[test]
    fn the_new_files_replace_the_old_ones_and_user_files_stay() {
        let (_dir, staged, target, backup) = setup();
        let journal = install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read(&target, "vixeeny-daemon.exe").as_deref(),
            Some("new daemon")
        );
        assert_eq!(
            read(&target, "licenses/THIRD-PARTY.txt").as_deref(),
            Some("new licences")
        );
        assert!(target.join("portable.flag").exists());
        assert_eq!(
            read(&backup, "vixeeny-daemon.exe").as_deref(),
            Some("old daemon")
        );
        assert_eq!(journal.replaced, [PathBuf::from("vixeeny-daemon.exe")]);
        assert_eq!(journal.added, [PathBuf::from("licenses/THIRD-PARTY.txt")]);
    }

    #[test]
    fn a_rollback_brings_the_previous_version_back() {
        let (_dir, staged, target, backup) = setup();
        install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        rollback(&target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read(&target, "vixeeny-daemon.exe").as_deref(),
            Some("old daemon")
        );
        assert_eq!(read(&target, "licenses/THIRD-PARTY.txt"), None);
        assert!(!backup.exists());
    }

    #[test]
    fn a_failure_midway_undoes_what_was_done() {
        let (_dir, staged, target, backup) = setup();
        // `aaa.txt` is replaced first; then `licenses/` cannot be created (a file is in the way).
        put(&staged, "aaa.txt", "new");
        put(&target, "aaa.txt", "old");
        put(&target, "licenses", "in the way");
        assert!(install(&staged, &target, &backup).is_err());
        assert_eq!(read(&target, "aaa.txt").as_deref(), Some("old"));
        assert_eq!(
            read(&target, "vixeeny-daemon.exe").as_deref(),
            Some("old daemon")
        );
    }

    #[test]
    fn files_the_new_version_no_longer_has_go_and_come_back_on_rollback() {
        let (_dir, staged, target, backup) = setup();
        put(&target, "old.dll", "old library");
        put(&target, "user.txt", "yours");
        put(
            &target,
            MANIFEST,
            "vixeeny-daemon.exe\nold.dll\n../outside.txt\n",
        );
        put(
            &staged,
            MANIFEST,
            "vixeeny-daemon.exe\nlicenses/THIRD-PARTY.txt\n",
        );
        let journal = install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(journal.removed, [PathBuf::from("old.dll")]);
        assert_eq!(read(&target, "old.dll"), None);
        assert_eq!(read(&target, "user.txt").as_deref(), Some("yours"));
        assert!(target.join("portable.flag").exists());
        rollback(&target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read(&target, "old.dll").as_deref(), Some("old library"));
        assert_eq!(
            read(&target, MANIFEST).as_deref(),
            Some("vixeeny-daemon.exe\nold.dll\n../outside.txt\n")
        );
    }

    #[test]
    fn without_a_previous_list_nothing_is_removed() {
        let (_dir, staged, target, backup) = setup();
        put(&target, "old.dll", "old library");
        put(&staged, MANIFEST, "vixeeny-daemon.exe\n");
        let journal = install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert!(journal.removed.is_empty());
        assert!(target.join("old.dll").exists());
    }

    #[test]
    fn a_journal_of_an_older_version_still_reads() {
        let journal: Journal = serde_json::from_str(r#"{"replaced":["a.exe"],"added":[]}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(journal.removed.is_empty());
    }

    #[test]
    fn an_install_cut_short_is_undone_later() {
        let (_dir, staged, target, backup) = setup();
        put(&target, "old.dll", "old library");
        put(&target, MANIFEST, "vixeeny-daemon.exe\nold.dll\n");
        put(
            &staged,
            MANIFEST,
            "vixeeny-daemon.exe\nlicenses/THIRD-PARTY.txt\n",
        );
        install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert!(unconfirmed(&backup));
        // As if the power went off before the daemon reached the backup: it never moved.
        std::fs::remove_file(target.join("vixeeny-daemon.exe")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::rename(
            backup.join("vixeeny-daemon.exe"),
            target.join("vixeeny-daemon.exe"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        rollback(&target, &backup).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read(&target, "vixeeny-daemon.exe").as_deref(),
            Some("old daemon")
        );
        assert_eq!(read(&target, "old.dll").as_deref(), Some("old library"));
        assert_eq!(
            read(&target, MANIFEST).as_deref(),
            Some("vixeeny-daemon.exe\nold.dll\n")
        );
        assert_eq!(read(&target, "licenses/THIRD-PARTY.txt"), None);
        assert!(!backup.exists());
    }

    /// An update before 0.20 took no lock: its journal, while recent, may be of an install
    /// still at work, which the new version's start must not undo.
    #[test]
    fn a_recent_journal_of_an_older_update_is_told_apart() {
        let (_dir, staged, target, backup) = setup();
        install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        let hour = std::time::Duration::from_secs(3600);
        assert!(!older_and_recent(&backup, hour), "written under the lock");
        // The journal as an older version wrote it: no `locked` field.
        let journal = std::fs::read_to_string(backup.join(JOURNAL)).unwrap_or_default();
        let older = journal.replace(",\"locked\":true", "");
        assert_ne!(older, journal);
        std::fs::write(backup.join(JOURNAL), older).unwrap_or_else(|e| panic!("{e}"));
        assert!(older_and_recent(&backup, hour));
        assert!(
            !older_and_recent(&backup, std::time::Duration::ZERO),
            "old enough"
        );
        commit(&backup).unwrap_or_else(|e| panic!("{e}"));
        assert!(!older_and_recent(&backup, hour), "no journal");
    }

    #[test]
    fn commit_forgets_the_backup() {
        let (_dir, staged, target, backup) = setup();
        install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        commit(&backup).unwrap_or_else(|e| panic!("{e}"));
        assert!(!unconfirmed(&backup));
        assert!(!backup.exists());
        commit(&backup).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn archives_are_unpacked_and_escaping_paths_refused() {
        let build = |name: &str| {
            let mut out = Vec::new();
            {
                let mut zip = zip::ZipWriter::new(io::Cursor::new(&mut out));
                let options = zip::write::SimpleFileOptions::default();
                zip.start_file(name, options)
                    .unwrap_or_else(|e| panic!("{e}"));
                zip.write_all(b"data").unwrap_or_else(|e| panic!("{e}"));
                zip.finish().unwrap_or_else(|e| panic!("{e}"));
            }
            out
        };
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        extract(io::Cursor::new(build("bin/a.exe")), dir.path()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read(dir.path(), "bin/a.exe").as_deref(), Some("data"));
        assert!(extract(io::Cursor::new(build("../evil.exe")), dir.path()).is_err());
        // The portable archive's flag does not make an installed copy portable.
        extract(io::Cursor::new(build(PORTABLE_FLAG)), dir.path())
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!dir.path().join(PORTABLE_FLAG).exists());
    }
}
