// SPDX-License-Identifier: GPL-3.0-or-later
//! Replacing the installed files by the new ones, keeping the old ones until the new version has
//! started (plan 10.2, rollback). Files that are not part of the update (settings, the
//! `portable.flag` marker) are never touched.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What `install` did, enough to undo it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// Files that existed and were moved to the backup folder.
    pub replaced: Vec<PathBuf>,
    /// Files that did not exist before.
    pub added: Vec<PathBuf>,
}

const JOURNAL: &str = "journal.json";

/// Unpacks a zip into `dest`. Entries that would land outside of it are an error.
pub fn extract(zip_bytes: &[u8], dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(io::Cursor::new(zip_bytes)).map_err(io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsafe path in the archive: {}", entry.name()),
            ));
        };
        let out = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        std::fs::write(&out, data)?;
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
/// everything done so far is undone before the error is returned.
pub fn install(staged: &Path, target: &Path, backup: &Path) -> io::Result<Journal> {
    std::fs::create_dir_all(backup)?;
    let mut journal = Journal::default();
    let result = files_under(staged).and_then(|files| {
        for relative in files {
            let (from, to) = (staged.join(&relative), target.join(&relative));
            if to.exists() {
                move_file(&to, &backup.join(&relative))?;
                journal.replaced.push(relative.clone());
            } else {
                journal.added.push(relative.clone());
            }
            // The old file is already moved away when this fails: the caller's undo brings it back.
            move_file(&from, &to)?;
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            let text = serde_json::to_string(&journal).map_err(io::Error::other)?;
            std::fs::write(backup.join(JOURNAL), text)?;
            Ok(journal)
        }
        Err(e) => {
            let _ = undo(&journal, target, backup);
            Err(e)
        }
    }
}

fn undo(journal: &Journal, target: &Path, backup: &Path) -> io::Result<()> {
    for relative in &journal.added {
        let _ = std::fs::remove_file(target.join(relative));
    }
    for relative in &journal.replaced {
        let _ = std::fs::remove_file(target.join(relative));
        move_file(&backup.join(relative), &target.join(relative))?;
    }
    Ok(())
}

/// Restores the previous version from `backup` (after a failed start of the new one).
pub fn rollback(target: &Path, backup: &Path) -> io::Result<()> {
    let text = std::fs::read_to_string(backup.join(JOURNAL))?;
    let journal: Journal = serde_json::from_str(&text).map_err(io::Error::other)?;
    undo(&journal, target, backup)?;
    std::fs::remove_dir_all(backup)
}

/// Forgets the previous version once the new one runs.
pub fn commit(backup: &Path) -> io::Result<()> {
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
    fn commit_forgets_the_backup() {
        let (_dir, staged, target, backup) = setup();
        install(&staged, &target, &backup).unwrap_or_else(|e| panic!("{e}"));
        commit(&backup).unwrap_or_else(|e| panic!("{e}"));
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
        extract(&build("bin/a.exe"), dir.path()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read(dir.path(), "bin/a.exe").as_deref(), Some("data"));
        assert!(extract(&build("../evil.exe"), dir.path()).is_err());
    }
}
