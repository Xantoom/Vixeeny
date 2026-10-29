// SPDX-License-Identifier: GPL-3.0-or-later
//! Every text the user sees, in English and French: the daemon reads this table too, without Slint or gettext.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

/// Declares every [`Key`] with its English and French text: the enum, [`Key::ALL`] and [`tr`].
macro_rules! keys {
    ($($key:ident => $en:literal, $fr:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Key {
            $($key,)*
        }

        impl Key {
            pub const ALL: &'static [Self] = &[$(Self::$key,)*];
        }

        pub fn tr(key: Key, lang: Lang) -> &'static str {
            match (key, lang) {
                $((Key::$key, Lang::En) => $en, (Key::$key, Lang::Fr) => $fr,)*
            }
        }
    };
}

keys! {
    TrayTooltip => "Vixeeny", "Vixeeny";
    TrayTooltipRecording => "Vixeeny — recording", "Vixeeny — enregistrement";
    MenuSettings => "Settings", "Paramètres";
    MenuQuit => "Quit", "Quitter";
    AppCrashed => "Vixeeny stopped unexpectedly", "Vixeeny s'est arrêté de façon inattendue";
    HotkeysUnavailable => "Some shortcuts are unavailable: see Settings → Shortcuts.",
        "Des raccourcis sont indisponibles : voir Paramètres → Raccourcis.";
    ScrollTitle => "Scrolling capture", "Capture défilante";
    ScrollIntro => "Scroll the content, then Enter or Finish.", "Faites défiler, puis Entrée ou Terminer.";
    ScrollFinish => "Finish", "Terminer";
    ScrollCancel => "Cancel", "Annuler";
    OvlImage => "Screenshot", "Capture d'image";
    OvlRegion => "Region", "Zone";
    OvlWindow => "Window", "Fenêtre";
    OvlScreen => "Screen", "Écran";
    OvlAllMonitors => "All screens", "Tous les écrans";
    OvlScrolling => "Scrolling capture", "Capture défilante";
    OvlVideo => "Video", "Vidéo";
    OvlRecord => "Record", "Enregistrer";
    OvlStopRecording => "Stop recording", "Arrêter l'enregistrement";
    OvlReplayStart => "Start replay buffer", "Activer le replay";
    OvlReplayStop => "Stop replay buffer", "Désactiver le replay";
    OvlReplaySave => "Save replay", "Sauvegarder le replay";
    OvlSettings => "Settings", "Paramètres";
    ScrollCapturing => "Capturing… {height} px (Enter to finish)", "Capture… {height} px (Entrée pour terminer)";
    ScrollTruncated => "Height limit reached ({height} px)", "Hauteur maximale atteinte ({height} px)";
    SecGeneral => "General", "Général";
    SecShortcuts => "Shortcuts", "Raccourcis";
    SecAbout => "About", "À propos";
    SetLanguage => "Language", "Langue";
    SetLangAuto => "Automatic", "Automatique";
    SetTheme => "Theme", "Thème";
    SetThemeSystem => "Follow the system", "Suivre le système";
    SetThemeLight => "Light", "Clair";
    SetThemeDark => "Dark", "Sombre";
    SetAutostart => "Start with Windows", "Démarrage avec Windows";
    SetNotifications => "Notifications", "Notifications";
    SetOverlayEdge => "Screen edge", "Bord de l'écran";
    SetEdgeLeft => "Left", "Gauche";
    SetEdgeRight => "Right", "Droite";
    SetEdgeTop => "Top", "Haut";
    SetEdgeBottom => "Bottom", "Bas";
    SetImageFormat => "Format", "Format";
    SetShowCursor => "Include the cursor", "Inclure le curseur";
    SetCopyClipboard => "Copy to the clipboard", "Copier dans le presse-papiers";
    SetAvifDepth => "Colour depth", "Profondeur de couleur";
    SetEncoder => "Encoder", "Encodeur";
    SetContainer => "Container", "Conteneur";
    SetResolution => "Resolution", "Résolution";
    SetFps => "Frame rate", "Fréquence d'images";
    SetChroma => "Chroma", "Chroma";
    SetPreset => "Profile", "Profil";
    SetSplit => "File splitting", "Découpage du fichier";
    SetSplitOff => "Never", "Jamais";
    SetSplitSize => "By size", "Par taille";
    SetSplitDuration => "By duration", "Par durée";
    SetSplitSizeMb => "Size of each part", "Taille de chaque partie";
    SetSplitMinutes => "Length of each part", "Durée de chaque partie";
    SetWidget => "Show the widget", "Afficher le widget";
    SetWidgetCorner => "Position", "Position";
    SetCornerTl => "Top left", "Haut gauche";
    SetCornerTr => "Top right", "Haut droite";
    SetCornerBl => "Bottom left", "Bas gauche";
    SetCornerBr => "Bottom right", "Bas droite";
    SetAudioRouting => "Audio tracks", "Pistes audio";
    SetRouteEach => "One track per source", "Une piste par source";
    SetRouteMix => "Everything on one track", "Tout sur une piste";
    SetRouteAdvanced => "Advanced (settings file)", "Avancé (fichier de réglages)";
    SetAudioCodec => "Codec", "Codec";
    SetAudioBitrate => "Bitrate", "Débit";
    SetAudioVbr => "Bitrate mode", "Mode de débit";
    SetAudioDenoise => "Noise reduction", "Réduction de bruit";
    SetHdrEnable => "HDR", "HDR";
    SetEncoderKind => "Encoder type", "Type d'encodeur";
    SetKindHardware => "Hardware (GPU)", "Matériel (GPU)";
    SetKindSoftware => "Software (CPU)", "Logiciel (CPU)";
    SetPresetBest => "Quality", "Qualité";
    SetPresetCustom => "Custom", "Personnalisé";
    SetDetecting => "Detecting the encoders of this PC…", "Détection des encodeurs de ce PC…";
    SetNoHardware => "No hardware encoder found: the processor encodes.",
        "Aucun encodeur matériel trouvé : le processeur encode.";
    SrcMic => "Windows default", "Par défaut (Windows)";
    SrcAbsent => "absent", "absent";
    UiPressKeys => "Press keys…", "Appuyez…";
    UiKeysHelp => "Hold the keys, then release. Esc cancels.", "Maintenez les touches, puis relâchez. Échap annule.";
    SetReplayStart => "Replay", "Replay";
    SetReplayDuration => "Duration", "Durée";
    SetTemplate => "File name", "Nom de fichier";
    SetCheckNow => "Check now", "Vérifier maintenant";
    SetProblems => "Problems with these settings", "Problèmes avec ces réglages";
    UiGithub => "GitHub page", "Page GitHub";
    UiLogs => "Open the logs folder", "Ouvrir le dossier des logs";
    ActCaptureRegion => "Capture a region", "Capturer une zone";
    ActCaptureWindow => "Capture a window", "Capturer une fenêtre";
    ActCaptureFullscreen => "Capture the screen", "Capturer l'écran";
    ActCaptureAll => "Capture all screens", "Capturer tous les écrans";
    ActCaptureScrolling => "Scrolling capture", "Capture défilante";
    ActRecordToggle => "Start / stop recording", "Démarrer / arrêter l'enregistrement";
    ActRecordPause => "Pause / resume recording", "Pause / reprise de l'enregistrement";
    ActReplayToggle => "Turn the replay on / off", "Activer / désactiver le replay";
    ActReplaySave => "Save the replay", "Sauvegarder le replay";
    ActOverlay => "Open the side bar", "Ouvrir la barre latérale";
    ActSettings => "Open the settings", "Ouvrir les paramètres";
    ShortcutConflict => "Already used for “{action}”", "Déjà utilisé pour « {action} »";
    ShortcutDuplicate => "Already in this list", "Déjà dans cette liste";
    AboutLicense => "Licence: GNU GPL v3 or later", "Licence : GNU GPL v3 ou ultérieure";
    ToastImageSaved => "Image saved", "Image enregistrée";
    ToastRecordingSaved => "Recording saved", "Enregistrement terminé";
    ToastReplaySaved => "Replay saved", "Replay enregistré";
    ToastOpenFolder => "Open folder", "Ouvrir le dossier";
    ToastOpenSettings => "Open settings", "Ouvrir les paramètres";
    ToastCaptureFailed => "Capture failed", "Échec de la capture";
    ToastRecordingFailed => "Recording failed", "Échec de l'enregistrement";
    ToastReplayFailed => "Replay not saved", "Replay non enregistré";
    ToastDiskFull => "The disk is full: free some space or pick another folder.",
        "Le disque est plein : libérez de la place ou choisissez un autre dossier.";
    ToastAccessDenied => "Access denied: pick a folder you can write to.",
        "Accès refusé : choisissez un dossier où vous pouvez écrire.";
    UpdateAvailable => "Vixeeny {version} is available. Open the settings to update.",
        "Vixeeny {version} est disponible. Ouvrez les paramètres pour mettre à jour.";
    UpdateNow => "Download and install", "Télécharger et installer";
    UpdateUpToDate => "Vixeeny {version} is up to date", "Vixeeny {version} est à jour";
    UpdateFound => "Version {version} is available", "La version {version} est disponible";
    UpdateChecking => "Checking…", "Vérification…";
    UpdateCheckFailed => "Check failed: no network, or GitHub unreachable.",
        "Vérification impossible : pas de réseau, ou GitHub injoignable.";
    UpdateInstalling => "Restarting…", "Redémarrage…";
    ToastAudioLost => "Audio source lost: its track stays silent until it returns.",
        "Source audio perdue : sa piste reste muette jusqu'à son retour.";
    SetReplayStorage => "Replay storage", "Stockage du replay";
    SetStorageRam => "Memory (RAM)", "Mémoire (RAM)";
    SetStorageDisk => "Temporary files on disk", "Fichiers temporaires sur le disque";
    UpdateDownloading => "Downloading version {version}…", "Téléchargement de la version {version}…";
    UpdateReady => "Version {version} is ready", "La version {version} est prête";
    UpdateReadyHint => "It installs at the next start of Vixeeny.",
        "Elle s'installera au prochain démarrage de Vixeeny.";
    UpdateRestart => "Restart", "Redémarrer";
    UpdateView => "View", "Voir";
    UpdateNever => "Never checked", "Jamais vérifié";
    SetAutoUpdate => "Automatic updates", "Mises à jour automatiques";
    OptAuto => "Automatic", "Automatique";
    OptFlac => "FLAC (lossless)", "FLAC (sans perte)";
    OptPcm16 => "PCM 16-bit", "PCM 16 bits";
    OptPcm24 => "PCM 24-bit", "PCM 24 bits";
    AboutTagline => "Screenshots and screen recording for Windows.", "Captures et enregistrement d'écran pour Windows.";
    UiRetry => "Try again", "Réessayer";
    UpdateCheckFailedShort => "Update check failed", "Vérification des mises à jour impossible";
    UpdateFailedShort => "Update failed", "Échec de la mise à jour";
    SetMicOn => "Record the microphone", "Enregistrer le micro";
    SetMicDevice => "Device", "Périphérique";
    SrcNoPrograms => "No program is playing sound.", "Aucun programme ne joue de son.";
    SecReplay => "Replay", "Replay";
    TipMove => "Move the zone or an annotation", "Déplacer la zone ou une annotation";
    TipPen => "Pen", "Crayon";
    TipHighlight => "Highlighter", "Surligneur";
    TipLine => "Line", "Ligne";
    TipArrow => "Arrow", "Flèche";
    TipRectangle => "Rectangle", "Rectangle";
    TipEllipse => "Ellipse", "Ellipse";
    TipText => "Text", "Texte";
    TipCounter => "Numbered marker", "Marqueur numéroté";
    TipBlur => "Blur", "Flouter";
    TipPixelate => "Pixelate", "Pixeliser";
    TipStyle => "Colour and size (wheel: size)", "Couleur et taille (molette : taille)";
    TipUndo => "Undo (Ctrl+Z)", "Annuler (Ctrl+Z)";
    TipCopy => "Copy (Ctrl+C)", "Copier (Ctrl+C)";
    TipSave => "Save (Ctrl+S) · Save as: Ctrl+Shift+S", "Enregistrer (Ctrl+S) · Enregistrer sous : Ctrl+Maj+S";
    TipClose => "Close (Esc)", "Fermer (Échap)";
    SecOverlay => "Overlay", "Overlay";
    SecImage => "Image", "Image";
    SecUpdates => "Updates", "Mises à jour";
    SetFolder => "Folder", "Dossier";
    SetSubfolder => "Subfolder per application", "Sous-dossier par application";
    UiChange => "Change", "Modifier";
    UiShow => "Open", "Ouvrir";
    UiRemove => "Remove", "Retirer";
    UiAddShortcut => "Add a shortcut", "Ajouter un raccourci";
    SetQuality => "Quality", "Qualité";
    SetPngCompression => "Compression", "Compression";
    OptPngFast => "Fast", "Rapide";
    OptPngDefault => "Normal", "Normale";
    OptPngHigh => "Maximum", "Maximale";
    SetPngOptimize => "oxipng optimisation", "Optimisation oxipng";
    OptOff => "Off", "Désactivée";
    SetLossless => "Lossless", "Sans perte";
    SetEffort => "Compression effort", "Effort de compression";
    SetAvifSpeed => "Encoding speed", "Vitesse d'encodage";
    UiNoShortcut => "None", "Aucun";
    OptRecommended => "recommended", "recommandé";
    OptVbr => "Variable bitrate", "Débit variable";
    OptCbr => "Constant bitrate", "Débit constant";
    OptStereo => "Stereo", "Stéréo";
    SetAudioChannels => "Channels", "Canaux";
    SetSurroundHint => "5.1 and 7.1 need MKV with Opus, FLAC or PCM: recorded in stereo here.",
        "Le 5.1 et le 7.1 demandent le MKV avec Opus, FLAC ou PCM : enregistré en stéréo ici.";
    UiLicenses => "Third-party licences", "Licences tierces";
    PcCpu => "Processor", "Processeur";
    PcCores => "{cores} cores, {threads} threads", "{cores} cœurs, {threads} threads";
    PcRam => "Memory (RAM)", "Mémoire vive";
    PcGpu => "Graphics card", "Carte graphique";
    PcIgpu => "Integrated graphics", "Graphiques intégrés";
    SetCapture => "Audio source", "Source audio";
    CapSystem => "All the PC sound", "Tout le son du PC";
    CapTarget => "Only the recorded program", "Seulement le programme enregistré";
    CapOutput => "One audio device", "Un appareil audio précis";
    CapPrograms => "Chosen programs", "Programmes choisis";
    CapNone => "None (microphone only)", "Aucun (micro seulement)";
    SetAudioOutput => "Audio device", "Périphérique audio";
    SrcClosed => "closed", "fermé";
    ReplayEstimate => "About {size}", "Environ {size}";
    ReplayEstimateRam => "About {size}, in RAM", "Environ {size}, en RAM";
    ReplayEstimateDisk => "About {size}, on the disk (too big for the RAM)",
        "Environ {size}, sur le disque (trop gros pour la RAM)";
    SetAspect => "Aspect ratio", "Format d'image (ratio)";
    OptAspectScreen => "The screen's ({ratio})", "Celui de l'écran ({ratio})";
    OptAspectCrop => "{ratio}, cropped in the middle", "{ratio}, recadré au centre";
    SetExpert => "Expert settings", "Paramètres experts";
    SrcOutputs => "Outputs", "Sorties";
    SrcInputs => "Inputs", "Entrées";
    PcScreenN => "Screen {n}", "Écran {n}";
    PcPrimary => "main", "principal";
    PcFree => "{free} free of {total}", "{free} libres sur {total}";
    OptGroupCompressed => "Compressed", "Compressés";
    OptGroupLossless => "Lossless, heavy", "Sans perte, lourds";
    SetMicVolume => "Volume", "Volume";
    SetCaptureVolume => "Volume", "Volume";
    SetReplayAfter => "Extend after the shortcut", "Prolonger après le raccourci";
    UiSearch => "Search a setting", "Rechercher un réglage";
    UiSearchNone => "No setting matches “{query}”.", "Aucun réglage ne correspond à « {query} ».";
    UiSearchTitle => "Search", "Recherche";
    ShortcutTaken => "Already used by another program", "Déjà utilisé par un autre logiciel";
    ShortcutWindows => "Reserved by Windows", "Réservé par Windows";
    UiReport => "Report a problem", "Signaler un problème";
    AboutReportReady => "Report ready, without personal data. Drop the selected file into the GitHub page.",
        "Rapport prêt, sans données personnelles. Glissez le fichier sélectionné dans la page GitHub.";
    SetDiskAlert => "Low disk space alert", "Alerte d'espace disque";
    WidgetFramesLost => "{n} frame(s) lost: the encoder cannot keep up",
        "{n} image(s) perdue(s) : l'encodeur ne suit pas";
    WidgetDiskLow => "Disk almost full: {size} free", "Disque presque plein : {size} libres";
    ToastMenuOpen => "Open the image", "Ouvrir l'image";
    ToastMenuReveal => "Show in File Explorer", "Afficher dans l'Explorateur de fichiers";
    ToastMenuClose => "Close", "Fermer";
    SetCornerTc => "Top centre", "Haut centre";
    SetCornerBc => "Bottom centre", "Bas centre";
    SetCornerCustom => "Custom", "Personnalisée";
    SetWidgetDisplay => "Visibility", "Visibilité";
    OptDisplayAlways => "Always", "Toujours";
    OptDisplayFade => "Fades after 3 s", "S'estompe après 3 s";
    OptDisplayAlerts => "Only when something goes wrong", "Seulement en cas de problème";
    ToastDiskLow => "Disk almost full", "Disque presque plein";
    ToastReplayDiskLow => "{size} free on the replays disk: the next replays may fail.",
        "{size} libres sur le disque des replays : les prochains peuvent échouer.";
    SetWidgetPlaceHint => "Click anywhere on the screen for a place of your own.",
        "Cliquez n'importe où sur l'écran pour une position personnalisée.";
    UpdateChecked => "Last checked: {when}", "Dernière vérification : {when}";
    PcLocalDisk => "Local Disk", "Disque local";
    ActReplayEdit => "Save and edit the replay", "Sauvegarder et éditer le replay";
    ToastReplayOn => "Replay on", "Replay activé";
    ToastReplayOff => "Replay off", "Replay désactivé";
    SetReplayNotify => "A game starts", "Lancement d'un jeu";
    OptNotifyBoth => "Always", "Toujours";
    OptNotifyOn => "When the replay is on", "Si le replay est actif";
    OptNotifyNone => "Never", "Jamais";
    HdrGames => "Games", "Jeux";
    OptGameAuto => "Detect automatically", "Détecter automatiquement";
    OptGameAlways => "Always record", "Toujours enregistrer";
    OptGameNever => "Never record", "Ne jamais enregistrer";
    CutSave => "Save", "Enregistrer";
    CutSaveAs => "Save as…", "Enregistrer sous…";
    CutCancel => "Cancel", "Annuler";
    CutLossless => "Starts on a key frame: copied as is, instant and lossless",
        "Début sur une image clé : copie directe, instantanée et sans perte";
    CutReencode => "Re-encoded: the part does not start on a key frame, or its format changes",
        "Réencodée : la partie ne commence pas sur une image clé, ou son format change";
    CutPlay => "Play (Space)", "Lecture (Espace)";
    CutPause => "Pause (Space)", "Pause (Espace)";
    CutPrevious => "Previous frame (←)", "Image précédente (←)";
    CutNext => "Next frame (→)", "Image suivante (→)";
    CutMute => "Sound (M)", "Son (M)";
    CutClose => "Close (Esc)", "Fermer (Échap)";
    TipCopyText => "Copy the text (Ctrl+Shift+C)", "Copier le texte (Ctrl+Maj+C)";
    ToastTextCopied => "Text copied", "Texte copié";
    ToastNoText => "No text found", "Aucun texte trouvé";
    ToastOcrFailed => "Text could not be read", "Lecture du texte impossible";
    ToastOcrNoLanguage =>
        "Windows has no language with text recognition: add one in Time & language → Language & region.",
        "Windows n'a aucune langue avec la reconnaissance de texte : ajoutez-en une dans Heure et langue → Langue et région.";
    ToastOcrLanguages => "Languages", "Langues";
    CutAnimated => "animated", "animé";
    ToastMenuEdit => "Edit the image", "Modifier l'image";
    SetWindowBackdrop => "Background", "Fond";
    OptBackdropNone => "None", "Aucun";
    OptBackdropBlur => "Wallpaper, blurred", "Fond d'écran, flouté";
    OptBackdropColour => "Plain colour", "Couleur unie";
    TipLive => "Show the screens live (Space)", "Voir l'écran en direct (Espace)";
    TipFreeze => "Freeze the screens (Space)", "Figer l'écran (Espace)";
    TipRedo => "Redo (Ctrl+Y)", "Rétablir (Ctrl+Y)";
    TipClear => "Clear everything", "Tout effacer";
    TipRightClick => "Right-click: ", "Clic droit : ";
    UpdateInstalled => "Installed version: {version}", "Version installée : {version}";
    GrpSideBar => "Side bar", "Barre latérale";
    GrpWidget => "Recording widget", "Widget d'enregistrement";
    GrpLocation => "Location", "Emplacement";
    GrpEncoding => "Encoding", "Encodage";
    GrpCapture => "Capture", "Capture";
    GrpPicture => "Picture", "Image";
    GrpFile => "File", "Fichier";
    GrpPcSound => "Computer sound", "Son du PC";
    GrpMic => "Microphone", "Micro";
    GrpTracks => "Tracks and format", "Pistes et format";
    GrpThisPc => "This PC", "Ce PC";
    UnitFrames => "frames", "images";
    UnitMinutes => "min", "min";
    UnitMb => "MB", "Mo";
    ShortcutModifierOnly => "Add a key to the modifiers", "Ajoutez une touche aux modificateurs";
    ShortcutUnknownKey => "Unknown key: {key}", "Touche inconnue : {key}";
    ShortcutNeedsModifier => "{key} needs CTRL, ALT, SHIFT or WIN", "{key} demande CTRL, ALT, SHIFT ou WIN";
    SetOverlayOn => "Turn on the side bar", "Activer la barre latérale";
    UiReset => "Reset", "Réinitialiser";
    UiCancel => "Cancel", "Annuler";
    UiResetPageTitle => "Reset the \"{page}\" page?", "Réinitialiser la page « {page} » ?";
    UiResetPageBody => "Its settings go back to their default values. The other pages do not change.",
        "Ses paramètres reprennent leur valeur par défaut. Les autres pages ne changent pas.";
    UiResetAll => "Restore default settings", "Restaurer les paramètres par défaut";
    UiResetAllHint => "Restores all application settings to their defaults.",
        "Restaure tous les paramètres par défaut de l’application.";
    UiResetAllTitle => "Restore default settings?", "Restaurer les paramètres par défaut ?";
    UiResetAllBody => "Every setting, on every page, goes back to its default value.",
        "Tous les paramètres, sur toutes les pages, reprennent leur valeur par défaut.";
    GrpReset => "Reset", "Réinitialisation";
    GrpWindowCapture => "Window capture", "Capture de fenêtre";
    OptBackdropGradient => "Gradient", "Dégradé";
    SetBackdropColour => "Colour", "Couleur";
    SetBackdropGradient => "Gradient", "Dégradé";
    SetPresetBalanced => "Balanced", "Équilibré";
    SetFolderUnreachable => "This folder cannot be reached: files go to the default folder.",
        "Dossier inaccessible : les fichiers vont dans le dossier par défaut.";
    SetAdmin => "Administrator rights", "Droits administrateur";
    SetAdminGrant => "Grant", "Accorder";
    SetAdminGranted => "Granted", "Accordés";
    SetAdminHint =>
        "Allows capturing all applications, including those started as administrator.",
        "Permet de capturer toutes les applications, même celles démarrées en tant qu’administrateur.";
    GrpImageSettings => "Settings", "Paramètres";
    SetCustomEncoding => "Custom encoding settings", "Paramètres d’encodage personnalisés";
    SetCustomName => "Custom file name", "Nom personnalisé";
    SetPreview => "Show preview", "Afficher un aperçu";
    SetKeepCopy => "Keep a copy", "Garder une copie";
    SetImageEncoding => "Encoding settings", "Paramètres d’encodage";
    GrpCaptureRegion => "Region capture", "Capture de zone";
    GrpCaptureFullscreen => "Screen capture", "Capture d’écran";
    GrpCaptureAllMonitors => "All screens capture", "Capture de tous les écrans";
    GrpCaptureScrolling => "Scrolling capture", "Capture défilante";
    GrpShortcutImages => "Image captures", "Captures d’image";
    GrpShortcutRecording => "Recording", "Enregistrement";
    GrpShortcutReplay => "Replay", "Replay";
    GrpShortcutOverlay => "Overlay", "Overlay";
    SetAdminDownloading => "Downloading the installer… {percent} %", "Téléchargement de l'installateur… {percent} %";
    SecNotifications => "Notifications", "Notifications";
    GrpNotifyCaptures => "Screenshots", "Captures d'écran";
    GrpNotifyVideos => "Videos", "Vidéos";
    SetNotifyImage => "Screenshot saved", "Capture enregistrée";
    SetNotifyText => "Text copied from a zone", "Texte copié depuis une zone";
    SetNotifyRecording => "Recording saved", "Enregistrement terminé";
    SetNotifyReplay => "Replay saved", "Replay enregistré";
    SetNotifyFailures => "Errors", "Erreurs";
    SetNotifyUpdates => "New version available", "Nouvelle version disponible";
    SetDiskAlertPercent => "Alert when free space is below", "Alerte espace libre sous";
    GrpReplay => "Recording", "Enregistrement";
    SetReplayUnit => "Unit", "Unité";
    OptSeconds => "Seconds", "Secondes";
    OptMinutes => "Minutes", "Minutes";
    UnitSeconds => "s", "s";
    SetReplayAfterSeconds => "Seconds after the shortcut", "Secondes après le raccourci";
    GrpReplayVideo => "Video and sound settings", "Paramètres vidéo et son";
    SetSameAsVideo => "Same settings as Recording", "Mêmes paramètres que l'enregistrement";
    GameNoneOpen => "No game is open right now", "Aucun jeu n'est ouvert actuellement";
    SetWhatsNew => "Show what's new after an update", "Afficher les nouveautés après une mise à jour";
    UpdateNotes => "Release notes", "Notes de version";
    WhatsNewTitle => "Vixeeny has been updated to {version}", "Vixeeny a été mis à jour en version {version}";
    WhatsNewLoading => "Loading the list of changes…", "Chargement de la liste des changements…";
    WhatsNewFailed => "The list of changes could not be loaded.", "La liste des changements n'a pas pu être chargée.";
    UiOk => "OK", "OK";
    SetPresetPerformance => "Performance", "Performance";
    SecRecording => "Recording", "Enregistrement";
    SetReplayNoHardware => "The replay needs a hardware encoder (graphics card or processor graphics): none was found.",
        "Le replay a besoin d'un encodeur matériel (carte graphique ou graphique du processeur) : aucun n'a été trouvé.";
    CutMinimise => "Minimise", "Réduire";
    CutOriginal => "Original", "Original";
    CutCustom => "Custom", "Personnalisé";
    CutSearch => "Search", "Rechercher";
    CutNoMatch => "No result", "Aucun résultat";
    CutMax => "max", "max";
    UnitGb => "GB", "Go";
    ToastSample => "Sample notification", "Notification d’exemple";
    ToastSampleBody => "Captures, recordings and alerts show here.",
        "Les captures, les enregistrements et les alertes s’affichent ici.";
}

