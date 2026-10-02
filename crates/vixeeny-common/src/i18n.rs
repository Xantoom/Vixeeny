// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal translations for the daemon, which links neither Slint nor gettext (plan 4.1).
//! The app and settings UI use Slint's `@tr()`; only the tray menu and notifications of the
//! daemon go through this table. French and English are shipped (plan 5.16).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    TrayTooltip,
    TrayTooltipRecording,
    MenuSettings,
    MenuQuit,
    AppCrashed,
    HotkeysUnavailable,
    OcrTitle,
    OcrCopy,
    OcrOpenSettings,
    OcrNoText,
    /// `{language}`
    OcrLanguageUsed,
    /// `{languages}`, `{command}`
    OcrMissingLanguages,
    /// `{languages}`
    OcrNoLanguage,
    /// `{error}`
    OcrFailed,
    ScrollTitle,
    ScrollIntro,
    ScrollStart,
    ScrollFinish,
    ScrollCancel,
    RecWidgetPause,
    RecWidgetResume,
    RecWidgetStop,
    /// `{height}`
    ScrollCapturing,
    ScrollLost,
    /// `{height}`
    ScrollTruncated,
    ConvMenuLabel,
    ConvTitle,
    ConvDropHint,
    ConvAddFiles,
    ConvAddFolder,
    ConvClear,
    ConvFormat,
    ConvQuality,
    ConvLossless,
    ConvExisting,
    ConvRename,
    ConvOverwrite,
    ConvSkip,
    ConvOutput,
    ConvSameFolder,
    ConvChoose,
    ConvReset,
    ConvConvert,
    ConvCancel,
    /// `{count}`
    ConvSummary,
    /// `{done}`, `{total}`
    ConvProgress,
    /// `{converted}`, `{skipped}`, `{failed}`
    ConvDone,
    /// `{converted}`, `{failed}`, `{cancelled}`
    ConvCancelled,
    ConvNothing,
}

impl Lang {
    /// Parses a BCP-47-ish tag (`fr`, `fr-FR`, `fr_FR.UTF-8`); unknown languages fall back to
    /// English.
    pub fn from_tag(tag: &str) -> Self {
        let primary = tag
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match primary.as_str() {
            "fr" => Self::Fr,
            _ => Self::En,
        }
    }

    /// Resolves the `general.language` setting. `auto` uses `os_locale`, the tag reported by
    /// the OS, when available, else the `LC_ALL` / `LC_MESSAGES` / `LANG` variables.
    pub fn resolve(setting: &str, os_locale: Option<&str>) -> Self {
        if setting != "auto" {
            return Self::from_tag(setting);
        }
        if let Some(tag) = os_locale {
            return Self::from_tag(tag);
        }
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()))
            .map_or(Self::En, |tag| Self::from_tag(&tag))
    }
}

