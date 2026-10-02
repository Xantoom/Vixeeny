// SPDX-License-Identifier: GPL-3.0-or-later
//! Named video + audio profiles (plan 5.13, section 10): create, duplicate, rename, delete.

use vixeeny_common::config::{Config, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    /// Empty after trimming.
    EmptyName,
    /// Another profile has this name.
    Taken,
    NotFound,
    /// The last profile cannot be deleted.
    LastOne,
}

fn clean(name: &str) -> Result<String, ProfileError> {
    let name = name.trim();
    if name.is_empty() {
        Err(ProfileError::EmptyName)
    } else {
        Ok(name.to_owned())
    }
}

/// A new profile with the default settings, made the current one.
pub fn create(config: &mut Config, name: &str) -> Result<(), ProfileError> {
    let name = clean(name)?;
    if config.profiles.contains_key(&name) {
        return Err(ProfileError::Taken);
    }
    config.profiles.insert(name.clone(), Profile::default());
    config.video.profile = name;
    Ok(())
}

/// A copy of `from` under a new name, made the current one.
pub fn duplicate(config: &mut Config, from: &str, name: &str) -> Result<(), ProfileError> {
    let name = clean(name)?;
    if config.profiles.contains_key(&name) {
        return Err(ProfileError::Taken);
    }
    let copy = config
        .profiles
        .get(from)
        .ok_or(ProfileError::NotFound)?
        .clone();
    config.profiles.insert(name.clone(), copy);
    config.video.profile = name;
    Ok(())
}

/// Renames `from`; whatever pointed at it (the current profile, the replay's) follows.
pub fn rename(config: &mut Config, from: &str, name: &str) -> Result<(), ProfileError> {
    let name = clean(name)?;
    if name == from {
        return Ok(());
    }
    if config.profiles.contains_key(&name) {
        return Err(ProfileError::Taken);
    }
    let profile = config.profiles.remove(from).ok_or(ProfileError::NotFound)?;
    config.profiles.insert(name.clone(), profile);
    if config.video.profile == from {
        config.video.profile = name.clone();
    }
    if config.replay.profile == from {
        config.replay.profile = name;
    }
    Ok(())
}

/// Deletes `name`. The current profile moves to the first remaining one; a replay that used it
/// goes back to following the recording's.
pub fn delete(config: &mut Config, name: &str) -> Result<(), ProfileError> {
    if !config.profiles.contains_key(name) {
        return Err(ProfileError::NotFound);
    }
    if config.profiles.len() == 1 {
        return Err(ProfileError::LastOne);
    }
    config.profiles.remove(name);
    if config.video.profile == name {
        config.video.profile = config.profiles.keys().next().cloned().unwrap_or_default();
    }
    if config.replay.profile == name {
        config.replay.profile.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let mut c = Config::default();
        c.profiles.insert("default".into(), Profile::default());
        c.video.profile = "default".into();
        c
    }

    #[test]
    fn creating_and_duplicating_make_the_new_profile_current() {
        let mut c = config();
        c.profiles.get_mut("default").unwrap().fps = 30;
        duplicate(&mut c, "default", " Jeu 4K ").unwrap();
        assert_eq!(c.video.profile, "Jeu 4K");
        assert_eq!(c.profiles["Jeu 4K"].fps, 30);
        create(&mut c, "Tuto").unwrap();
        assert_eq!(
            (c.video.profile.as_str(), c.profiles["Tuto"].fps),
            ("Tuto", 60)
        );
        assert_eq!(create(&mut c, "Tuto"), Err(ProfileError::Taken));
        assert_eq!(create(&mut c, "  "), Err(ProfileError::EmptyName));
        assert_eq!(duplicate(&mut c, "nope", "x"), Err(ProfileError::NotFound));
    }

    #[test]
    fn renaming_moves_what_pointed_at_the_profile() {
        let mut c = config();
        c.replay.profile = "default".into();
        rename(&mut c, "default", "Principal").unwrap();
        assert_eq!(c.video.profile, "Principal");
        assert_eq!(c.replay.profile, "Principal");
        assert!(!c.profiles.contains_key("default"));
        create(&mut c, "Autre").unwrap();
        assert_eq!(
            rename(&mut c, "Autre", "Principal"),
            Err(ProfileError::Taken)
        );
        assert_eq!(rename(&mut c, "Autre", "Autre"), Ok(()));
    }

    #[test]
    fn deleting_keeps_at_least_one_profile_and_fixes_the_references() {
        let mut c = config();
        assert_eq!(delete(&mut c, "default"), Err(ProfileError::LastOne));
        create(&mut c, "Tuto").unwrap();
        c.replay.profile = "Tuto".into();
        delete(&mut c, "Tuto").unwrap();
        assert_eq!(c.video.profile, "default");
        assert_eq!(c.replay.profile, "");
        assert_eq!(delete(&mut c, "Tuto"), Err(ProfileError::NotFound));
    }
}