impl Lang {
    /// Parses a BCP-47-ish tag (`fr`, `fr-FR`, `fr_FR.UTF-8`); unknown languages fall back to English.
    pub fn from_tag(tag: &str) -> Self {
        let primary = tag.split(['-', '_', '.', '@']).next().unwrap_or_default().to_ascii_lowercase();
        match primary.as_str() {
            "fr" => Self::Fr,
            _ => Self::En,
        }
    }

    /// Resolves the `general.language` setting. `auto` uses `os_locale`, the tag reported by the OS, when available,
    /// else the `LC_ALL` / `LC_MESSAGES` / `LANG` variables.
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

/// A size in bytes: megabytes below a gigabyte, gigabytes with one decimal above.
pub fn size_label(bytes: u64, lang: Lang) -> String {
    let fr = lang == Lang::Fr;
    if bytes < 1 << 30 {
        let mb = (bytes >> 20).max(1);
        format!("{mb} {}", if fr { "Mo" } else { "MB" })
    } else {
        let gb = format!("{:.1}", bytes as f64 / f64::from(1u32 << 30));
        let gb = if fr { gb.replace('.', ",") } else { gb };
        format!("{gb} {}", if fr { "Go" } else { "GB" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(text: &str) -> Vec<&str> {
        let mut found: Vec<&str> = text.split('{').skip(1).filter_map(|rest| rest.split('}').next()).collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn every_key_is_translated_the_same_way_in_every_language() {
        for key in Key::ALL {
            let (en, fr) = (tr(*key, Lang::En), tr(*key, Lang::Fr));
            assert!(!en.trim().is_empty(), "{key:?} has no English text");
            assert!(!fr.trim().is_empty(), "{key:?} has no French text");
            assert_eq!(placeholders(en), placeholders(fr), "{key:?}: the placeholders differ between languages");
        }
    }

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