pub fn tr(key: Key, lang: Lang) -> &'static str {
    match (key, lang) {
        (Key::TrayTooltip, _) => "Vixeeny",
        (Key::TrayTooltipRecording, Lang::En) => "Vixeeny — recording",
        (Key::TrayTooltipRecording, Lang::Fr) => "Vixeeny — enregistrement",
        (Key::MenuSettings, Lang::En) => "Settings",
        (Key::MenuSettings, Lang::Fr) => "Paramètres",
        (Key::MenuQuit, Lang::En) => "Quit",
        (Key::MenuQuit, Lang::Fr) => "Quitter",
        (Key::AppCrashed, Lang::En) => "Vixeeny stopped unexpectedly",
        (Key::AppCrashed, Lang::Fr) => "Vixeeny s'est arrêté de façon inattendue",
        (Key::HotkeysUnavailable, Lang::En) => {
            "Some shortcuts could not be registered (already used elsewhere?)"
        }
        (Key::HotkeysUnavailable, Lang::Fr) => {
            "Certains raccourcis n'ont pas pu être enregistrés (déjà utilisés ailleurs ?)"
        }
        (Key::OcrTitle, Lang::En) => "Text recognition",
        (Key::OcrTitle, Lang::Fr) => "Reconnaissance de texte",
        (Key::OcrCopy, Lang::En) => "Copy",
        (Key::OcrCopy, Lang::Fr) => "Copier",
        (Key::OcrOpenSettings, Lang::En) => "Open language settings",
        (Key::OcrOpenSettings, Lang::Fr) => "Ouvrir les paramètres de langue",
        (Key::OcrNoText, Lang::En) => "No text found",
        (Key::OcrNoText, Lang::Fr) => "Aucun texte trouvé",
        (Key::OcrLanguageUsed, Lang::En) => "Language: {language} — copied to the clipboard",
        (Key::OcrLanguageUsed, Lang::Fr) => "Langue : {language} — copié dans le presse-papier",
        (Key::OcrMissingLanguages, Lang::En) => {
            "Missing recognition language: {languages}. In Windows Settings → Time & language → Language & region, add the language with “Optical character recognition”, or run in an administrator PowerShell: Add-WindowsCapability -Online -Name {command}"
        }
        (Key::OcrMissingLanguages, Lang::Fr) => {
            "Langue de reconnaissance manquante : {languages}. Dans Paramètres Windows → Heure et langue → Langue et région, ajoutez la langue avec « Reconnaissance optique des caractères », ou exécutez dans PowerShell administrateur : Add-WindowsCapability -Online -Name {command}"
        }
        (Key::OcrNoLanguage, Lang::En) => "No recognition language is installed for: {languages}",
        (Key::OcrNoLanguage, Lang::Fr) => {
            "Aucune langue de reconnaissance n'est installée pour : {languages}"
        }
        (Key::OcrFailed, Lang::En) => "Text recognition failed: {error}",
        (Key::OcrFailed, Lang::Fr) => "Échec de la reconnaissance de texte : {error}",
        (Key::ScrollTitle, Lang::En) => "Scrolling capture",
        (Key::ScrollTitle, Lang::Fr) => "Capture défilante",
        (Key::ScrollIntro, Lang::En) => "Click Start, then scroll the content yourself.",
        (Key::ScrollIntro, Lang::Fr) => "Cliquez sur Démarrer, puis faites défiler le contenu.",
        (Key::ScrollStart, Lang::En) => "Start",
        (Key::ScrollStart, Lang::Fr) => "Démarrer",
        (Key::ScrollFinish, Lang::En) => "Finish",
        (Key::ScrollFinish, Lang::Fr) => "Terminer",
        (Key::ScrollCancel, Lang::En) => "Cancel",
        (Key::ScrollCancel, Lang::Fr) => "Annuler",
        (Key::RecWidgetPause, Lang::En) => "Pause recording",
        (Key::RecWidgetPause, Lang::Fr) => "Mettre en pause",
        (Key::RecWidgetResume, Lang::En) => "Resume recording",
        (Key::RecWidgetResume, Lang::Fr) => "Reprendre",
        (Key::RecWidgetStop, Lang::En) => "Stop recording",
        (Key::RecWidgetStop, Lang::Fr) => "Arrêter l'enregistrement",
        (Key::ScrollCapturing, Lang::En) => "Capturing… {height} px (Enter to finish)",
        (Key::ScrollCapturing, Lang::Fr) => "Capture… {height} px (Entrée pour terminer)",
        (Key::ScrollLost, Lang::En) => "Scrolled too fast: scroll back a little",
        (Key::ScrollLost, Lang::Fr) => "Défilement trop rapide : remontez un peu",
        (Key::ScrollTruncated, Lang::En) => "Height limit reached ({height} px)",
        (Key::ScrollTruncated, Lang::Fr) => "Hauteur maximale atteinte ({height} px)",
        (Key::ConvMenuLabel, Lang::En) => "Convert with Vixeeny",
        (Key::ConvMenuLabel, Lang::Fr) => "Convertir avec Vixeeny",
        (Key::ConvTitle, Lang::En) => "Convert images",
        (Key::ConvTitle, Lang::Fr) => "Convertir des images",
        (Key::ConvDropHint, Lang::En) => "Drop images or folders here",
        (Key::ConvDropHint, Lang::Fr) => "Déposez des images ou des dossiers ici",
        (Key::ConvAddFiles, Lang::En) => "Add files…",
        (Key::ConvAddFiles, Lang::Fr) => "Ajouter des fichiers…",
        (Key::ConvAddFolder, Lang::En) => "Add folder…",
        (Key::ConvAddFolder, Lang::Fr) => "Ajouter un dossier…",
        (Key::ConvClear, Lang::En) => "Clear",
        (Key::ConvClear, Lang::Fr) => "Vider",
        (Key::ConvFormat, Lang::En) => "Format",
        (Key::ConvFormat, Lang::Fr) => "Format",
        (Key::ConvQuality, Lang::En) => "Quality",
        (Key::ConvQuality, Lang::Fr) => "Qualité",
        (Key::ConvLossless, Lang::En) => "Lossless",
        (Key::ConvLossless, Lang::Fr) => "Sans perte",
        (Key::ConvExisting, Lang::En) => "If it exists",
        (Key::ConvExisting, Lang::Fr) => "Si le fichier existe",
        (Key::ConvRename, Lang::En) => "Rename",
        (Key::ConvRename, Lang::Fr) => "Renommer",
        (Key::ConvOverwrite, Lang::En) => "Overwrite",
        (Key::ConvOverwrite, Lang::Fr) => "Écraser",
        (Key::ConvSkip, Lang::En) => "Skip",
        (Key::ConvSkip, Lang::Fr) => "Ignorer",
        (Key::ConvOutput, Lang::En) => "Output folder",
        (Key::ConvOutput, Lang::Fr) => "Dossier de sortie",
        (Key::ConvSameFolder, Lang::En) => "Same folder as each image",
        (Key::ConvSameFolder, Lang::Fr) => "Même dossier que chaque image",
        (Key::ConvChoose, Lang::En) => "Choose…",
        (Key::ConvChoose, Lang::Fr) => "Choisir…",
        (Key::ConvReset, Lang::En) => "Reset",
        (Key::ConvReset, Lang::Fr) => "Réinitialiser",
        (Key::ConvConvert, Lang::En) => "Convert",
        (Key::ConvConvert, Lang::Fr) => "Convertir",
        (Key::ConvCancel, Lang::En) => "Cancel",
        (Key::ConvCancel, Lang::Fr) => "Annuler",
        (Key::ConvSummary, Lang::En) => "{count} image(s)",
        (Key::ConvSummary, Lang::Fr) => "{count} image(s)",
        (Key::ConvProgress, Lang::En) => "Converting… {done} / {total}",
        (Key::ConvProgress, Lang::Fr) => "Conversion… {done} / {total}",
        (Key::ConvDone, Lang::En) => {
            "Done: {converted} converted, {skipped} skipped, {failed} failed"
        }
        (Key::ConvDone, Lang::Fr) => {
            "Terminé : {converted} convertie(s), {skipped} ignorée(s), {failed} en échec"
        }
        (Key::ConvCancelled, Lang::En) => {
            "Cancelled: {converted} converted, {failed} failed, {cancelled} not started"
        }
        (Key::ConvCancelled, Lang::Fr) => {
            "Annulé : {converted} convertie(s), {failed} en échec, {cancelled} non traitée(s)"
        }
        (Key::ConvNothing, Lang::En) => "No supported image found",
        (Key::ConvNothing, Lang::Fr) => "Aucune image prise en charge",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(Lang::from_tag("fr"), Lang::Fr);
        assert_eq!(Lang::from_tag("fr-FR"), Lang::Fr);
        assert_eq!(Lang::from_tag("fr_CA.UTF-8"), Lang::Fr);
        assert_eq!(Lang::from_tag("en-US"), Lang::En);
        assert_eq!(Lang::from_tag("de"), Lang::En);
        assert_eq!(Lang::from_tag(""), Lang::En);
    }

    #[test]
    fn explicit_setting_wins() {
        assert_eq!(Lang::resolve("fr", Some("en-US")), Lang::Fr);
        assert_eq!(Lang::resolve("auto", Some("fr-FR")), Lang::Fr);
    }
}
