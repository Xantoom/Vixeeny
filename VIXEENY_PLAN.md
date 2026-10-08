# VIXEENY — Cahier des charges et plan de développement complet (v1.0)

> Ce document est la **source de vérité unique** du projet. Il est destiné à une IA de développement (agent de code) qui doit construire Vixeeny 1.0 de A à Z, et aux personnes qui reprendront le projet (forks).
> Toute décision qui n'est pas écrite ici doit être documentée dans la section 14 (« Journal des décisions ») avant d'être appliquée.

---

## Sommaire

0. Règles pour l'agent IA
1. Vision et contraintes non négociables
2. Plateformes, matériel de référence et priorités
3. Décisions techniques figées
4. Architecture
5. Spécifications fonctionnelles
6. Registre des codecs (conçu pour l'ajout futur d'AV2, x266, etc.)
7. Exigences non fonctionnelles chiffrées
8. Limitations connues par OS
9. Build, dépendances natives, licences
10. Packaging, distribution et mises à jour
11. Feuille de route par jalons
12. Plan de tests
13. Annexes (config par défaut, matrices de compatibilité)
14. Journal des décisions

---

## 0. Règles pour l'agent IA

### 0.1 Méthode de travail

1. Travailler **jalon par jalon** (section 11), dans l'ordre. Ne jamais commencer un jalon tant que la « Définition de terminé » du précédent n'est pas remplie.
2. Chaque jalon se termine par : le code compile sans avertissement (`cargo clippy --all-targets -- -D warnings`), `cargo fmt --check` passe, tous les tests passent, la CI est verte sur les OS concernés.
3. Quand un jalon contient un point **🧪 TEST MANUEL**, s'arrêter et demander au mainteneur de faire le test, avec une procédure pas à pas. Ne pas déclarer le jalon terminé sans son retour.
4. Avant d'ajouter une dépendance (crate ou bibliothèque native), **vérifier qu'elle existe**, qu'elle est maintenue (commit ou release depuis moins de 12 mois), et que sa licence est compatible GPL-3.0. Ne jamais inventer le nom ou l'API d'une crate. En cas de doute, lire la documentation sur docs.rs ou le dépôt.
5. Au jalon M0, relever la **dernière version stable** de chaque dépendance listée en section 3 et l'épingler. Les versions citées dans ce document sont indicatives à la date de rédaction (octobre 2026).
6. Toute déviation par rapport à ce document est notée dans la section 14 avec la raison.
7. En cas de blocage technique réel (API OS qui ne permet pas une fonctionnalité), ne pas contourner par une méthode interdite (0.2) : documenter la limitation dans la section 8, implémenter le meilleur comportement dégradé possible, et le signaler.

### 0.2 Interdits absolus

- Aucune injection de DLL, hook graphique ou lecture de la mémoire d'un autre processus (risque de bannissement par les anti-cheats).
- Aucune boucle de polling dans le démon (`loop { sleep() }`, timers répétitifs à haute fréquence). Le démon dort sur des événements OS.
- Aucune webview (Tauri, Electron, WebView2, WKWebView) pour l'interface.
- Aucune télémétrie, aucune connexion réseau à part la vérification de mise à jour (désactivable).
- Aucun envoi en ligne des captures.
- Aucune dépendance incompatible GPL-3.0 (ex. : fdk-aac, `--enable-nonfree` de FFmpeg).
- Pas de `unsafe` sans un commentaire `// SAFETY:` qui justifie l'invariant.
- Pas de `unwrap()` / `expect()` hors tests et hors invariants prouvés (commentés).

### 0.3 Conventions

- Rust stable, édition 2024, `rust-toolchain.toml` épinglé.
- Code et commentaires en anglais ; interface traduite (FR + EN au minimum).
- Erreurs : `thiserror` dans les bibliothèques, `anyhow` uniquement dans les binaires.
- Logs : `tracing`, fichiers tournants dans le dossier de logs de l'OS, niveau `info` par défaut.
- Chaque crate a un `README.md` court qui explique son rôle et ses points d'entrée.
- Commits au format Conventional Commits (`feat:`, `fix:`, `docs:`…).

---

## 1. Vision et contraintes non négociables

Vixeeny est une application de capture d'écran (images et vidéos) **open-source, 100 % gratuite, sous licence GPL-3.0-or-later**, pour Windows, macOS et Linux.

Objectif de cycle de vie : publier une 1.0 quasi définitive. Après la 1.0, les seules évolutions prévues sont l'ajout de nouveaux codecs (AV2, H.266/x266, nouveaux encodeurs matériels de nouvelles générations de GPU). La maintenance au-delà est laissée aux forks. Le code doit donc être **simple à reprendre** : architecture documentée, build reproductible en une commande, tests.

Contraintes non négociables :

1. Écrite en Rust.
2. Installée une fois, tourne en arrière-plan, démarre automatiquement avec l'OS (désactivable).
3. **0,0 % de CPU au repos** et empreinte RAM minimale (chiffres en section 7).
4. Capture plein écran, zone, fenêtre/application, compatible avec toutes les API graphiques (DirectX 9 à 12, Vulkan, OpenGL, Metal), sans injection.
5. Choix complet des formats, codecs, conteneurs et réglages par l'utilisateur.
6. Interface moderne, épurée, animations fluides.

---

## 2. Plateformes, matériel de référence et priorités

### 2.1 Priorités

1. **Windows : priorité absolue.** C'est la plateforme du mainteneur. La 1.0 ne sort pas si une fonctionnalité Windows est cassée.
2. macOS et Linux : livrés dans la 1.0, testés par des volontaires. Une 0.9 publique sert de bêta pour recueillir leurs retours avant la 1.0.

### 2.2 Versions minimales

| OS | Minimum | Notes |
|---|---|---|
| Windows | Windows 10 22H2 (x64) et Windows 11 | Certaines options exigent Windows 11 (voir section 8). ARM64 non visé en 1.0. |
| macOS | macOS 13 Ventura | Apple Silicon et Intel (binaire universel). |
| Linux | Noyau et PipeWire récents (PipeWire ≥ 0.3.x épinglé au M0) | Wayland : GNOME, KDE Plasma, compositeurs wlroots (Hyprland, Sway). X11 aussi. |

### 2.3 Machine de référence (tests du mainteneur)

- Windows 11 Pro 26H2
- AMD Ryzen 7 9800X3D (iGPU désactivé, ne sera pas activé)
- NVIDIA RTX 5080 (Blackwell)
- Écran 4K QD-OLED 240 Hz (HDR)
- 64 Go DDR5

Conséquences : NVENC (H.264, HEVC, AV1, 8 et 10 bits, 4:2:0 et 4:2:2 selon le codec) est testé en conditions réelles. AMF (AMD) et QSV (Intel), le multi-GPU et Linux/macOS sont testés par des volontaires. Le code multi-GPU doit être testable sans matériel grâce à des « faux adaptateurs » (section 12).

---

## 3. Décisions techniques figées

Ces choix sont **définitifs**. Ne pas proposer d'alternative.

| Domaine | Choix | Raison |
|---|---|---|
| Licence | GPL-3.0-or-later | Obligatoire avec x264/x265 ; impose aux forks de rester libres. |
| Interface | **Slint** (rendu Skia ou FemtoVG, au choix du M0 selon la consommation mesurée) | Natif, léger, animations GPU, s'endort sans animation, traductions intégrées. |
| Vidéo, audio, muxing | **FFmpeg** (branche stable la plus récente au M0, 9.0.x à la date de rédaction) via `ffmpeg-sys-next` / `ffmpeg-next`, lié statiquement | Fournit tous les encodeurs logiciels et matériels et les conteneurs. |
| AV1 logiciel | **SVT-AV1** officiel (`AOMediaCodec/SVT-AV1`, commit épinglé) | Décision du mainteneur (2026-10-01) : version officielle plutôt que le fork Tritium. |
| H.264 / HEVC logiciel | x264, x265 (versions épinglées) | |
| VP9 | libvpx | |
| JPEG | **jpegli** (`google/jpegli`, API compatible libjpeg62), via FFI | Meilleure compression perceptuelle à compatibilité totale. |
| PNG | crate `png` + `oxipng` (optimisation optionnelle) | |
| WebP | libwebp (via `libwebp-sys`) | |
| AVIF | libavif, avec SVT-AV1 comme encodeur et dav1d comme décodeur | |
| JPEG XL | libjxl | |
| Lecture d'images (conversion) | crate `image` + libavif/libjxl/jpegli pour leurs formats | |
| Config | TOML (`serde`, `toml`), schéma versionné avec migrations | Lisible et modifiable à la main. |
| Chemins OS | crate `directories` | |
| IPC démon ↔ UI | `interprocess` (named pipe sous Windows, socket Unix ailleurs), messages `serde` + `postcard` | |
| Raccourcis globaux | crate `global-hotkey` (Windows, macOS, X11) + portail `GlobalShortcuts` via `ashpd` (Wayland) | |
| Icône système | crate `tray-icon` | |
| Démarrage auto | crate `auto-launch` (vérifier au M0 ; sinon implémentation native : clé Run, LaunchAgent, fichier `.desktop` autostart) | |
| Presse-papier | crate `arboard` | |
| Notifications | `notify-rust` (Linux/macOS) et toasts natifs Windows via le crate `windows` | |
| API Windows | crate `windows` (officiel Microsoft) | |
| API macOS | crates `objc2` et associés (`objc2-screen-capture-kit`, `objc2-vision`, `objc2-av-foundation`…) — vérifier les noms exacts au M0 | |
| Linux capture/audio | `ashpd` (portails), `pipewire` (bindings Rust), `x11rb` (X11) | |
| Traductions | système `@tr()` de Slint (gettext) | |
| Tests | `cargo test`, `insta` (snapshots), tests d'intégration avec sources de capture factices | |

---

## 4. Architecture

### 4.1 Deux processus

```
┌──────────────────────────────┐        IPC (pipe / socket)      ┌──────────────────────────────────┐
│ vixeeny-daemon               │ ◀────────────────────────────▶ │ vixeeny-app                       │
│ toujours actif, < 15 Mo      │                                 │ lancé à la demande                │
│ • icône système              │                                 │ • overlay latéral                 │
│ • raccourcis globaux         │                                 │ • éditeur Print Screen            │
│ • démarrage auto             │                                 │ • enregistrement vidéo + replay   │
│ • instance unique            │                                 │ • capture défilante               │
│ • lit la config              │                                 │ • paramètres                      │
│ • lance / réveille l'app     │                                 │ • conversion d'images             │
│ • vérif. mise à jour (1/24h) │                                 │ • FFmpeg, encodeurs, Slint        │
└──────────────────────────────┘                                 └──────────────────────────────────┘
```

Règles :

- Le démon **ne lie ni Slint ni FFmpeg ni les bibliothèques d'images**. Il ne contient que : boucle d'événements OS, raccourcis, icône système, IPC, config, lancement de processus, vérification de mise à jour.
- Le démon dort dans la boucle de messages de l'OS (`GetMessageW` sous Windows, `CFRunLoop` sous macOS, boucle `poll`/`epoll` bloquante sous Linux). La vérification de mise à jour utilise un unique timer OS toutes les 24 h (réveil négligeable).
- `vixeeny-app` est lancé quand un raccourci est pressé. Après une action terminée et **N secondes d'inactivité** (défaut 30 s, réglable, 0 = quitter immédiatement), il se ferme et libère toute sa mémoire. Pendant un enregistrement ou un replay buffer actif, il reste ouvert.
- Pour le Print Screen, le démon prend lui-même la capture d'écran brute (API OS, sans dépendance lourde) **avant** de lancer l'app, et la transmet par mémoire partagée. Ainsi l'image figée correspond exactement à l'instant de l'appui, même si l'app met du temps à démarrer.
- Si l'app plante, le démon reste vivant et notifie l'utilisateur.

### 4.2 Workspace Cargo

```
vixeeny/
├── Cargo.toml                  # workspace
├── rust-toolchain.toml
├── deny.toml                   # cargo-deny : licences et vulnérabilités
├── xtask/                      # tâches de build (cargo xtask …)
├── native/                     # scripts de build des bibliothèques C/C++ épinglées
│   ├── versions.toml           # versions et commits de toutes les libs natives
│   └── build/                  # scripts par OS
├── crates/
│   ├── vixeeny-common/         # types partagés, config, IPC, erreurs, i18n des clés
│   ├── vixeeny-daemon/         # binaire démon
│   ├── vixeeny-platform/       # abstractions OS : fenêtres, processus, écrans, DPI, notifications
│   ├── vixeeny-capture/        # capture d'images et de vidéo (trait CaptureSource)
│   ├── vixeeny-audio/          # capture audio système / par application / micro
│   ├── vixeeny-encode/         # pipeline vidéo/audio, registre de codecs, sondage GPU, muxing, replay
│   ├── vixeeny-image/          # encodage, décodage, conversion d'images, tone-mapping HDR→SDR
│   ├── vixeeny-stitch/         # assemblage de la capture défilante
│   ├── vixeeny-editor/         # modèle de l'éditeur d'annotations (logique pure, testable)
│   ├── vixeeny-ui/             # fichiers .slint et liaison Rust
│   ├── vixeeny-overlay/        # éditeur de zone natif (DirectComposition + Direct2D)
│   ├── vixeeny-app/            # binaire app
│   └── vixeeny-updater/        # vérification, téléchargement, vérification de signature, remplacement
├── assets/                     # icônes, polices, sons
├── i18n/                       # traductions (gettext .po)
├── packaging/                  # Inno Setup / WiX, dmg, Flatpak, AppImage, deb, PKGBUILD
├── docs/
│   ├── ARCHITECTURE.md         # résumé de la section 4 pour les contributeurs
│   ├── ADDING_A_CODEC.md       # procédure de la section 6.5
│   └── LIMITATIONS.md          # copie de la section 8
├── THIRD_PARTY_LICENSES.md     # généré automatiquement
└── VIXEENY_PLAN.md             # ce document
```

### 4.3 Abstractions principales (traits)

Les signatures ci-dessous fixent l'intention ; l'agent peut ajuster les détails en le notant dans la section 14.

```rust
// vixeeny-capture
pub enum CaptureTarget {
    Monitor(MonitorId),
    Region { monitor: MonitorId, rect: PhysicalRect },
    Window(WindowId),
}

pub struct CaptureOptions {
    pub target: CaptureTarget,
    pub show_cursor: bool,
    pub target_fps: Option<u32>,      // None = au rythme de l'écran
    pub hdr: HdrMode,                 // Native10Bit | ToneMapToSdr | Sdr
    pub exclude_own_windows: bool,    // toujours true en pratique
}

pub enum FramePayload {
    GpuTexture(GpuFrame),             // D3D11 / Metal (IOSurface) / DMA-BUF
    Cpu(CpuFrame),                    // repli
}

pub trait CaptureSource: Send {
    fn start(&mut self, sink: Box<dyn FrameSink>) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
    fn grab_still(&mut self) -> Result<CpuFrame, CaptureError>;  // une seule image
}

pub trait FrameSink: Send {
    fn on_frame(&mut self, frame: FramePayload, pts: Timestamp);
}

// vixeeny-audio
pub enum AudioSourceKind {
    SystemMix,                         // tout le son système
    Application { pid: u32, name: String },
    Microphone(DeviceId),
}

pub trait AudioSource: Send {
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<(), AudioError>;
}

// vixeeny-encode
pub trait VideoEncoderBackend { /* ouvert depuis une entrée du registre (section 6) */ }

// vixeeny-ocr
pub trait OcrEngine {
    fn available_languages(&self) -> Vec<LanguageTag>;
    fn recognize(&self, image: &CpuFrame, langs: &[LanguageTag]) -> Result<OcrResult, OcrError>;
}
```

Chaque trait a une implémentation par OS (`#[cfg(target_os = ...)]`) **et** une implémentation factice (`Fake…`) pour les tests.

### 4.4 Pipeline vidéo

```
Capture (texture GPU)
  → conversion couleur sur GPU (shader de calcul) : BGRA/RGBA16F → NV12 / P010 / formats 4:2:2 / 4:4:4
  → mise à l'échelle GPU (si résolution de sortie ≠ source)
  → encodeur matériel (zero-copy via hwframes FFmpeg : d3d11 → NVENC/AMF/QSV ; IOSurface → VideoToolbox ; DMA-BUF → VAAPI/Vulkan)
     OU téléchargement vers la RAM → encodeur logiciel (x264, x265, libvpx, SVT-AV1)
  → paquets encodés
  → [replay buffer en anneau] et/ou [muxer fichier]
Audio (N sources) → rééchantillonnage 48 kHz → mixage selon le routage des pistes → encodeur audio → muxer
```

- Une **horloge maîtresse** unique (horloge monotone de l'OS) date toutes les images et tous les échantillons audio. La synchro A/V doit rester inférieure à 20 ms sur 1 h.
- Les images capturées en retard sont dupliquées, celles en avance sont abandonnées, pour respecter un framerate constant (CFR) par défaut. Option VFR pour MKV.
- La pause arrête l'envoi au muxer et décale les horodatages à la reprise (pas de trou dans le fichier).

### 4.5 Config

- Fichier `config.toml` dans le dossier de config de l'OS (`directories::ProjectDirs`).
- Champ `schema_version`. Migrations automatiques de chaque version vers la suivante, testées.
- Le démon et l'app relisent la config sur notification IPC (`ConfigChanged`) quand l'UI l'enregistre.
- Exemple complet en annexe 13.1.

### 4.6 Protocole IPC (extrait)

```rust
enum DaemonToApp {
    RunAction { action: ActionId, frozen_frame: Option<SharedMemHandle> },
    ConfigChanged,
    Shutdown,
}
enum AppToDaemon {
    Ready,
    RecordingStateChanged(RecState),     // pour l'icône système (point rouge)
    RequestRestartForUpdate,
    Idle,                                  // l'app va se fermer
}
```

---

## 5. Spécifications fonctionnelles

Chaque fonctionnalité a des **critères d'acceptation** (CA). Une fonctionnalité n'est terminée que si tous ses CA sont vérifiés.

### 5.1 Actions et raccourcis

Liste des actions assignables :

| ID | Action | Raccourci par défaut |
|---|---|---|
| `capture_region` | Éditeur Print Screen (zone + annotations) | `Print Screen` |
| `capture_window` | Capture de la fenêtre active (directement en fichier) | `Alt+Print Screen` |
| `capture_fullscreen` | Capture de l'écran sous la souris (directement en fichier) | `Shift+Print Screen` |
| `capture_all_monitors` | Capture de tous les écrans assemblés | aucun |
| `capture_scrolling` | Capture défilante | aucun |
| `ocr_region` | Sélection d'une zone → OCR | aucun |
| `record_toggle` | Démarrer / arrêter l'enregistrement vidéo | `Ctrl+Shift+R` |
| `record_pause` | Pause / reprise | `Ctrl+Shift+P` |
| `replay_toggle` | Activer / désactiver le replay buffer | aucun |
| `replay_save` | Sauvegarder le replay | `Ctrl+Shift+S` |
| `overlay_toggle` | Afficher / masquer l'overlay latéral | `Ctrl+Shift+O` |
| `open_settings` | Ouvrir les paramètres | aucun |

Règles :

- **3 raccourcis maximum par action.**
- Saisie d'un raccourci : clic sur le champ, appui sur la combinaison, enregistrement. `Échap` annule, `Retour arrière` efface.
- Détection de conflit : avec une autre action de Vixeeny (bloquant) et, quand l'OS le permet, avec un raccourci déjà réservé (avertissement).
- Sous Wayland, les raccourcis passent par le portail `GlobalShortcuts` : l'utilisateur confirme dans la boîte de dialogue du système (voir section 8).

CA :
- CA-HK-1 : chaque action accepte 0 à 3 raccourcis et refuse le 4e.
- CA-HK-2 : un raccourci fonctionne quand une application plein écran (jeu) a le focus, sous Windows.
- CA-HK-3 : appuyer sur un raccourci au repos ne crée aucune activité CPU mesurable en dehors du traitement de l'action.

### 5.2 Capture d'images

Modes : écran entier (écran sous la souris), tous les écrans, zone, fenêtre/application.

Options (dans les paramètres) :
- Afficher le curseur : oui/non (défaut : non pour les images).
- Format de sortie par défaut et réglages par format (5.4).
- HDR : si l'écran est en HDR, `Convertir en SDR` (défaut) ou `Conserver le HDR` (seulement pour AVIF et JPEG XL ; les autres formats sont toujours convertis).
- Son de capture : oui/non.
- Après la capture : copier dans le presse-papier (oui/non), notification avec aperçu (oui/non), ouvrir l'éditeur (pour les captures directes).

CA :
- CA-IMG-1 : une capture plein écran en 4K est écrite sur le disque en moins de 300 ms (PNG optimisé exclu).
- CA-IMG-2 : la capture ne contient aucune fenêtre de Vixeeny.
- CA-IMG-3 : multi-écrans avec DPI différents : les coordonnées et dimensions sont correctes (pixels physiques).
- CA-IMG-4 : la conversion HDR→SDR ne produit ni image délavée ni écrêtage visible sur l'écran de référence (🧪 TEST MANUEL).

### 5.3 Éditeur Print Screen (type Lightshot)

Déroulement :

1. Appui sur `Print Screen` → le démon capture tous les écrans instantanément.
2. Une fenêtre plein écran sans bordure, au-dessus de tout, affiche l'image figée sur chaque écran avec un voile sombre (opacité 40 %, réglable).
3. L'utilisateur trace une zone à la souris. Pendant le tracé : dimensions en pixels affichées, loupe avec réticule et couleur du pixel sous le curseur (hex). La zone sélectionnée est affichée sans voile.
4. Après la sélection : poignées pour redimensionner ou déplacer la zone. Une barre d'outils apparaît collée à la zone (à l'extérieur, du côté où il y a de la place).
5. Détection de fenêtres : au survol avant le tracé, la fenêtre sous le curseur est mise en surbrillance ; un clic simple la sélectionne entièrement.

Outils :

| Outil | Détails |
|---|---|
| Crayon | Trait libre lissé |
| Ligne | Maj = angles de 15° |
| Flèche | Pointe propre, épaisseur liée au trait |
| Rectangle | Contour ou plein ; Maj = carré |
| Ellipse | Contour ou plein ; Maj = cercle |
| Texte | Police, taille, couleur ; fond optionnel |
| Surligneur | Semi-transparent, mode multiplication |
| Flou | Flou gaussien sur une zone (irréversible à l'export) |
| Pixellisation | Mosaïque sur une zone (irréversible à l'export) |
| Marqueurs numérotés | Pastilles 1, 2, 3… numérotation automatique |
| Recadrage | Ajuste la zone finale |
| Pipette | Prend une couleur de l'image |

Commun : palette de couleurs (8 couleurs prédéfinies + sélecteur complet + dernières couleurs), épaisseur, annuler/rétablir illimités, sélection et déplacement d'une annotation existante, suppression.

Actions et raccourcis dans l'éditeur :

| Action | Raccourci |
|---|---|
| Copier dans le presse-papier et fermer | `Ctrl+C` (`Cmd+C` sur macOS) |
| Enregistrer dans le dossier par défaut avec le format par défaut | `Ctrl+S` |
| Enregistrer sous (choix du dossier et du format) | `Ctrl+Shift+S` |
| OCR de la zone | `Ctrl+T` |
| Annuler / Rétablir | `Ctrl+Z` / `Ctrl+Y` et `Ctrl+Shift+Z` |
| Fermer sans enregistrer | `Échap` |

CA :
- CA-ED-1 : entre l'appui sur Print Screen et l'affichage de l'image figée : moins de 150 ms sur la machine de référence, app déjà fermée (🧪 TEST MANUEL).
- CA-ED-2 : `Ctrl+C` place une image PNG (sans perte) dans le presse-papier, collable dans un navigateur, Discord, un éditeur d'images.
- CA-ED-3 : le modèle de l'éditeur (`vixeeny-editor`) est testé unitairement : chaque outil, annuler/rétablir, rendu de référence par snapshot.
- CA-ED-4 : fonctionne correctement sur plusieurs écrans avec des DPI différents, et sur un écran 240 Hz sans saccade (rendu à la fréquence de l'écran pendant les interactions, 0 image par seconde au repos).

### 5.4 Formats d'image

| Format | Bibliothèque | Réglages | Défaut |
|---|---|---|---|
| PNG | `png` + `oxipng` | Niveau de compression, optimisation oxipng (oui/non, niveau) | Compression rapide, oxipng désactivé |
| JPEG | jpegli | Qualité ou distance (butteraugli), sous-échantillonnage (4:4:4 / 4:2:0), progressif | Qualité 90, 4:4:4 |
| WebP | libwebp | Avec perte (qualité) / sans perte, effort | Sans perte |
| AVIF | libavif + SVT-AV1 | Qualité, vitesse, profondeur 8/10/12 bits, chroma 4:4:4 / 4:2:0, HDR (PQ) | Qualité 80, 10 bits, 4:4:4 |
| JPEG XL | libjxl | Distance / sans perte, effort, HDR | Sans perte, effort 7 |

Métadonnées : profil de couleur ICC incorporé (sRGB, ou BT.2100 PQ pour le HDR), aucune donnée personnelle.

CA :
- CA-FMT-1 : chaque format produit un fichier qui s'ouvre dans au moins deux visionneuses courantes de chaque OS (navigateurs Chrome et Firefox pour WebP/AVIF/JXL lorsqu'ils les supportent).
- CA-FMT-2 : tests d'aller-retour : encodage puis décodage, vérification des dimensions et, pour les formats sans perte, identité bit à bit.

### 5.5 Conversion rapide d'images

- Dans l'app : fenêtre « Convertir » où l'on glisse-dépose un ou plusieurs fichiers ou dossiers. Choix du format et des réglages, dossier de sortie (même dossier par défaut), comportement si le fichier existe (renommer / écraser / ignorer). Barre de progression. Traitement parallèle sur tous les cœurs.
- Intégration système : entrée « Convertir avec Vixeeny » dans le menu contextuel de l'explorateur (Windows : menu contextuel ; macOS : action rapide / service ; Linux : fichier `.desktop` avec `MimeType` et action pour Nautilus/Dolphin). Installable et désinstallable depuis les paramètres.
- Formats lus : PNG, JPEG, WebP, AVIF, JPEG XL, BMP, TIFF, GIF (première image).

CA :
- CA-CONV-1 : conversion de 100 PNG 4K en AVIF sans erreur, avec progression affichée et annulation possible.
- CA-CONV-2 : les profils ICC et l'orientation EXIF sont respectés.

### 5.6 OCR

> **Retiré en 0.9.11** (décision du mainteneur : inutile). Spécification gardée pour l'historique.

- Moteur : **celui de l'OS**.
  - Windows : `Windows.Media.Ocr`. Les langues disponibles dépendent des modules de langue installés dans Windows. Si une langue manque (ex. : japonais, coréen), l'app affiche comment l'installer (Paramètres Windows → Langue → ajouter la langue avec la fonctionnalité de reconnaissance optique), avec un bouton qui ouvre la page des paramètres.
  - macOS : framework Vision (`VNRecognizeTextRequest`, niveau « accurate », correction linguistique activée).
  - Linux : Tesseract (dépendance de paquet `tesseract` + packs de langues). Si absent, message expliquant la commande d'installation selon la distribution détectée.
- Toutes les langues proposées par le moteur sont utilisables. Réglage : langues prioritaires (multi-sélection) ; défaut : langue de l'interface + anglais. Option « détection automatique » lorsque le moteur la propose (Vision).
- Résultat : le texte est copié dans le presse-papier et affiché dans un petit panneau où il est modifiable, avec un bouton « Copier ». Respect des sauts de ligne et de l'ordre de lecture (y compris vertical pour le japonais quand le moteur le gère).

CA :
- CA-OCR-1 : une image de test en anglais, japonais, coréen et allemand (fournie dans `tests/fixtures/ocr/`) est reconnue avec moins de 5 % d'erreur de caractères sur Windows avec les langues installées (🧪 TEST MANUEL).
- CA-OCR-2 : si aucune langue demandée n'est installée, aucun plantage, message clair.

### 5.7 Capture défilante

Mode retenu : **l'utilisateur fait défiler lui-même**, Vixeeny capture et assemble.

1. L'utilisateur sélectionne une zone (comme l'éditeur) puis clique « Démarrer ».
2. Il fait défiler le contenu (molette, barre de défilement, clavier). Vixeeny capture la zone à intervalle régulier (≈ 15 images/s) uniquement pendant cette phase.
3. Un aperçu vertical réduit se construit en direct à côté de la zone.
4. « Terminer » (ou `Entrée`) → l'image assemblée s'ouvre dans l'éditeur d'annotations.

Algorithme d'assemblage (`vixeeny-stitch`) :
- Comparaison de chaque nouvelle image avec la précédente pour trouver le décalage vertical (corrélation sur des empreintes de lignes, puis vérification fine).
- Détection et exclusion des zones fixes (en-têtes et barres collantes) en haut et en bas.
- Défilement vers le haut ou vers le bas géré ; décalage horizontal ignoré en 1.0.
- Image finale limitée à 30 000 px de hauteur (réglable), avec avertissement.

CA :
- CA-SCR-1 : tests automatisés sur des séquences synthétiques (page générée + défilement simulé, avec en-tête fixe) : l'image assemblée est identique pixel par pixel à la page de référence.
- CA-SCR-2 : 🧪 TEST MANUEL sur une page web longue dans un navigateur et sur une conversation Discord.

### 5.8 Dossiers, noms de fichiers et dossiers par application

- Dossiers séparés pour images, vidéos et replays (défaut : `Images/Vixeeny`, `Vidéos/Vixeeny`, `Vidéos/Vixeeny/Replays`).
- Modèle de nom de fichier paramétrable. Variables : `{app}`, `{title}`, `{date}` (AAAA-MM-JJ), `{time}` (HH-MM-SS), `{ms}`, `{counter}`, `{width}`, `{height}`, `{monitor}`. Défaut : `{app}_{date}_{time}` (si l'app est inconnue ou c'est le bureau : `Vixeeny`). Exemple : `Wuthering Waves_2026-10-01_17-12-00.png`.
- Option **« Créer un sous-dossier par application »**, réglable séparément pour images, vidéos et replays.
- Résolution du nom de l'application (dans cet ordre) :
  1. Table de correspondance utilisateur (modifiable dans les paramètres : exécutable → nom affiché).
  2. Métadonnées de l'exécutable : `ProductName`, puis `FileDescription` (Windows) ; `CFBundleName` (macOS) ; champ `Name` du `.desktop` correspondant (Linux).
  3. Titre de la fenêtre, nettoyé.
  4. Nom de l'exécutable sans extension.
  - Les noms génériques (ex. : `Client-Win64-Shipping`, utilisé par de nombreux jeux Unreal Engine) sont exclus de l'étape 4 : on passe au titre de la fenêtre.
- Nettoyage : suppression des caractères interdits par le système de fichiers, espaces en bout, noms réservés Windows (`CON`, `NUL`…), longueur limitée à 100 caractères.
- Capture du bureau ou d'une zone multi-fenêtres : `{app}` = application au premier plan au moment de la capture (option) ou `Bureau`.

CA :
- CA-DIR-1 : capturer Wuthering Waves avec l'option active crée `…/Wuthering Waves/` et y place le fichier (🧪 TEST MANUEL).
- CA-DIR-2 : tests unitaires du nettoyage de noms et de l'ordre de résolution.

### 5.9 Enregistrement vidéo

Cibles : écran entier, zone, fenêtre/application (la capture de fenêtre suit la fenêtre si elle bouge ; si elle est réduite, l'image est figée sur la dernière image).

Réglages vidéo :
- Encodeur : liste issue du registre (section 6), filtrée par ce que le sondage a validé. Groupée par « Logiciel (CPU) » puis par GPU (nom du GPU).
- Conteneur : MKV, MP4 (hybride), MP4 fragmenté, WebM. Les combinaisons interdites sont grisées (annexe 13.2).
- Résolution de sortie : identique à la source, ou préréglages (2160p, 1440p, 1080p, 720p) ou personnalisée ; conservation du ratio.
- Framerate : 24, 30, 48, 60, 90, 120, 144, 165, 240 ou personnalisé ; limité à la fréquence de l'écran source.
- Profondeur : 8 ou 10 bits (selon l'encodeur).
- Sous-échantillonnage : 4:2:0, 4:2:2, 4:4:4 (selon l'encodeur).
- HDR : `Enregistrer en HDR` (10 bits, BT.2020 PQ, métadonnées HDR10 ; HEVC et AV1 seulement) ou `Convertir en SDR`. Visible seulement si l'écran est en HDR.
- Mode de réglage : **Simple** (préréglages Qualité / Équilibré / Performance / Taille réduite, définis par encodeur dans le registre) ou **Avancé** (contrôle de débit CRF/CQP/VBR/CBR selon l'encodeur, débit, preset, tune, intervalle d'images clés, B-frames, lookahead, etc., générés depuis le registre) + champ libre d'options FFmpeg supplémentaires (`clé=valeur`), validé à l'ouverture de l'encodeur.
- Curseur : visible oui/non (défaut : oui).
- Découpage automatique : désactivé / par taille (Go) / par durée (minutes). Découpe sur image clé, sans perte d'images entre deux fichiers.
- Raccourci de pause/reprise.

Conteneurs :
- **MP4 hybride** (défaut) : écrit en fragmenté pendant l'enregistrement (lisible même après un plantage), puis converti à l'arrêt en MP4 classique avec `moov` en tête (faststart), sans réencodage.
- **MP4 fragmenté** : reste fragmenté.
- **MKV** : supporte toutes les pistes audio, très robuste.
- **WebM** : VP9 ou AV1 + Opus uniquement.

Widget d'enregistrement :
- Petite pilule horizontale en haut à gauche (position réglable : 4 coins), affichable ou non (option).
- Contenu : point rouge pulsant, chronomètre `HH:MM:SS`, bouton pause/reprise, bouton stop. En pause : point orange fixe et chronomètre figé.
- Option « Masquer automatiquement après 3 s, réapparaître au survol ».
- **Toujours exclu de la capture** (Windows : `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` ; macOS : fenêtres exclues du filtre ScreenCaptureKit ; Linux : selon le compositeur, voir section 8).
- L'icône système passe en état « enregistrement » (point rouge).

CA :
- CA-REC-1 : 🧪 TEST MANUEL — 10 minutes en 4K 240 i/s, HEVC NVENC 10 bits, sur un jeu DirectX 12 et un jeu Vulkan : 0 image perdue côté encodeur (compteur affiché dans les logs), impact sur les i/s du jeu < 5 %.
- CA-REC-2 : un plantage forcé (kill du processus) pendant un enregistrement MP4 hybride laisse un fichier lisible jusqu'aux dernières secondes.
- CA-REC-3 : pause/reprise : le fichier final est continu, la durée correspond au temps hors pause (± 1 image).
- CA-REC-4 : synchro A/V < 20 ms après 1 h d'enregistrement (test automatisé avec source factice + 🧪 TEST MANUEL).
- CA-REC-5 : le widget n'apparaît jamais dans la vidéo.
- CA-REC-6 : chaque encodeur sondé avec succès produit un fichier lisible par FFmpeg et par VLC (test automatisé pour les encodeurs logiciels ; manuel pour le matériel).

### 5.10 Audio

- Sources disponibles :
  - Son système complet.
  - **Applications individuelles** (liste des applications qui émettent du son, rafraîchie à l'ouverture du menu).
  - Microphones (tous les périphériques d'entrée).
- Routage :
  - Mode **« Une source = une piste »** (défaut) : chaque source choisie a sa propre piste.
  - Mode **« Tout mixer »** : une seule piste.
  - Mode **Avancé** : l'utilisateur crée des pistes et y assigne les sources (ex. : piste 1 = micro + jeu + Spotify mixés ; piste 2 = micro seul).
  - Volume par source, et réduction de bruit simple pour le micro (option, filtre FFmpeg `afftdn`, désactivée par défaut).
- Si le conteneur ne supporte qu'une piste… ils en supportent tous plusieurs, mais **WebM** est limité par certains lecteurs : avertissement non bloquant.
- Codecs audio :

| Codec | Implémentation | Réglages | Défaut |
|---|---|---|---|
| AAC-LC | Windows : Media Foundation (`aac_mf`) ; macOS : AudioToolbox (`aac_at`) ; Linux : encodeur `aac` natif de FFmpeg | Débit ; VBR quand l'encodeur le permet, sinon CBR | **VBR ≈ 160 kb/s** (CBR 160 kb/s sur Linux) |
| Opus | libopus | Débit, VBR/CVBR/CBR | **VBR 160 kb/s** |
| FLAC | natif FFmpeg | Niveau de compression | 5 |
| PCM 16/24 bits | natif FFmpeg | Profondeur | MKV seulement |

- Codec par défaut : AAC pour MP4, Opus pour MKV et WebM.
- Fréquence : 48 kHz ; canaux : ceux de la source (stéréo par défaut, 5.1/7.1 conservés si présents en MKV).
- Métadonnées de piste : chaque piste porte le nom de ses sources (ex. : « Micro », « Wuthering Waves », « Spotify »).

CA :
- CA-AUD-1 : enregistrer micro + jeu + Spotify en 3 pistes MKV : chaque piste ne contient que sa source (🧪 TEST MANUEL).
- CA-AUD-2 : une application qui démarre ou s'arrête pendant l'enregistrement ne casse pas l'enregistrement (piste silencieuse).
- CA-AUD-3 : débranchement du micro pendant l'enregistrement : silence sur sa piste, notification, aucun plantage.

### 5.11 Replay buffer

- Activable par raccourci, depuis l'overlay ou automatiquement au démarrage (option).
- Durée réglable de 5 s à 20 min (défaut 30 s), par pas de 5 s. L'interface affiche une **estimation de la RAM nécessaire** selon le débit choisi.
- Stockage : anneau de paquets encodés en RAM, découpé sur les images clés (intervalle d'images clés forcé à 1 s maximum en mode replay). Option « stocker sur disque » pour les longues durées (fichier temporaire tournant).
- `replay_save` : écrit les N dernières secondes dans un fichier (même conteneur et mêmes pistes que l'enregistrement), sans réencodage, puis notification. Le buffer continue de tourner.
- Compatible avec un enregistrement normal simultané (une seule session d'encodage partagée).
- Réglages vidéo/audio propres au replay (par défaut : identiques à l'enregistrement).

CA :
- CA-RPL-1 : sauvegarde en moins de 2 s pour 2 min de 4K 60 i/s ; le fichier fait la durée demandée (± 1 s, alignement image clé).
- CA-RPL-2 : la RAM consommée ne dépasse pas l'estimation de plus de 10 %.
- CA-RPL-3 : deux sauvegardes consécutives rapprochées produisent deux fichiers valides.

### 5.12 Overlay latéral

- Raccourci `overlay_toggle`. Apparaît par défaut **à la verticale sur le bord droit** de l'écran sous la souris (bord réglable : gauche/droite/haut/bas).
- Animation d'entrée : glissement + fondu, courbe « spring » ou ease-out, 180–250 ms. Sortie : 150 ms. Respecte le réglage « réduire les animations » de l'OS.
- Style : fond translucide flouté quand l'OS le permet (Mica/Acrylic sous Windows 11, vibrancy sous macOS ; sinon fond opaque semi-transparent), coins arrondis, icônes nettes, thème clair/sombre suivant l'OS (ou forcé).
- Contenu :
  - Capture image : Zone / Fenêtre / Écran / Défilante / OCR.
  - Vidéo : Enregistrer (avec choix rapide de la cible) / Replay (état + sauvegarder).
  - Sélecteur rapide de profil (profils de réglages nommés, ex. « Jeu 4K HDR », « Tuto 1080p »).
  - Accès aux paramètres.
- Se ferme sur `Échap`, clic à l'extérieur, ou après le choix d'une action.
- Navigation complète au clavier (flèches, Entrée).

CA :
- CA-OVL-1 : animation fluide à 240 Hz sur la machine de référence, sans image sautée visible (🧪 TEST MANUEL).
- CA-OVL-2 : aucune consommation CPU/GPU une fois l'animation terminée et l'overlay immobile.

### 5.13 Application de paramètres

Ouverte par un clic sur l'icône système ou par `open_settings`. Pas de galerie ni de profils d'enregistrement (retirés en 0.9.11). Une page par onglet, un seul titre, sans texte d'explication superflu.

Onglets :
1. **Général** : langue, thème, démarrage avec Windows, notifications, sons, modèle de nom, nom du jeu en plein écran.
2. **Overlay** : bord de l'écran, widget d'enregistrement.
3. **Image** : dossier (choisir / ouvrir dans l'Explorateur), sous-dossier par application, format et réglages, presse-papiers, curseur, HDR.
4. **Vidéo** : dossier, sous-dossier, résolution, images par seconde, curseur, encodeur, préréglage, 10 bits, HDR, conteneur, découpage, réglages personnalisés.
5. **Son** : son du PC, micro, pistes, codec, débit ; en dernier, les programmes enregistrés à part (« Ajouter un programme » ouvre la liste des programmes ouverts).
6. **Replay** : activation au démarrage, durée, stockage, dossier.
7. **Raccourcis** : une carte par action, groupées (captures, vidéo, autres), jusqu'à 3 raccourcis chacune.
8. **Mise à jour** : état, vérification automatique, installation automatique.
9. **À propos** : version, licences, GitHub, dossier des logs, copie des infos système.

Chaque réglage modifié s'applique immédiatement (pas de bouton « Appliquer »).

### 5.14 Notifications et retour après capture

- Notification native avec miniature : clic = ouvrir le fichier ; bouton « Ouvrir le dossier ».
- Erreurs (disque plein, encodeur indisponible, permission refusée) : notification claire avec action proposée.

### 5.15 Instance unique et démarrage

- Un seul démon par session utilisateur. Lancer Vixeeny une deuxième fois ouvre les paramètres de l'instance existante.
- Premier lancement : assistant court (langue, dossiers, démarrage auto, permissions nécessaires selon l'OS, détection matérielle).

### 5.16 Internationalisation

- Langues livrées : français et anglais. Toutes les chaînes passent par `@tr()` ; aucune chaîne en dur.
- Fichiers `.po` dans `i18n/`, guide de contribution pour ajouter une langue.
- Dates et nombres au format de la langue.

---

## 6. Registre des codecs

**Objectif : ajouter un codec futur (AV2, H.266/VVC via VVenC, nouveaux encodeurs matériels) sans modifier la logique de l'application.** Toute la connaissance spécifique aux codecs est dans des données.

### 6.1 Fichier `crates/vixeeny-encode/codecs/registry.toml`

Embarqué dans le binaire (`include_str!`), une entrée par encodeur. Exemple :

```toml
[[encoder]]
id = "nvenc_hevc"
display_name = "NVIDIA NVENC HEVC"
family = "hevc"                  # h264 | hevc | av1 | vp9 | (futur) vvc | av2
kind = "hardware"                # software | hardware
vendor = "nvidia"                # none | nvidia | amd | intel | apple | vaapi | vulkan
ffmpeg_encoder = "hevc_nvenc"
platforms = ["windows", "linux"]
hw_frames = ["d3d11", "cuda"]    # chemins zero-copy acceptés
pixel_formats = [
  { depth = 8,  chroma = "420", ffmpeg = "nv12" },
  { depth = 10, chroma = "420", ffmpeg = "p010le" },
  { depth = 8,  chroma = "444", ffmpeg = "yuv444p" },
  { depth = 10, chroma = "444", ffmpeg = "yuv444p16le" },
]
hdr = true
containers = ["mkv", "mp4", "fmp4"]
rate_control = ["cqp", "vbr", "cbr", "constqp_lossless"]

[encoder.presets]                # mode Simple
quality     = { rc = "vbr", cq = 19, preset = "p6", tune = "hq",  multipass = "qres" }
balanced    = { rc = "vbr", cq = 23, preset = "p4", tune = "hq" }
performance = { rc = "vbr", cq = 25, preset = "p2", tune = "ll" }
small       = { rc = "vbr", cq = 30, preset = "p5", tune = "hq" }

[[encoder.params]]               # mode Avancé (généré automatiquement dans l'UI)
key = "preset"
ffmpeg_option = "preset"
type = "enum"
values = ["p1","p2","p3","p4","p5","p6","p7"]
default = "p4"
label = "encoder.nvenc.preset"   # clé de traduction
```

Les valeurs ci-dessus sont des exemples : **l'agent doit vérifier chaque option et valeur dans la documentation FFmpeg de la version épinglée** (`ffmpeg -h encoder=<nom>`) et générer des tests qui comparent le registre aux options réellement exposées par l'encodeur (via l'API `AVOption` de FFmpeg).

### 6.2 Encodeurs à inclure en 1.0

| Famille | Logiciel | NVIDIA | AMD | Intel | Apple | Linux générique |
|---|---|---|---|---|---|---|
| H.264 | `libx264` | `h264_nvenc` | `h264_amf` (Win) | `h264_qsv` | `h264_videotoolbox` | `h264_vaapi`, `h264_vulkan` |
| HEVC | `libx265` | `hevc_nvenc` | `hevc_amf` (Win) | `hevc_qsv` | `hevc_videotoolbox` | `hevc_vaapi`, `hevc_vulkan` |
| AV1 | `libsvtav1` | `av1_nvenc` | `av1_amf` (Win) | `av1_qsv` | — (aucun encodeur AV1 matériel Apple à la date de rédaction ; le sondage le détectera si cela change) | `av1_vaapi`, `av1_vulkan` |
| VP9 | `libvpx-vp9` | — | — | `vp9_qsv` | — | `vp9_vaapi` |

Vérifier au M0 la présence de chaque encodeur dans la version de FFmpeg épinglée (notamment les encodeurs Vulkan Video, plus récents). Sous Linux, AMF n'est pas utilisé : les GPU AMD passent par VAAPI/Vulkan.

### 6.3 Sondage matériel

- Énumération des adaptateurs : DXGI (Windows), Metal (macOS), Vulkan + DRM (Linux). Pour chaque GPU : nom, fabricant, ID, version du pilote.
- Pour chaque encodeur matériel compatible avec le fabricant : ouverture d'une **session d'essai** pour chaque format de pixel déclaré (1 image 256×256 puis 3840×2160), avec et sans HDR. Seules les combinaisons qui réussissent sont proposées dans l'UI.
- Multi-GPU : chaque encodeur est associé à son GPU ; l'UI affiche toutes les combinaisons valides. Si la capture se fait sur un GPU différent de l'encodeur, une copie inter-GPU est faite (avertissement « performance réduite »).
- Résultat mis en cache (`hw_cache.toml`) avec comme clé l'ensemble {GPU, version du pilote, version de Vixeeny}. Nouveau sondage automatique si la clé change, ou sur demande.
- Le sondage s'exécute dans un processus enfant séparé (`vixeeny-app --probe`) pour qu'un plantage du pilote ne fasse pas tomber l'app.
- Durée cible : < 5 s.

### 6.4 Validation des combinaisons

Une fonction pure `validate(profile) -> Vec<Issue>` vérifie : encodeur ↔ conteneur, format de pixel ↔ encodeur, HDR ↔ codec/profondeur, codec audio ↔ conteneur, framerate ↔ niveau du codec. Testée exhaustivement (tests de propriétés).

### 6.5 Procédure « Ajouter un codec » (copiée dans `docs/ADDING_A_CODEC.md`)

Exemple : ajouter H.266 via VVenC, ou AV2 quand il sera disponible.

1. Ajouter la bibliothèque dans `native/versions.toml` (dépôt, commit/version, licence) et son script de build. Vérifier la compatibilité de licence avec GPL-3.0.
2. Activer l'encodeur dans la configuration de build de FFmpeg (`--enable-lib…`). Mettre à jour FFmpeg si l'encodeur n'existe que dans une version plus récente.
3. Ajouter la famille (`vvc`, `av2`) dans l'énumération `CodecFamily` et dans la matrice conteneurs (annexe 13.2).
4. Ajouter l'entrée `[[encoder]]` dans `registry.toml` (formats, préréglages, paramètres).
5. Ajouter les clés de traduction.
6. Lancer `cargo xtask verify-registry` (compare le registre aux `AVOption` réelles) et `cargo test`.
7. Pour un nouvel encodeur matériel d'une nouvelle génération de GPU : souvent, seule l'étape 2 (mise à jour FFmpeg et des en-têtes du SDK) et l'ajout des formats de pixel dans le registre sont nécessaires ; le sondage les détectera automatiquement.

---

## 7. Exigences non fonctionnelles chiffrées

| ID | Exigence | Mesure |
|---|---|---|
| NF-1 | CPU du démon au repos | 0,0 % en moyenne sur 10 min (Gestionnaire des tâches / `ps`), moins de 10 réveils par minute (Windows : Process Explorer « Context Switch Delta » ≈ 0) |
| NF-2 | RAM du démon au repos | < 15 Mo d'ensemble de travail privé sous Windows (cible 8 Mo) |
| NF-3 | RAM de l'app au repos (avant fermeture) | < 120 Mo hors buffer replay |
| NF-4 | Démarrage à froid de l'app (overlay visible) | < 300 ms sur la machine de référence |
| NF-5 | Print Screen → image figée affichée | < 150 ms |
| NF-6 | Enregistrement 4K 240 i/s NVENC | 0 image perdue sur 10 min, impact jeu < 5 % |
| NF-7 | Synchro A/V | < 20 ms sur 1 h |
| NF-8 | Robustesse | aucun plantage sur 8 h d'enregistrement continu (test d'endurance) |
| NF-9 | Taille | installeur Windows < 120 Mo |
| NF-10 | Accessibilité | navigation clavier complète, contrastes WCAG AA, lecteur d'écran sur les fenêtres de paramètres (accessibilité Slint) |
| NF-11 | Fichiers | aucune perte de données si le disque se remplit : arrêt propre, fichier finalisé, notification |

Des commandes de mesure reproductibles sont fournies dans `xtask` (`cargo xtask bench-idle`, `bench-latency`).

---

## 8. Limitations connues par OS

À documenter aussi dans `docs/LIMITATIONS.md` et dans l'aide de l'app.

### Windows
- La capture par Windows Graphics Capture couvre DirectX 9 à 12, Vulkan et OpenGL en fenêtré et en plein écran sans bordure, et la plupart des jeux en plein écran grâce aux « optimisations plein écran » de Windows. Un jeu en **plein écran exclusif strict** peut ne pas être capturable par fenêtre : proposer alors la capture d'écran entier, et l'indiquer dans l'aide.
- Bordure jaune de capture : désactivable sous Windows 11 (`IsBorderRequired = false`) ; présente sous Windows 10.
- Contrôle du rythme de capture (`MinUpdateInterval`) : versions récentes de Windows 11 ; sinon, régulation côté Vixeeny.
- Capture audio par application : disponible sous Windows 11 ; à vérifier sur Windows 10 22H2 (repli : son système complet).
- OCR : dépend des langues installées dans Windows.
- Sans signature de code : avertissement SmartScreen au premier lancement.

### macOS
- Permissions obligatoires au premier usage : Enregistrement de l'écran, Microphone, Accessibilité (raccourcis globaux selon la méthode), Notifications. L'assistant guide l'utilisateur.
- L'audio système/par application passe par ScreenCaptureKit (macOS 13+). Micro via AVFoundation (ou ScreenCaptureKit selon la version).
- Sans notarisation : l'app est bloquée au premier lancement ; le README explique comment l'autoriser (clic droit → Ouvrir, ou Réglages → Confidentialité et sécurité).
- Pas d'encodeur AV1 matériel Apple à la date de rédaction.

### Linux
- **Wayland** :
  - La capture passe par le portail ScreenCast : boîte de dialogue de l'OS pour choisir l'écran ou la fenêtre (mémorisation via le jeton de restauration du portail quand c'est supporté). L'éditeur Print Screen sous Wayland utilise le portail Screenshot ou ScreenCast, ce qui peut ajouter de la latence.
  - Les raccourcis globaux passent par le portail GlobalShortcuts (KDE, GNOME récent, Hyprland). Sur un compositeur sans ce portail : instructions pour lier une commande `vixeeny ctl <action>` dans les raccourcis du compositeur.
  - Overlay et widget au-dessus de tout : via `wlr-layer-shell` (wlroots, KDE). Sous GNOME, pas de layer-shell : fenêtre normale « toujours au-dessus » quand c'est possible, comportement dégradé documenté.
  - Exclusion du widget de la capture : pas garantie selon le compositeur ; option de masquer le widget pendant l'enregistrement.
  - Détection de l'application capturée : limitée par le portail (on reçoit le flux, pas forcément l'application) ; repli sur le titre ou « Écran ».
- **X11** : capture via XComposite/XShm, raccourcis via XGrabKey, pas de restriction de permission.
- Audio par application : via les nœuds PipeWire de chaque application.
- OCR : nécessite Tesseract et ses packs de langues installés.
- Encodage matériel : VAAPI (Intel, AMD) et NVENC (NVIDIA, pilote propriétaire), Vulkan Video selon les pilotes.

---

## 9. Build, dépendances natives, licences

### 9.1 Build reproductible

- `native/versions.toml` liste chaque bibliothèque native avec version ou commit exact et somme SHA-256 de l'archive source : FFmpeg, x264, x265, libvpx, SVT-AV1-Tritium, dav1d, libopus, libwebp, libavif, libjxl, jpegli, nv-codec-headers, AMF headers, oneVPL/libvpl (QSV), libva (Linux).
- `cargo xtask build-native` télécharge, vérifie et compile tout en statique dans `target/native/<triple>/`. Le build Rust lie ces artefacts.
- La CI met en cache les artefacts natifs par hash de `versions.toml`.
- Une seule commande pour un fork : `cargo xtask dist` produit les paquets de l'OS courant.
- FFmpeg configuré avec `--enable-gpl --enable-version3`, **sans** `--enable-nonfree`, uniquement avec les composants nécessaires (encodeurs listés, décodeurs nécessaires aux miniatures et à la conversion, muxers MKV/MP4/WebM, filtres utilisés).

### 9.2 Licences

- Vixeeny : GPL-3.0-or-later (fichier `LICENSE`, en-tête SPDX dans chaque fichier source).
- `cargo-deny` en CI : refus de toute licence incompatible.
- `THIRD_PARTY_LICENSES.md` généré (crates via `cargo-about`, bibliothèques natives depuis `versions.toml`), affiché dans « À propos ».
- Les codes sources correspondants des bibliothèques natives sont accessibles (liens et archives dans les releases GitHub), conformément à la GPL.

### 9.3 CI (GitHub Actions)

- À chaque push : fmt, clippy, tests, `cargo deny`, build sur Windows x64, macOS (arm64 + x64 → universel), Linux x64.
- Tests nécessitant un GPU ou un écran : marqués `#[ignore]` en CI, exécutables localement avec `cargo xtask test-hw`.
- Sur tag `v*` : build des paquets, signature des artefacts de mise à jour (9.4), publication d'une release GitHub en brouillon.

### 9.4 Signature des mises à jour (gratuite)

Pas de signature de code payante (Authenticode, notarisation Apple). En revanche, chaque artefact de release est signé avec une clé **minisign/ed25519** gratuite gérée par le mainteneur (clé privée dans les secrets GitHub). La clé publique est intégrée au binaire. Le système de mise à jour refuse tout fichier dont la signature est invalide.

---

## 10. Packaging, distribution et mises à jour

### 10.1 Formats

| OS | Formats |
|---|---|
| Windows | Installeur `.exe` (Inno Setup ou WiX, choix au M0) par utilisateur sans droits admin + version portable `.zip` (config à côté de l'exécutable si un fichier `portable.flag` est présent) |
| macOS | `.dmg` (binaire universel) |
| Linux | Flatpak, AppImage, `.deb`, PKGBUILD pour l'AUR |

L'installeur Windows : installation par utilisateur, entrée « Programmes et fonctionnalités », désinstallation propre (option : conserver la config), démarrage auto activé par défaut, menu contextuel optionnel.

### 10.2 Mises à jour

- Le démon vérifie l'API GitHub Releases au démarrage puis toutes les 24 h (désactivable ; aucun autre trafic réseau).
- Si une nouvelle version existe : notification + bandeau dans les paramètres avec les notes de version. L'utilisateur clique « Mettre à jour ».
- Selon le format :
  - **Installeur Windows, portable Windows, `.dmg`, AppImage** : téléchargement, vérification SHA-256 + signature minisign, remplacement, **redémarrage automatique** du démon et de l'app. Si un enregistrement est en cours, la mise à jour attend la fin.
    - Windows : l'exécutable en cours est verrouillé → un petit programme de mise à jour (`vixeeny-updater.exe`) attend la fermeture des processus, remplace les fichiers, relance le démon.
    - macOS : remplacement du bundle `.app` puis relance ; retrait de l'attribut de quarantaine sur le fichier téléchargé par Vixeeny lui-même.
    - AppImage : remplacement du fichier AppImage.
  - **Flatpak, `.deb`, AUR** : simple notification « une mise à jour est disponible via votre gestionnaire de paquets ».
- Retour arrière : la version précédente est conservée jusqu'au redémarrage réussi de la nouvelle ; en cas d'échec de démarrage, restauration automatique.

---

## 11. Feuille de route par jalons

Légende : 🪟 Windows, 🍎 macOS, 🐧 Linux, 🧪 test manuel requis.

### Phase A — Fondations (toutes plateformes)

**M0 — Squelette et décisions**
- Workspace, toolchain, CI 3 OS, `cargo-deny`, licence, structure de dossiers (4.2).
- Relevé et épinglage des versions de toutes les dépendances (règle 0.1.5) ; mise à jour de la section 3 et du journal 14.
- Choix et justification : moteur de rendu Slint, installeur Windows.
- *Terminé quand* : CI verte sur 3 OS avec un binaire « hello » par crate.

**M1 — Build natif**
- `native/versions.toml`, `cargo xtask build-native` pour FFmpeg et toutes les bibliothèques, sur les 3 OS.
- Test : un programme de démonstration encode 100 images synthétiques avec chaque encodeur logiciel et chaque conteneur.
- *Terminé quand* : artefacts natifs en cache CI, tests d'encodage logiciel verts sur 3 OS.

**M2 — Démon minimal** 🪟
- Boucle d'événements, icône système (menu : Paramètres, Quitter), instance unique, démarrage auto, lecture de config, IPC, lancement de l'app.
- `cargo xtask bench-idle`.
- 🧪 Mesure NF-1 et NF-2 sur la machine de référence.
- *Terminé quand* : NF-1 et NF-2 respectées.

**M3 — Raccourcis globaux** 🪟
- 3 raccourcis par action, détection de conflits, déclenchement dans un jeu plein écran.
- 🧪 CA-HK-2.

### Phase B — Images sous Windows

**M4 — Capture d'images** 🪟
- `vixeeny-platform` (écrans, DPI, fenêtres, processus), `vixeeny-capture` Windows (WGC) pour image unique, exclusion des fenêtres de Vixeeny.
- Captures directes plein écran / fenêtre / tous écrans en PNG.
- *Terminé quand* : CA-IMG-1, CA-IMG-2, CA-IMG-3.

**M5 — Formats d'image et HDR** 
- `vixeeny-image` : PNG, JPEG (jpegli), WebP, AVIF, JPEG XL ; profils ICC ; tone-mapping HDR→SDR (algorithme documenté, ex. BT.2390 ou équivalent) ; conservation HDR pour AVIF/JXL.
- Tests d'aller-retour sur 3 OS.
- 🧪 CA-IMG-4 sur l'écran QD-OLED HDR.

**M6 — Dossiers, noms, sous-dossiers par application** 🪟
- Modèle de nom, résolution du nom d'application (5.8), table de correspondance.
- 🧪 CA-DIR-1 avec Wuthering Waves.

**M7 — Éditeur Print Screen** 🪟
- Capture par le démon + mémoire partagée, fenêtre figée multi-écrans, sélection, loupe, détection de fenêtres, `vixeeny-editor` (modèle) + UI Slint, tous les outils, presse-papier, enregistrement.
- *Terminé quand* : CA-ED-1 à CA-ED-4. 🧪

**M8 — OCR** 🪟
- `vixeeny-ocr` Windows, panneau de résultat, gestion des langues manquantes.
- 🧪 CA-OCR-1.

**M9 — Capture défilante**
- `vixeeny-stitch` (multiplateforme, testé sur séquences synthétiques), UI Windows.
- *Terminé quand* : CA-SCR-1 vert en CI ; 🧪 CA-SCR-2.

**M10 — Conversion d'images** 🪟
- Fenêtre de conversion, menu contextuel Windows.
- CA-CONV-1, CA-CONV-2.

### Phase C — Vidéo sous Windows

**M11 — Registre et sondage**
- `registry.toml` complet (6.2), `cargo xtask verify-registry`, `validate()`, sondage GPU en processus enfant, cache.
- Faux adaptateurs pour tester le multi-GPU.
- 🧪 Sondage sur la RTX 5080 : liste des encodeurs NVENC et formats détectés, à comparer à la documentation NVIDIA.

**M12 — Pipeline vidéo** 🪟
- Capture vidéo WGC, horloge maîtresse, conversion couleur et mise à l'échelle GPU, zero-copy D3D11 → NVENC/AMF/QSV, repli CPU pour les encodeurs logiciels, muxers MKV / MP4 hybride / fMP4 / WebM, pause, découpage, HDR.
- *Terminé quand* : CA-REC-2, CA-REC-3, CA-REC-6 ; 🧪 CA-REC-1.

**M13 — Audio** 🪟
- Son système, par application, micro ; routage des pistes ; encodeurs AAC (MF), Opus, FLAC, PCM.
- CA-AUD-2, CA-AUD-3 ; 🧪 CA-AUD-1 ; CA-REC-4.

**M14 — Widget d'enregistrement** 🪟
- CA-REC-5. 🧪

**M15 — Replay buffer** 🪟
- CA-RPL-1 à CA-RPL-3. 🧪

### Phase D — Interface complète

**M16 — Overlay latéral** 🪟
- CA-OVL-1, CA-OVL-2. 🧪

**M17 — Paramètres, galerie, profils, assistant de premier lancement, notifications, traductions FR/EN**
- Toutes les sections de 5.13.
- 🧪 Revue complète par le mainteneur.

**M18 — Mises à jour et packaging Windows**
- Installeur, portable, `vixeeny-updater`, signature minisign, retour arrière.
- 🧪 Mise à jour d'une version de test vers une autre avec redémarrage automatique.

**➡ Release 0.5 (Windows uniquement)** : usage quotidien par le mainteneur pendant au moins 2 semaines, correction des bugs.

### Phase E — macOS

**M19 — Plateforme et capture macOS** 🍎 : ScreenCaptureKit (image, vidéo, audio système et par application), permissions, IOSurface → VideoToolbox, exclusion des fenêtres.
**M20 — Intégration macOS** 🍎 : raccourcis, icône de barre de menus, LaunchAgent, OCR Vision, notifications, action rapide de conversion, vibrancy, `.dmg`, mise à jour.
- 🧪 Testeurs volontaires : checklist de la section 12.3.

### Phase F — Linux

**M21 — Capture et audio Linux** 🐧 : portails ScreenCast/Screenshot (Wayland), X11, PipeWire (vidéo DMA-BUF et audio par application), VAAPI / NVENC / Vulkan.
**M22 — Intégration Linux** 🐧 : raccourcis (portail + X11 + commande `vixeeny ctl`), layer-shell, autostart, Tesseract, notifications, menus contextuels, Flatpak/AppImage/deb/AUR, mise à jour.
- 🧪 Testeurs volontaires sur GNOME, KDE, Hyprland, X11.

### Phase G — Release

**M23 — Version 0.9 bêta publique** : 3 OS, appel à testeurs, collecte des retours (modèles d'issues GitHub avec infos système générées par l'app : `Aide → Copier les infos système`).
**M24 — Version 1.0** : tous les CA Windows validés ; aucun bug bloquant ouvert sur macOS/Linux ; documentation finale (README, ARCHITECTURE, ADDING_A_CODEC, LIMITATIONS, CONTRIBUTING) ; tag `v1.0.0`.

---

## 12. Plan de tests

### 12.1 Automatisés (CI, 3 OS)

- Unitaires : config et migrations, nettoyage des noms, résolution des noms d'applications (avec données factices), modèle de l'éditeur, `validate()`, assemblage défilant, tone-mapping (images de référence), raccourcis (analyse et conflits).
- Intégration : pipeline vidéo complet avec `FakeCaptureSource` (mire animée avec horodatage incrusté) et `FakeAudioSource` (bips synchronisés) → vérification par décodage FFmpeg : nombre d'images, durée, synchro A/V, pistes, métadonnées HDR.
- Encodeurs logiciels × conteneurs × profondeurs : matrice complète.
- Registre : `verify-registry`.
- Snapshots UI (rendu Slint hors écran) des écrans principaux.

### 12.2 Matériel (local, `cargo xtask test-hw`)

- Sondage réel, encodage avec chaque encodeur matériel détecté, zero-copy, capture réelle d'une fenêtre de test (DirectX 11, DirectX 12, Vulkan, OpenGL — petits programmes de test fournis dans `tests/hw-apps/`).

### 12.3 Checklist manuelle (à fournir aux testeurs, par OS)

Installation → assistant → raccourcis → Print Screen + annotations + Ctrl+C → capture fenêtre d'un jeu → sous-dossier par application → OCR multilingue → capture défilante → enregistrement 10 min avec 3 pistes audio → pause/reprise → replay → widget absent de la vidéo → overlay → conversion → mise à jour → désinstallation. Chaque étape : OK / KO + capture + logs.

### 12.4 Endurance

8 h d'enregistrement continu (NF-8) et 7 jours de démon au repos sans fuite mémoire (RAM stable à ± 1 Mo).

---

## 13. Annexes

### 13.1 Config par défaut (extrait)

```toml
schema_version = 1

[general]
language = "auto"
theme = "system"
autostart = true
app_idle_exit_seconds = 30
sounds = true
notifications = true
check_updates = true

[hotkeys]
capture_region      = ["PrintScreen"]
capture_window      = ["Alt+PrintScreen"]
capture_fullscreen  = ["Shift+PrintScreen"]
record_toggle       = ["Ctrl+Shift+R"]
record_pause        = ["Ctrl+Shift+P"]
replay_save         = ["Ctrl+Shift+S"]
overlay_toggle      = ["Ctrl+Shift+O"]

[paths]
images  = "{pictures}/Vixeeny"
videos  = "{videos}/Vixeeny"
replays = "{videos}/Vixeeny/Replays"
filename_template = "{app}_{date}_{time}"
per_app_subfolder = { images = false, videos = false, replays = false }

[image]
format = "png"
show_cursor = false
hdr = "tonemap_sdr"
copy_to_clipboard = false

[image.jpeg]
quality = 90
chroma = "444"

[image.avif]
quality = 80
depth = 10
chroma = "444"

[video]
profile = "default"

[profiles.default]
encoder = "auto"            # meilleur encodeur matériel détecté, sinon libx264
container = "mp4_hybrid"
resolution = "source"
fps = 60
depth = 8
chroma = "420"
hdr = "tonemap_sdr"
mode = "simple"
preset = "balanced"
show_cursor = true
vfr = false                 # fréquence variable (MKV / WebM seulement)
split = { mode = "off" }

[profiles.default.audio]
routing = "one_track_per_source"
sources = ["system"]
codec = "auto"              # AAC en MP4, Opus en MKV/WebM
bitrate_kbps = 160
vbr = true

[recording_widget]
enabled = true
corner = "top_left"
auto_hide = false

[replay]
enabled_on_start = false
duration_seconds = 30
storage = "ram"

[overlay]
edge = "right"

[ocr]
languages = ["auto"]
```

### 13.2 Matrice conteneur × codec

| | H.264 | HEVC | AV1 | VP9 | AAC | Opus | FLAC | PCM |
|---|---|---|---|---|---|---|---|---|
| MKV | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| MP4 hybride / fMP4 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| WebM | ❌ | ❌ | ✅ | ✅ | ❌ | ✅ | ❌ | ❌ |

HDR (PQ, HDR10) : HEVC et AV1 en 10 bits, conteneurs MKV et MP4. (VP9 HDR possible mais non proposé en 1.0.)

### 13.3 Glossaire

- **WGC** : Windows Graphics Capture, API officielle de capture d'écran et de fenêtres.
- **SCK** : ScreenCaptureKit, API de capture de macOS.
- **Zero-copy** : l'image reste en mémoire GPU de la capture jusqu'à l'encodeur.
- **fMP4** : MP4 fragmenté, lisible même s'il n'est pas finalisé.
- **MP4 hybride** : fragmenté pendant l'enregistrement, converti en MP4 classique à la fin.
- **Replay buffer** : enregistrement permanent en mémoire des dernières secondes, sauvegardées à la demande.
- **Tone-mapping** : conversion d'une image HDR en SDR en préservant l'aspect.

---

## 14. Journal des décisions

| Date | Décision | Raison |
|---|---|---|
| 2026-10-01 | Licence GPL-3.0-or-later | x264/x265 sont GPL ; forks libres |
| 2026-10-01 | Architecture démon + app séparés | RAM minimale au repos |
| 2026-10-01 | Slint pour l'UI, pas de webview | RAM, fluidité |
| 2026-10-01 | FFmpeg pour toute la vidéo et l'audio | couverture des encodeurs et conteneurs |
| 2026-10-01 | ~~SVT-AV1-Tritium~~ remplacé par SVT-AV1 officiel (voir plus bas) | |
| 2026-10-01 | OCR de l'OS (Tesseract sous Linux) | légèreté, toutes langues |
| 2026-10-01 | Capture défilante pilotée par l'utilisateur | fiabilité multiplateforme |
| 2026-10-01 | Pas de mise en ligne, pas de télémétrie | vie privée, simplicité |
| 2026-10-01 | Pas de signature de code payante ; signature minisign des mises à jour | coût |
| 2026-10-01 | Windows prioritaire ; 0.5 Windows, 0.9 bêta 3 OS, puis 1.0 | seul le mainteneur teste Windows |
| 2026-10-01 | Registre de codecs piloté par données | ajout futur d'AV2 / x266 sans changement de logique |
| 2026-10-01 | M0 : toolchain Rust 1.98.1 épinglée ; versions relevées sur crates.io : slint 1.18.1, ffmpeg-next / ffmpeg-sys-next 9.0.0, serde 1.0.229, toml 1.1.6, postcard 1.1.3, directories 6.0.0, interprocess 2.4.4, global-hotkey 0.8.0, ashpd 0.13.13, tray-icon 0.26.0, auto-launch 0.6.0, arboard 3.6.1, notify-rust 4.18.1, windows 0.62.2, png 0.18.1, oxipng 10.2.1, libwebp-sys 0.14.4, image 0.25.10, thiserror 2.0.21, anyhow 1.0.104, tracing 0.1.44, insta 1.48.0, pipewire 0.10.1, x11rb 0.14.0, objc2 0.6.4 (seules les versions du workspace sont figées dans Cargo.toml ; le reste sera épinglé à l'usage) | règle 0.1.5 |
| 2026-10-01 | M0 : `panic = "abort"` en release, lints workspace (`unwrap_used`/`expect_used` en warning, `undocumented_unsafe_blocks` en deny) | règles 0.2 |
| 2026-10-01 | M0 : rendu Slint provisoire = FemtoVG (feature `renderer-femtovg`) ; à confirmer par mesure RAM/CPU au M2/M7 (bascule vers Skia possible sans changer les `.slint`) | plus léger à compiler et à lier que Skia ; aucune mesure possible avant qu'une fenêtre existe |
| 2026-10-01 | M0 : installeur Windows = Inno Setup (portable en zip en plus) | gratuit, script texte versionnable, pas de dépendance .NET/WiX, mise à jour silencieuse simple |
| 2026-10-01 | M1 : bibliothèques natives épinglées par commit dans `native/versions.toml` (FFmpeg n9.0.2, x264 stable, x265 4.2, libvpx 1.17.0, SVT-AV1 4.2.0 officiel, dav1d 1.5.4, opus 1.6.1, libwebp 1.6.0, libavif 1.4.2, libjxl 0.12.0, jpegli HEAD) ; FLAC/AAC via les encodeurs natifs de FFmpeg, pas de bibliothèque externe | règle 0.1.5 ; évite fdk-aac (0.2) |
| 2026-10-01 | Le mainteneur demande SVT-AV1 officiel (v4.2.0) au lieu de Tritium, pour l'AV1 logiciel et AVIF. Recette : LTO désactivé (`-DSVT_AV1_LTO=OFF`), les objets LTO « slim » GCC étant illisibles par lld | décision utilisateur ; édition de liens Rust |
| 2026-10-01 | x265 compilé en 8 et 10 bits seulement (pas de 12 bits) | décision utilisateur ; HDR10 = 10 bits |
| 2026-10-01 | **Windows : FFmpeg précompilé** (BtbN/FFmpeg-Builds, `autobuild-2026-10-01-13-06`, n9.0.2+22, GPL *shared*, SHA-256 épinglé dans `native/versions.toml`) au lieu de le compiler ; DLL livrées à côté de l'exécutable. Déroge à « lié statiquement » (section 3) pour Windows ; à rouvrir avant la 1.0 (retour au statique, ou conserver). Linux/macOS : build statique inchangé | demande du mainteneur : le build MSVC statique coûtait des heures de CI ; le build précompilé contient déjà x264/x265/SVT-AV1/libvpx/dav1d/opus/NVENC/AMF/QSV |
| 2026-10-01 | M1 : les sources natives sont épinglées par **commit git** (adressage par contenu, équivalent à un SHA-256) plutôt que par somme SHA-256 d'archive ; le zip FFmpeg précompilé Windows, lui, est vérifié par SHA-256 | simplicité ; intégrité équivalente |
| 2026-10-01 | M2 : le démon n'utilise pas `@tr()` (Slint) mais une petite table `vixeeny_common::i18n` (en/fr) pour le menu et les info-bulles ; la langue vient de `general.language` ou de la locale de l'OS | le démon ne doit pas lier Slint (< 15 Mo) |
| 2026-10-01 | M2 : `ConfigChanged` ajouté aux messages app → démon (section 4.6) pour que le démon relise la config après sauvegarde | nécessaire à l'autostart et à la langue |
| 2026-10-01 | M2 : le réveil de la boucle Windows passe par `PostMessageW(WM_APP)` vers une fenêtre « message-only », traité dans la procédure de fenêtre (pas `PostThreadMessage`, perdu pendant la boucle modale du menu) | robustesse |
| 2026-10-01 | M2 : notifications = infobulle de l'icône + journal seulement (toasts au M17) ; ACL du tube nommé : tube par utilisateur (nom contenant l'utilisateur), durcissement explicite à faire avant la 1.0 | périmètre M2 |
| 2026-10-02 | M3 : raccourcis = `RegisterHotKey` via la crate `global-hotkey` 0.8.0 (sans hook ni polling) ; analyse, limite de 3 par action et conflits (premier arrivé = prioritaire, ordre de `ActionId::ALL`) dans `vixeeny_common::hotkey` (pur, testé) ; refus de l'OS = avertissement journal + infobulle. Les 12 actions de 5.1 existent dans `ActionId` et `[hotkeys]`. `RegisterHotKey` ne capte pas les jeux qui lisent le clavier en exclusif : à valider par CA-HK-2 | M3 |
| 2026-10-02 | M4 : `vixeeny-capture` expose `Capturer` (cibles Monitor/Region/Window/AllMonitors, logique testée avec `FakeBackend`) + trait `StillBackend` (WGC sur Windows) ; seul `grab` (image unique) existe, `start`/`stop`/`FrameSink` de 4.3 arrivent avec la vidéo (M12). Région = relative à l'écran, pixels physiques. Exclusion des fenêtres de Vixeeny par `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (`vixeeny_platform::exclude_from_capture`, à appeler à la création de chaque fenêtre, dès M3 UI) ; fonction PNG rapide (`vixeeny_image::png_fast`, RGB, compression rapide) pour CA-IMG-1 ; nom de fichier provisoire `Vixeeny_{date}_{time}.png` en attendant M6 ; journalisation déplacée dans `vixeeny-common` (démon et app) | M4 |
| 2026-10-02 | M5 (1re partie) : `vixeeny-image` = PNG (`png` + `oxipng`, chunk sRGB), JPEG, WebP (`libwebp-sys`, conteneur VP8X + ICCP sRGB construit à la main), tests d'aller-retour ; tone-mapping HDR→SDR pur et testé (scRGB → sRGB : normalisation sur le blanc SDR, genou à 0,8, Reinhard étendu sur l'excédent, mise à l'échelle RGB conservant la teinte, division par le canal max au lieu d'écrêter). **Dérogation provisoire** : JPEG par la crate Rust pure `jpeg-encoder` au lieu de jpegli (pas de binding FFI sûr sur 3 OS sans bindgen) ; à rouvrir (jpegli est déjà compilé par `build-native`) | M5 |
| 2026-10-02 | M5 (2e partie) : AVIF (libavif + SVT-AV1) et JPEG XL (libjxl) via FFI avec bindgen, derrière la feature `native-codecs` de `vixeeny-image` (et `vixeeny-app`), trouvés par pkg-config dans le préfixe de `build-native` ; désactivée par défaut pour que `cargo build/test` marche sans bibliothèques natives. **Constat : le SVT-AV1 officiel n'encode qu'en 4:2:0** (« Only support 420 now ») : l'AVIF demandé en 4:4:4 (défaut du plan 5.4) est donc encodé en 4:2:0 en attendant un second encodeur AV1 (libaom, ou Tritium qui gère le 4:4:4). **Décision à prendre par le mainteneur** : ajouter libaom pour le 4:4:4 (texte plus net sur captures d'écran), ou accepter 4:2:0 | M5 |
| 2026-10-02 | M5 (3e partie) : capture HDR → SDR. `vixeeny_platform::hdr_info` (DXGI `IDXGIOutput6::GetDesc1` pour détecter l'HDR G2084 et le pic de luminance, `DisplayConfigGetDeviceInfo(GET_SDR_WHITE_LEVEL)` pour le blanc SDR) ; `MonitorInfo.hdr` ; WGC en `R16G16B16A16Float` (scRGB) ; conversion par `vixeeny_image::tonemap` passée à `Capturer` sous forme de pointeur de fonction (la crate capture ne dépend pas de la crate image). Appliqué aux captures d'écran (Monitor / Region / AllMonitors) ; capture d'une fenêtre HDR et « conserver le HDR » (AVIF/JXL en PQ BT.2100) restent à faire. À valider par 🧪 CA-IMG-4 sur l'écran QD-OLED | M5 |
| 2026-10-02 | M6 : noms de fichiers et dossiers. Module pur `vixeeny_common::naming` (assainissement Windows : caractères interdits, noms réservés, 100 caractères max ; résolution de `{app}` : table utilisateur `paths.app_names` → `ProductName` → `FileDescription` → titre de fenêtre → nom de l'exe sauf s'il est générique, p. ex. `Client-Win64-Shipping` ; gabarit `{app} {title} {date} {time} {ms} {counter} {width} {height} {monitor}` ; collisions par `_2…` ou compteur). `vixeeny_platform::exe_metadata` (ressource de version Win32). `paths.use_foreground_app` : `{app}` d'une capture de bureau = application au premier plan, sinon `Vixeeny`. Sous-dossier par application selon `paths.per_app_subfolder.images`. Tests unitaires CA-DIR-2 ; reste 🧪 CA-DIR-1 (Wuthering Waves) à valider par le mainteneur | M6 |
| 2026-10-02 | M7 (1re partie) : `vixeeny-editor`, modèle pur et testé (CA-ED-3). Annotations (crayon lissé, ligne, flèche, rectangle, ellipse, texte, surligneur en mode multiplication, flou, pixellisation, marqueurs numérotés), recadrage, pipette, annuler/rétablir illimités (éditions inversibles), sélection/déplacement/suppression, Maj = angles de 15° / carré. Rendu par `tiny-skia` ; flou (3 passes de boîte ≈ gaussienne) et mosaïque appliqués à l'export sur l'image aplatie, donc irréversibles ; texte par `fontdue` avec la police **Inter** (SIL OFL 1.1) embarquée dans `assets/fonts` (licence copiée). Test de référence par instantané ASCII (échantillonnage grossier, insensible aux différences d'anti-crénelage entre plateformes). Reste pour M7 : capture par le démon + mémoire partagée, fenêtre figée multi-écrans, sélection/loupe/détection de fenêtres, UI Slint, presse-papier, enregistrement | M7 |
| 2026-10-02 | M7 (2e partie) : éditeur utilisable de bout en bout sous Windows. `vixeeny_editor::{Selection, Session}` (purs, testés) : tracé de zone, poignées et bords, déplacement, détection de fenêtre au survol/clic, placement de la barre d'outils et de la loupe (CA-ED-3). UI Slint (`vixeeny-ui`, `ui/editor.slint`) : image figée en pixels physiques (`phx`, donc exacte quel que soit le DPI), voile réglable (`[editor] dim_percent`), loupe + hex, étiquette de dimensions, barre d'outils à icônes vectorielles (13 outils, annuler/rétablir, copier, enregistrer, enregistrer sous, OCR, fermer), palette de 8 couleurs + sélecteur HSV + dernières couleurs, épaisseur unique qui dimensionne tous les outils. Tests d'interface **sans écran** avec le moteur logiciel de Slint (`VIXEENY_SCREENSHOTS=<dossier>` écrit des PNG pour contrôle visuel). `vixeeny-app` : `region.rs` (capture de tous les écrans AVANT toute fenêtre, fenêtre sans bordure sur le bureau virtuel, `Ctrl+C/S/Maj+S/T/Z/Y/Échap`), presse-papier Windows en **PNG + CF_DIBV5** (`vixeeny_platform::clipboard`, sans perte), « Enregistrer sous » via `rfd`. **Écarts au plan** : (1) l'app capture elle-même au lieu de recevoir l'image du démon par mémoire partagée (le démon reste sans D3D/WGC pour tenir NF-1) ; à rouvrir si CA-ED-1 (< 150 ms) n'est pas tenu ; (2) OCR = M8 ; (3) pas de notification ni de son de capture. Vérification Windows depuis Linux : `cargo clippy --target x86_64-pc-windows-msvc` avec un faux compilateur C. À valider par 🧪 CA-ED-1, CA-ED-2 (collage navigateur/Discord/éditeur), CA-ED-4 (multi-DPI, 240 Hz) | M7 |
| 2026-10-02 | M8 : OCR. `vixeeny-ocr` : trait `Engine`, moteur `Windows.Media.Ocr` (WinRT via `windows`, thread dédié car les appels WinRT bloquent), choix des langues pur et testé (`auto` = langue de l'interface puis anglais, correspondance par sous-étiquette principale, langues absentes signalées), **heuristique multi-langues** : chaque langue disponible lit l'image et on garde le résultat dont le texte cadre le mieux avec l'écriture de sa langue (caractères de ses écritures +1, autres lettres −½ ; égalité = ordre de priorité) — WinRT ne fournit pas de confiance ; réduction par facteur entier au-delà de `MaxImageDimension`. CA-OCR-2 : aucune langue installée → `OcrError::NoLanguage` (jamais de plantage) et fenêtre expliquant `Add-WindowsCapability … Language.OCR~~~xx-XX~0.0.1.0` + bouton vers `ms-settings:regionlanguage`. Fenêtre de résultat Slint (texte modifiable, Copier, textes FR/EN dans `vixeeny_common::i18n`), texte copié automatiquement (`clipboard::copy_text`, CF_UNICODETEXT). `Ctrl+T` dans l'éditeur et le raccourci `ocr_region` (zone → OCR dès que la zone est validée, `Session::with_auto_command`). 🧪 CA-OCR-1 (anglais/japonais/coréen/allemand, < 5 % d'erreurs) reste au mainteneur ; je n'ai pas pu fournir de fixtures `tests/fixtures/ocr/` (pas de police CJK sous Linux) — à produire avec une capture réelle | M8 |
| 2026-10-02 | M9 : capture défilante. `vixeeny-stitch` (pur, sans OS) : empreintes de lignes (FNV-1a, 2 bits de poids faible ignorés) qui votent pour le décalage vertical, vérification sur tout le recouvrement (≥ 92 % de lignes égales, recouvrement minimal), repli grossier sur la luminance pour les images bruitées, détection d'en-tête/pied collants (lignes identiques haut/bas, retirées si elles suivent aussi le décalage = marges blanches), position suivie dans l'image assemblée (descendre puis remonter n'ajoute rien), ajout en haut ou en bas, limite `[scrolling] max_height` (30 000) avec drapeau `truncated`, `Lost` récupérable. CA-SCR-1 : 11 tests sur pages synthétiques (pas de 1 à 70 px, en-tête+pied, bandes blanches, haut/bas/mixte, bruit, trames identiques, saut sans recouvrement, limite, changement de taille), image assemblée identique octet pour octet. Windows : action `capture_scrolling` → zone (même overlay, `Command::Scroll`) → fenêtre de contrôle Slint placée à côté de la zone (`scroll.slint`, aperçu vertical rafraîchi 4×/s, Démarrer / Terminer ou Entrée / Annuler ou Échap) → capture ≈ 15 i/s dans un thread → enregistrement. **Écart au plan** : l'image assemblée est enregistrée (et copiée si `copy_to_clipboard`) au lieu de s'ouvrir dans l'éditeur, dont l'overlay est un écran figé incapable d'afficher une image plus haute que l'écran ; un éditeur à défilement/zoom reste à faire. Capture par moniteur entier puis recadrage (pas encore optimisée pour 15 i/s en 4K). 🧪 CA-SCR-2 (page web longue, Discord) au mainteneur | M9 |
| 2026-10-02 | M10 : conversion d'images. `vixeeny-image::decode` : PNG, JPEG, WebP, BMP, TIFF, GIF (1re image) via `image` ; JPEG XL via `jxl-oxide` (pur Rust, sortie sRGB demandée au CMS moxcms) ; AVIF via libavif/dav1d (`native-codecs`, `irot`/`imir` appliqués). Résultat : BGRA opaque, **orientation EXIF appliquée, profil ICC converti vers sRGB** (profil illisible = pixels laissés tels quels, jamais d'échec), transparence aplatie sur blanc. Tests CA-CONV-2 : orientation EXIF 1/3/6, profil Display P3 converti / profil sRGB intact, alpha. Nouveau crate `vixeeny-convert` (sans UI) : `collect` (fichiers + dossiers récursifs), `run` parallèle sur tous les cœurs avec progression par événement et annulation, politique si le fichier existe (renommer `nom (2)` / écraser / ignorer) avec réservation `create_new` (aucune collision entre threads, **la source n'est jamais écrasée**), `Choices` (format, qualité, sans perte → `Settings`). CA-CONV-1 : test `#[ignore]` `many_4k_pngs_to_avif` (`VIXEENY_CONV_COUNT=100`, 8 images 4K en ≈ 1 s en release ici) ; la progression et l'annulation sont couvertes par des tests. Fenêtre Slint `convert.slint` (liste, glisser-déposer via l'évènement winit `DroppedFile` — feature `unstable-winit-030`, boutons Ajouter fichiers/dossier, puces de format, qualité ±5, sans perte, politique, dossier de sortie, barre de progression, Annuler) lancée par `vixeeny-app --convert <chemins…>` **sans démon**. Menu contextuel Windows : verbe par utilisateur sous `HKCU\Software\Classes\SystemFileAssociations\.ext\shell\VixeenyConvert` (`vixeeny_platform::context_menu`, pur côté données et testé), `vixeeny-app --install-context-menu` / `--uninstall-context-menu` en attendant l'app de paramètres (M16+) qui offrira l'interrupteur. **À valider 🧪** : sélection multiple dans l'Explorateur (`MultiSelectModel=Player` + `"%1"` : à confirmer, sinon une fenêtre par fichier), glisser-déposer réel, CA-CONV-1 avec 100 vrais PNG 4K | M10 |
| 2026-10-02 | M11 : registre et sondage. `vixeeny-encode::registry` : `codecs/registry.toml` embarqué (23 encodeurs de la section 6.2 : x264, x265, SVT-AV1, libvpx-vp9, NVENC/AMF/QSV/VideoToolbox/VAAPI/Vulkan ; formats de pixels, HDR, conteneurs, contrôles de débit, 4 préréglages, paramètres du mode Avancé) et ses types serde, avec contrôles structurels au chargement. **Les préréglages sont directement des options FFmpeg** (`crf`, `preset`, `rc`, `cq`…), ce qui permet de les vérifier ; les conteneurs du registre sont `mkv/mp4/fmp4/webm` (`mp4` = hybride ou classique). `validate(profil, contexte)` pur (encodeur ↔ conteneur ↔ format de pixels ↔ HDR ↔ audio ↔ cadence, sévérités Error/Warning, disponibilité d'après le sondage) testé **exhaustivement** (encodeur × conteneur × profondeur × chroma × HDR) contre les données ; `pick_auto` (meilleur matériel sondé, H.264 d'abord, sinon libx264). Sondage : trait `Prober`, session d'essai par format (256×256 puis 3840×2160, avec/sans HDR), chaque encodeur associé à son GPU par fabricant, adaptateurs logiciels (WARP) exclus, testé avec de **faux adaptateurs** (2 NVIDIA de générations différentes + iGPU Intel + WARP, absence de GPU) ; clé de cache {GPU, pilote, version de Vixeeny} → `hw_cache.toml` (invalidation testée) ; exécution dans un **processus enfant** `vixeeny-app --probe` avec délai maximal et détection de plantage (`run_child`). `FfmpegProber` (feature `ffmpeg-next`, option `gpu=N` pour NVENC). `cargo xtask verify-registry` / test `verify_registry` : chaque option, valeur d'énumération, option de préréglage et format de pixels du registre est comparé aux `AVOption` réelles (le vérificateur est lui-même testé sur des données fausses) — **OK sur les 4 encodeurs logiciels** ici ; sous Windows la CI exige que tous les encodeurs matériels existent et exposent les options déclarées. Windows : `vixeeny_platform::gpu_adapters` (DXGI : nom, vendor/device id, version du pilote) et `attach_console` ; `vixeeny-app --probe-report [--force]` affiche GPU et encodeurs/formats validés. **Limites** : VAAPI/Vulkan (surfaces GPU) et Apple ne sont sondés qu'aux M20/M22 (le sondeur répond « contexte matériel requis ») ; les options des encodeurs matériels ne sont vérifiées que par la CI Windows. 🧪 **Au mainteneur** : `vixeeny-app.exe --probe-report` sur la RTX 5080, à comparer à la documentation NVIDIA (encodeurs NVENC et formats) | M11 |
| 2026-10-02 | M12 (en cours) : enregistrement. `vixeeny-encode::clock` (CFR : répète/abandonne selon l'horloge maître) et `recorder` (conversion BT.709/2020 + échelle swscale, encodeurs ouverts depuis le registre, MKV / MP4 hybride (fragmenté puis `faststart`) / fMP4 / WebM, pause, découpage sur images clés `CLOSED_GOP`, `flush_packets=1` pour la reprise après crash) : CA-REC-2/3/6 automatisés avec encodeurs logiciels (12 tests). Étape B : `vixeeny-capture::wgc_stream` (WGC continu, horodatage `SystemRelativeTime`, recadrage, redimensionnement de fenêtre), `vixeeny_platform::monotonic_ns`, `vixeeny-app::record` (profil → encodeur `auto`/registre, nom de fichier par gabarit, `_partN`) branché sur `RecordToggle`/`RecordPause` + `RecordingStateChanged` ; non vérifiable depuis Linux (compilé par la CI Windows, étape « Recording app »). Étape C : VFR pour MKV/WebM (`profile.vfr`, horodatage en µs, pauses retranchées) ; HDR réel (`hdr.rs` : scRGB demi-flottants → BT.2020/PQ 16 bits par table, swscale → 10 bits ; WGC en `R16G16B16A16Float` ; testé par un vrai encodage HEVC) ; chemin GPU Windows (`d3d_convert` : D3D11 Video Processor BGRA/scRGB → NV12/P010 avec matrices 709 ou 2020/PQ ; `gpu` : pool de frames D3D11 FFmpeg lu directement par NVENC/AMF ; auto-test à la création et repli CPU automatique, `VIXEENY_NO_GPU=1` pour le forcer) — le chemin GPU n'est compilé que par la CI Windows et n'a pas pu être exécuté ici (pas de GPU) : à vérifier avec 🧪 CA-REC-1 ; QSV reste sur le chemin CPU. |
| 2026-10-02 | M13 : audio. `vixeeny-audio` : sources `system` / `mic[:id|nom]` / `app:<exe>` (`SourceSpec`), format unique 48 kHz stéréo f32 (Windows convertit via `AUTOCONVERTPCM`), `Mixer` piloté par le temps (blocs de 20 ms, latence 150 ms ; trous = silence, dérive d'horloge corrigée au-delà de 10 ms, pause sans trou ; données tardives ignorées) : CA-REC-4 automatisé (dérive 50 ppm sur 1 h simulée → < 20 ms ; enregistrement réel de 120 s avec dérive 100 ppm : audio et vidéo finissent ensemble à 40 ms près), routage (`one_track_per_source` / `mix_all` / `advanced` avec `[[profiles.x.audio.tracks]]` et `volumes`), source de test `FakeAudioSource`. `vixeeny-encode::audio` : AAC (`aac_mf` / `aac_at` / `aac`), Opus (VBR), FLAC (5), PCM 16/24 ; N pistes nommées dans MKV/MP4/WebM, ancrage sur la 1re image vidéo, coupure par le temps pour le découpage (l'audio d'une partie n'est écrit que quand la vidéo l'a rattrapé). Windows (`wasapi.rs`, compilé et clippy-é mais non exécuté ici) : loopback du son système, micros, loopback par processus (Windows 10 2004+, `ActivateAudioInterfaceAsync`), liste des micros et des applications ; une source perdue (micro débranché, appli fermée, changement de sortie par défaut) émet `Lost`, la piste reste silencieuse et la source est rouverte toute seule chaque seconde (CA-AUD-2/3, à vérifier à la main). Écarts : AAC en débit constant (le VBR d'`aac_mf` n'est pas vérifiable ici) ; stéréo seulement (pas de 5.1/7.1) ; réduction de bruit `afftdn` et notification visuelle de perte de micro non faites (journal seulement) ; nom de piste MP4 laissé au muxer. 🧪 CA-AUD-1 (3 pistes micro + jeu + Spotify) et CA-REC-4 (1 h réelle) à la charge du mainteneur. |
| 2026-10-02 | Décisions du mainteneur : **JPEG = jpegli uniquement, pas de repli** (la crate `jpeg-encoder` est supprimée ; JPEG exige donc la feature `native-codecs`) ; l'API C `jpegli_*` est déclarée à la main + structures libjpeg via bindgen ; la recette `build-native` installe désormais `libjpegli.a`/`jpegli.lib` et `libjpegli.pc` (non installés par défaut par jpegli). **AVIF 4:4:4** : jugé peu pertinent, on reste en 4:2:0 avec le SVT-AV1 officiel (pas de libaom) | demande utilisateur |
| 2026-10-02 | M14 : widget d'enregistrement. Pastille Slint sans bordure, toujours au-dessus, fond transparent (point rouge qui pulse, point orange fixe en pause, temps `HH:MM:SS`, pause/reprise, arrêt ; masquage automatique après 3 s hors survol si `auto_hide`), testée en rendu headless (captures `7-widget-*.png`, clics → événements). Elle tourne dans un processus à part (`vixeeny-app --widget …`, protocole en lignes sur stdin/stdout, partie pure dans `widget_math.rs` testée sous Linux : coin et marge selon le DPI, messages, horloge) pour ne pas bloquer la boucle d'actions de l'app ; la fenêtre est exclue de toute capture (`SetWindowDisplayAffinity`, CA-REC-5) et marquée `WS_EX_NOACTIVATE`/`TOOLWINDOW` (ne prend jamais le focus). Branché dans `record.rs` (coin `top_left`/`top_right`/`bottom_left`/`bottom_right` de l'écran enregistré, état envoyé au démarrage, à chaque pause/reprise et resynchronisé toutes les 5 s ; le temps ne compte pas les pauses) ; échec de lancement = journal, l'enregistrement continue. Compilé et clippy-é, non exécuté ici. 🧪 CA-REC-5 (le widget n'apparaît jamais dans la vidéo) à la charge du mainteneur. |
| 2026-10-02 | M15 : replay buffer. `vixeeny-encode::replay::Ring` : anneau de paquets encodés (partagés, sans copie) découpé par GOP, en temps média (échantillons 48 kHz) ; garde la durée demandée plus au plus un intervalle d'images clés (forcé à 1 s en mode replay) ; `estimate_ram_bytes` (débit × (durée + 1 intervalle)) — test CA-RPL-2 : consommation entre 90 % et 110 % de l'estimation pour 10 s, 30 s, 120 s ; `clamp_seconds` (5 s à 20 min, pas de 5 s). `Recorder` : `RecordConfig.replay_seconds` / `files` (session replay seule, sans fichier) et `save_replay(path)` : les paquets sont choisis sur le thread d'encodage, puis un thread dédié remuxe vidéo + pistes audio fusionnées par le temps (aucun réencodage ; MP4 hybride refait en faststart) sans interrompre l'encodage ; deux sauvegardes rapprochées = deux fichiers valides (CA-RPL-3, testé), durée mesurée à ±1 s (CA-RPL-1, partie durée, testée sur synthétique). App : `record::start_replay` (mêmes capture/GPU/audio que l'enregistrement, profil `[replay].profile` ou celui de l'enregistrement, aucun widget, pas de pause), `ReplayToggle` / `ReplaySave` dans `main.rs`, fichiers dans le dossier Replays avec le modèle de nom et le sous-dossier par application ; le daemon lance le buffer au démarrage si `[replay].enabled_on_start`. Écarts : l'enregistrement normal simultané ouvre sa propre session de capture/encodage (pas de session partagée) ; stockage disque (`storage = "disk"`) non fait (RAM seulement) ; pas de notification de fin de sauvegarde (journal) ; estimation de RAM dans l'interface avec les réglages (M17). Non exécuté sur Windows ici. 🧪 CA-RPL-1 (2 min de 4K 60 i/s en moins de 2 s), CA-RPL-2 (RAM réelle) et CA-RPL-3 en conditions réelles à la charge du mainteneur. |
| 2026-10-02 | M16 : overlay latéral. `vixeeny-ui::side_panel` + `side_panel.slint` : bande translucide sur le bord réglable (`[overlay].edge` = `left`/`right`/`top`/`bottom`, colonne ou rangée), thème clair/sombre (`general.theme`, `system` = réglage Windows), coins arrondis, icônes dessinées en formes Slint (nettes à toute échelle, pas de police d'icônes). Contenu : capture d'image (zone, fenêtre, écran, tous les écrans, défilante, OCR), vidéo (enregistrer/arrêter, replay activer/désactiver, sauvegarder le replay — grisé sans replay), sélecteur de profil (clic = profil suivant, écrit dans `config.toml` puis `ConfigChanged` au daemon), paramètres. Animation d'entrée 220 ms (courbe ease-out expo) glissant depuis le bord + fondu, sortie 150 ms ; durées à 0 si Windows a désactivé les animations (`animations_enabled`, `SPI_GETCLIENTAREAANIMATION`). Navigation clavier complète (flèches/Tab, Entrée, Échap ; les titres et entrées grisées sont sautés), fermeture au clic extérieur (perte de focus winit), après un choix ou sur Échap ; aucun timer actif une fois l'animation finie (CA-OVL-2 par construction : la boucle d'événements dort). `vixeeny-platform` : `system_prefers_dark`, `animations_enabled`, `apply_acrylic` (Windows 11 : fond acrylique `DWMWA_SYSTEMBACKDROP_TYPE` + coins ronds ; sous Windows 10 l'échec laisse le fond plus opaque). App : `side.rs`, l'action choisie est exécutée après fermeture (120 ms de répit avant une capture pour que la bande ne soit pas dans l'image figée). Correction au passage : `ReplayToggle`/`ReplaySave` n'étaient pas routées vers le replay dans la boucle de l'app (M15) — c'est fait. Testé en rendu headless (captures `8-side-*.png`, clavier, souris, cyclage du profil) ; non exécuté sur Windows. Écarts : un second `overlay_toggle` pendant que la bande est ouverte est traité après sa fermeture (la bande bloque le fil de l'app) ; le choix rapide de la cible d'enregistrement n'est pas dans la bande (l'enregistrement filme l'écran sous la souris). 🧪 CA-OVL-1 (240 Hz sans image sautée) et le rendu acrylique sont à la charge du mainteneur. |
| 2026-10-02 | M17 : paramètres, galerie, notifications, assistant. `vixeeny-settings` (pur, testé) : lignes pilotées par les données (`Row` avec lecture/écriture, `Kind` qui valide, application immédiate, remise à zéro par section), 14 sections (galerie, général, raccourcis, images, vidéo, audio, replay, dossiers, OCR, profils, matériel, mises à jour, intégration, à propos), gestion des profils (créer/dupliquer/renommer/supprimer), table des raccourcis avec conflits expliqués. `settings.slint` + `settings_panel.rs` : fenêtre à barre latérale, sans widgets std, thème clair/sombre, relibellé immédiat au changement de langue. **La fenêtre tourne dans un processus enfant** (`vixeeny-app --settings`, instance unique par mutex `Vixeeny.Settings`, la 2e instance met la 1re au premier plan) pour ne jamais retarder les raccourcis ; elle écrit `config.toml` elle-même puis envoie `ControlRequest::ReloadConfig` au démon (nouveau). Galerie : scan des dossiers images/vidéos/replays, filtres type/application, miniatures décodées sur un thread, actions ouvrir / dossier / copier / convertir / corbeille. Matériel : sondage `probe::current` sur un thread, une ligne par GPU + logiciel. Intégration : menu contextuel de l'Explorateur (installer/retirer). **Notifications (5.14)** : carte Slint `--toast` (processus enfant, miniature, clic = ouvrir le fichier, bouton « Ouvrir le dossier », auto-fermeture 6 s, coin bas-droit du moniteur principal), branchée sur capture directe, fin d'enregistrement et sauvegarde de replay ; erreurs avec conseil pour disque plein / accès refusé ; respecte `general.notifications`. **Assistant de premier lancement (5.15)** : 4 étapes (langue, dossiers, démarrage auto + notifications, matériel) ; le démon ouvre les paramètres au premier démarrage (`general.first_run_done`), le processus enfant montre l'assistant d'abord ; le fermer ou « Passer » le marque fait. i18n : table Rust `i18n.rs` (243 clés), test de complétude (`Key::ALL`, textes non vides, mêmes `{placeholders}`), guide `docs/TRANSLATING.md`. Écarts : i18n par table Rust et non `.po`/`@tr()` (le démon n'a pas Slint) ; stockage disque du replay non fait ; les sessions d'enregistrement et de replay sont séparées ; pistes audio avancées éditées dans le fichier de config ; section Mises à jour = case + version (le programme de mise à jour est M18) ; miniatures vidéo = pastille ; l'overlay latéral bloque le thread de l'app ; la page Vidéo valide avec une source 1920×1080 ; la langue de l'assistant ne retraduit pas le texte matériel déjà sondé ; les notifications ne sont pas des toasts Windows natifs (carte Slint) ; pas de son de notification ; l'assistant ne demande pas de permission (Windows n'en exige pas). Tests : 🧪 revue complète par le mainteneur (fenêtre, galerie, assistant, notifications) à faire à la main. CI verte sur les 5 parties. |
| 2026-10-02 | M18 : mises à jour et packaging Windows. `vixeeny-updater` (lib + programme) : lecture de l'API GitHub Releases (brouillons et pré-versions ignorés), comparaison semver, vérification **SHA-256 (`SHA256SUMS`) + minisign** avant toute modification (clé publique embarquée `packaging/minisign.pub`, refus tant qu'elle n'est pas configurée), extraction de l'archive avec refus des chemins sortants, remplacement des fichiers avec **journal et retour arrière** (les anciens fichiers restent dans `.update-backup` jusqu'à ce que le nouveau démon réponde, restauration automatique sinon ; les fichiers utilisateur ne sont jamais touchés), états dans `update.json`. `vixeeny-updater check` écrit l'état et affiche `new <version>` une seule fois ; `apply` télécharge, vérifie, demande l'arrêt du démon (`ControlRequest::QuitForUpdate`, **ignorée pendant un enregistrement** : l'updater redemande jusqu'à la fin), remplace, relance et attend la réponse du démon (20 s). Le démon ne lie pas le client réseau : un thread lance `check` 90 s après le démarrage puis toutes les 24 h (si `check_updates`) et notifie via la zone de notification. Page Mises à jour : version, notes, boutons « Vérifier » / « Mettre à jour ». Mode portable : `portable.flag` à côté de l'exécutable → config et journaux dans `data\`. Packaging : `cargo xtask dist` (staging, archive de mise à jour `Vixeeny-<v>-windows-x64.zip`, archive portable `…-portable.zip`, `THIRD-PARTY-LICENSES.txt` généré depuis `cargo metadata` + `versions.toml`), `cargo xtask sums`, script Inno Setup `packaging/windows/vixeeny.iss` (par utilisateur, sans droits admin, français/anglais, tâches menu Démarrer / démarrage auto / menu contextuel, désinstallation avec question sur la config), `.github/workflows/release.yml` (tag `v*` : build, installeur, sommes, signature minisign si le secret existe, release GitHub en brouillon ; tag `v*-test` : mêmes fichiers sans brouillon). Essai réel sur le tag `v0.0.1-test` : installeur 62 Mo (NF-9 < 120 Mo : OK), archives 93 Mo. Corrigé au passage : `vixeeny-image` avec `native-codecs` ne compilait pas sous Windows (constantes bindgen `i32`), ce que le pas CI `continue-on-error` masquait ; licence CDLA-Permissive-2.0 (webpki-roots) autorisée dans `deny.toml`. Écarts : l'installeur et l'archive de mise à jour ont le même contenu (la mise à jour remplace les fichiers, elle ne relance pas l'installeur : l'entrée « Programmes et fonctionnalités » garde l'ancienne version affichée) ; pas de notification cliquable ; l'updater n'attend pas la fermeture de l'app/paramètres en cours (le renommage d'un exécutable en cours est permis sous Windows) ; Flatpak/deb/AUR/dmg/AppImage = M20/M22. **À faire par le mainteneur** : générer la paire de clés (`packaging/README.md`), commiter la clé publique, ajouter les secrets `MINISIGN_KEY` et `MINISIGN_PASSWORD`. 🧪 Mise à jour d'une version de test vers une autre avec redémarrage automatique : à faire à la main (publier deux versions signées, installer la première, cliquer « Mettre à jour »). |
| 2026-10-03 | **Décision du mainteneur : FFmpeg reste en DLL sous Windows pour la 1.0** (le point « à rouvrir avant la 1.0 » de la ligne du 2026-10-01 est clos) | gain de taille faible, build statique MSVC long et à maintenir ; installeur à 62 Mo pour une cible de 120 Mo. Documenté dans `docs/LIMITATIONS.md` |
| 2026-10-03 | M18 (essai réel du mainteneur, 0.5.0 → 0.5.1) : mise à jour réussie. Défauts trouvés et corrigés pour la 0.5.2 : fenêtre de terminal visible lors du lancement de l'updater ; un dépôt sans release publiée (404) comptait comme une vérification échouée ; l'interface affichait « Mise à jour en cours » même si l'updater échouait (désormais `update.log` + erreur affichée dans les paramètres) ; la fenêtre des paramètres (processus à part) restait sur l'ancienne version (l'updater ferme maintenant les processus `vixeeny-app`). Leçon de publication : une release doit être publiée sans la case « pre-release » (l'API `latest` ignore les pré-versions) et il ne doit exister qu'une release par tag. |
| 2026-10-03 | Vignettes vidéo dans la galerie : première image décodée par FFmpeg (`vixeeny_encode::thumbnail::video_thumbnail`, feature `ffmpeg`), réduite comme les vignettes d'images ; test dans `tests/recorder.rs` | M17 |
| 2026-10-03 | Replay sur disque : `Storage::{Ram, Disk}` dans `replay.rs`, paquets écrits dans des fichiers de segment (32 Mio, coupés aux images clés, blocs d'écriture de 1 Mio, supprimés dès que plus aucun paquet ne les référence), anneau borné comme en RAM, dossiers périmés (> 24 h) nettoyés au démarrage, repli sur la RAM si le disque échoue ; réglage `replay.storage` (ram/disk) → `%TEMP%\Vixeeny-replay`. Tests : aller-retour disque, bornes, segment vivant pendant la sauvegarde, repli, fichier identique RAM/disque | M8 |
| 2026-10-03 | Annotation d'une capture défilante : l'image assemblée s'ouvre dans l'éditeur (toute l'image sélectionnée) en défilement 1:1 (molette, PgUp/PgDn, Début/Fin, ascenseur fin à droite) sous une barre d'outils fixe ; `Overlay::set_scrolling`, `Session::select_all`, `region::output_command` partagé avec le flux Impr. écran ; réglage `[scrolling] annotate` (vrai par défaut, faux = enregistrement immédiat), repli sur l'enregistrement direct si l'éditeur ne s'ouvre pas. **Écart** : pas de zoom ni de recadrage fin de l'image longue ; l'image reste alignée à gauche. 🧪 à valider par le mainteneur (Windows) | M9 |
| 2026-10-03 | Notifications Windows natives avec clic : `vixeeny_platform::native_toast` (toast WinRT `ToastGeneric`, image de la capture, bouton « Ouvrir le dossier »), clic et boutons en **activation par protocole** (URL `file:///…` ouverte par le shell, `vixeeny://settings` traité par `vixeeny-app --uri`), donc sans processus en attente ; enregistrement par utilisateur (AppUserModelId + schéma `vixeeny:`) à la première notification, sans droits admin. Réglage `general.notification_style` (`native` par défaut, `card` = carte Slint d'avant), repli automatique sur la carte si le toast échoue. La notification de mise à jour devient cliquable (ouvre les réglages) via `vixeeny-app --update-toast`. **Écart** : « Afficher dans le dossier » ouvre le dossier (pas de sélection du fichier, impossible par URL). 🧪 à valider sur Windows (affichage réel, mode Ne pas déranger) | M13 |
| 2026-10-03 | Audio 5.1 / 7.1 : les pistes ont 2, 6 ou 8 canaux (ordre Windows = FFmpeg : FL FR FC LFE BL BR [SL SR]). `AudioChunk.channels`, `Mixer::with_channels` (conversion de mise en page par source : mono/stéréo → paire avant, 7.1 ↔ 5.1, repli vers la stéréo avec −3 dB centre/surround, LFE écarté, normalisé contre l'écrêtage — `layout.rs`, testé), WASAPI : les points de terminaison gardent leur format natif (`GetMixFormat`, `WAVEFORMATEXTENSIBLE` + masque) jusqu'au maximum demandé par la piste, la boucle d'application reste stéréo ; `source_channels` sonde l'appareil avant l'enregistrement ; encodeur : `AudioTrackConfig.channels`, layouts `5POINT1_BACK` / `7POINT1`, débit réglé ×canaux/2. **Seulement en Matroska avec Opus, FLAC ou PCM** (`AudioCodec::surround_in`) ; MP4, WebM et AAC restent stéréo (AAC Media Foundation et lecteurs non fiables en multicanal). Réglage `audio.surround` (vrai par défaut). Test d'enregistrement 5.1/7.1 × Opus/FLAC/PCM (CI), mélange 5.1 + stéréo dans une piste 5.1. **Non fait** : réduction de bruit `afftdn` du micro (option du plan 5.10, reste à faire). 🧪 à valider sur un vrai système 5.1/7.1 | M13 |
| 2026-10-03 | M19, tranche 1 (macOS, **non testée sur un Mac**, vérifiée par `cargo clippy --target aarch64-apple-darwin` et par les jobs macOS de la CI) : `vixeeny-platform` macOS (`macos_impl.rs`, crates `objc2-core-graphics`) — écrans (CoreGraphics, pixels physiques = points × échelle), position du curseur, permission « Enregistrement de l'écran » (`screen_capture_allowed` / `request_screen_capture_access`), thème sombre, langue, `open` ; le reste retombe sur `unsupported`. `vixeeny-capture` : `SckBackend` (ScreenCaptureKit, `SCScreenshotManager`, macOS 14+, image BGRA, curseur optionnel) et l'exemple `cargo run -p vixeeny-capture --example grab` qui écrit un PPM par écran (outil de test pour un volontaire macOS). **Reste pour M19** : capture de fenêtre, repli macOS 13 (SCStream à une image), liste des fenêtres (`CGWindowListCopyWindowInfo`), exclusion des fenêtres de l'app, flux vidéo SCK → IOSurface → VideoToolbox, audio système/par application (SCK) et micro (AVFoundation), HDR | M19 |
| 2026-10-03 | M19, tranche 2 (macOS, **toujours non testée sur un Mac**) : liste des fenêtres (`CGWindowListCopyWindowInfo` : couche 0, à l'écran, ≥ 50 px, de l'avant vers l'arrière, exécutable par `proc_pidpath`), capture de fenêtre (SCK `initWithDesktopIndependentWindow`), horloge `monotonic_ns` = `mach_absolute_time` (la même que les horodatages SCK), flux vidéo `SckVideoStream` (SCStream, classe Objective-C `VixeenyStreamOutput` via `define_class!`, images BGRA copiées depuis le CVPixelBuffer, `minimumFrameInterval`, images perdues si l'encodeur est en retard), source audio `SckAudioSource` dans `vixeeny-audio` (audio système, audio d'une application trouvée par nom ou identifiant de bundle, micro via SCK sur macOS 15+ ; PCM planaire → entrelacé stéréo ; signale `Lost` à l'arrêt mais **ne réessaie pas encore**). Exemples de test pour un volontaire : `--example grab` (images, fenêtres, flux 2 s) et `--example listen [mic\|app:Nom]` (audio 5 s). **Reste** : brancher ces sources dans l'enregistreur macOS (encodeur VideoToolbox via le registre, chemin sans copie IOSurface), repli macOS 13 pour les images, micro AVFoundation avant macOS 15, exclusion des fenêtres de l'app, HDR, nouvelle tentative des sources audio | M19 |
| 2026-10-03 | M20, tranche 1 (macOS, **non testée sur un Mac**, vérifiée par clippy sur `aarch64-apple-darwin`, Windows et Linux) : démon sur macOS (`platform/macos.rs` : boucle `NSApplication` en mode accessoire, réveil par la file principale, icône de barre de menus et raccourcis globaux partagés avec Windows via `platform/desktop.rs`), presse-papiers `NSPasteboard`, côté application : captures directes via `SckBackend` (SDR, pas de tone-mapping), module `clipboard.rs` commun, notifications par `osascript` (**écart** : pas de clic sur la notification tant que l'application n'est pas un bundle `.app`, prévu avec l'empaquetage). Reste pour M20 : éditeur de région, réglages, OCR Vision, action rapide de conversion, vibrance, LaunchAgent, `.dmg`, mise à jour. | |
| 2026-10-03 | M20, tranche 2 (macOS, **non testée sur un Mac**) : l'éditeur de région (capture figée par ScreenCaptureKit, fenêtre Slint plein écran), la fenêtre de réglages / galerie et la fenêtre de conversion sont compilés pour macOS (mêmes sources que Windows, `cfg(any(windows, target_os = "macos"))`) ; l'action rapide Finder « Convertir avec Vixeeny » est un bundle `~/Library/Services/*.workflow` (Info.plist + document.wflow, script `vixeeny-app --convert "$@"`) installé/retiré comme l'entrée de registre sous Windows (`context_menu::{install,uninstall,is_installed}`). **Écarts** : OCR et capture défilante absents de macOS pour l'instant (le raccourci OCR attend Vision), mise à jour désactivée (pas encore de `vixeeny-updater` pour `.app`). | |
| 2026-10-03 | M20, tranche 3 (macOS, **non testée sur un Mac**) : OCR par Vision (`vixeeny-ocr::VisionEngine`, `VNRecognizeTextRequest` en mode précis, image transmise en BMP en mémoire pour éviter de construire un `CGImage` à la main ; langues = `supportedRecognitionLanguages`, noms affichés = codes BCP-47 bruts) ; la fenêtre de résultat est la même que sous Windows, le lien « langues » ouvre les Réglages Système. **Écart** : pas de nom de langue localisé pour l'instant. | |
| 2026-10-03 | M20, tranche 4 (macOS, **non testée sur un Mac**) : démarrage automatique en LaunchAgent explicite (`~/Library/LaunchAgents`, sans invite d'éléments de connexion) ; `cargo xtask dist-mac` assemble `Vixeeny.app` (agent `LSUIElement`, démon = exécutable principal, `vixeeny-app` à côté, descriptions d'usage écran/micro), la signe **ad hoc** (`codesign -s -`), et produit l'archive `Vixeeny-<v>-macos-<arch>.zip` et le `.dmg` (`hdiutil`, lien vers `/Applications`) ; la CI (`native-macos`) vérifie `plutil -lint` et `codesign --verify`, `release.yml` ajoute un job macOS qui joint les fichiers au brouillon. **Écarts** : pas d'icône d'application (aucun actif graphique dans le dépôt), pas de signature Developer ID ni de notarisation (compte Apple requis : à ta charge, voir section 10), binaire arm64 seul (universel plus tard), mise à jour automatique macOS et vibrancy non faites. | |
| 2026-10-03 | M20, tranche 5 (macOS, **non testée sur un Mac**) : mise à jour automatique macOS : `vixeeny-updater` (plateforme `macos-arm64` / `macos-x64`) remplace tout le bundle `Vixeeny.app` à partir de l'archive `.zip` (préparée hors du bundle, dans le dossier de réglages, pour ne pas invalider la signature), conserve les bits exécutables (test ajouté), arrête `vixeeny-app` par `pkill`, redémarre le démon et revient en arrière s'il ne répond pas ; chaque système a sa liste de sommes (`SHA256SUMS-macos`, Windows garde `SHA256SUMS`), le job macOS de `release.yml` signe `.zip`/`.dmg` avec minisign. **Écarts** : la vibrancy n'est pas faite — elle n'a de sens que pour la barre latérale (`side.rs`, encore Windows seulement), à reprendre avec le portage de cette barre ; l'archive téléchargée par l'updater n'a pas l'attribut de quarantaine, donc aucun `xattr` n'est nécessaire. Une app installée dans `/Applications` d'un autre utilisateur (non inscriptible) ne pourra pas se mettre à jour seule. | |
| 2026-10-03 | M20, tranche 6 (macOS, **non testée sur un Mac**, compilée par la CI avec FFmpeg) : barre latérale (`side.rs`) sur macOS avec vibrancy (`NSVisualEffectView` derrière la fenêtre + fenêtre transparente, `apply_acrylic`) et exclusion des captures (`NSWindow.sharingType = none`) ; enregistrement vidéo dans l'application (`record.rs` partagé avec Windows : flux ScreenCaptureKit `SckVideoStream`, pistes audio système / micro via `SckAudioSource`, tampon de replay, découpage, pause). **Écarts** : chemin CPU seulement (pas de zéro-copie IOSurface → VideoToolbox), SDR seulement, audio stéréo seulement (pas de 5.1/7.1), pas de widget d'enregistrement (stub : l'enregistrement se pilote par l'icône de barre de menus et les raccourcis), `SckVideoStream` rendu `Send` à la main (SCStream est thread-safe selon Apple). La vibrancy n'a jamais été vue : si le rendu Slint cache la vue d'effet, il faudra ajuster sur un vrai Mac. | |
| 2026-10-03 | M21, tranche 1 (Linux, X11) : `vixeeny-platform::linux_impl` (x11rb, sans bibliothèque C) : écrans par RandR 1.5 (échelle = `Xft.dpi`, 96 sinon), curseur, liste des fenêtres et fenêtre active par les propriétés EWMH (`_NET_CLIENT_LIST_STACKING`, cadre avec `_NET_FRAME_EXTENTS`, PID → `/proc/<pid>/exe`), nom d'application = `Name=` du `.desktop` dont `Exec` lance l'exécutable, langue depuis `LANG`, `xdg-open`, `gio trash` ; `vixeeny-capture` : `X11Backend` (image fixe par `GetImage` de la racine, alpha forcé opaque, curseur non inclus) et `X11VideoStream` (interrogation à `fps`, même interface que le flux macOS). Testé en CI sous Xvfb (fenêtre rouge réelle → pixel lu, flux de 5 images à horodatage croissant). **Vérifié ici** : écrans et curseur sur le X de WSLg ; la lecture de la racine y est refusée (XWayland sans racine lisible), ce qui est justement le cas Wayland à traiter par les portails (tranches suivantes). | |
| 2026-10-03 | M21, tranches 2 à 4 (Linux, **non testées sur un vrai bureau Wayland / avec GPU**) : (2) Wayland, images fixes par le portail `Screenshot` (`ashpd`, `PortalBackend` ; PNG décodé, découpe par écran avec le rapport d'échelle de l'image, fichier temporaire du portail supprimé) et `LinuxBackend` qui choisit portail (session Wayland) ou X11 ; tests purs (URI, PNG, découpe). (3) Vidéo Wayland : `PipeWireVideoStream` (portail `ScreenCast` → fd PipeWire + nœud, jeton de restauration enregistré dans un fichier, formats BGRx/BGRA/RGBx/RGBA en mémoire partagée, sans DMA-BUF ; images perdues si le consommateur est lent), et audio : `PipeWireAudioSource` (sortie par défaut = `stream.capture.sink`, micro par défaut ou nommé, **application** par son nom d'exécutable trouvé dans le registre PipeWire, 48 kHz stéréo `f32`, événements perdu/retrouvé) ; ces deux modules sont derrière la fonctionnalité `pipewire` (besoin de `libpipewire-0.3-dev` et clang) et compilés + clippy-és par un job CI `pipewire-linux`, mais ne peuvent pas être exécutés sans compositeur. (4) Encodeurs VAAPI / Vulkan : `vixeeny-encode::hwupload` (périphérique `av_hwdevice_ctx_create`, pool de surfaces, copie CPU → surface par image) branché dans l'enregistreur et dans le sondage (qui refusait ces encodeurs jusque-là) ; liste des GPU depuis `/sys/class/drm` avec noms de la base PCI. **Écarts** : pas de DMA-BUF → encodeur (zéro-copie), le périphérique VAAPI/Vulkan est celui par défaut (pas de choix du GPU), la fenêtre « application capturée » du portail n'est pas connue (nom = `Écran`), pas de test d'exécution PipeWire en CI (pas de serveur) ; à valider par 🧪 volontaires GNOME / KDE / Hyprland / X11. | |
| 2026-10-03 | M22, tranche 1 (daemon Linux, **non testée sur un vrai bureau** ; démarrage vérifié sous WSLg) : `platform/linux.rs` — pas de boucle d'interface, un canal : n'importe quel thread réveille la boucle (`Waker`), qui dort dans `recv` sinon. Tray = StatusNotifierItem par `ksni` (D-Bus, sans GTK ; sans `StatusNotifierWatcher` le daemon continue sans icône). Raccourcis : prises de touches X11 par `global-hotkey` (code partagé avec Windows/macOS, déplacé dans `platform/hotkeys.rs`) ; sur une session Wayland, portail `GlobalShortcuts` (`portal_shortcuts.rs`, session par thread, le portail confirme les touches). Pour les compositeurs sans portail : `vixeeny-daemon ctl <action>` (nouvelle `ControlRequest::Action`, testée dans `core`), à lier à une touche dans le compositeur. Notifications par `notify-send` (repli : infobulle). **Écarts** : la CLI est `vixeeny-daemon ctl` et non `vixeeny ctl` (pas de binaire `vixeeny` séparé) ; pas de layer-shell ni d'app Linux à ce stade (tranches suivantes). `ksni` est sous Unlicense (domaine public, compatible GPL) : ajoutée à `deny.toml`. |
| 2026-10-03 | M22, tranche 2 (app Linux, **non testée sur un vrai bureau** ; la CI joue le daemon et l'app sous Xvfb) : les modules de l'app partagés avec Windows/macOS sont ouverts à Linux (`cfg(any(windows, macos, linux))`). Captures directes par `LinuxBackend` (X11 / portail), notifications par `notify-send`, boîtes de fichiers par le portail xdg (`rfd` sans GTK). Presse-papiers : le processus app s'arrêtant juste après la capture, les données sont confiées à `wl-copy` (Wayland) ou `xclip`/`xsel` (X11) qui restent propriétaires ; sans eux, message qui nomme les paquets. OCR : `TesseractEngine` lance `tesseract` (pas de lien à la bibliothèque) ; codes Tesseract ↔ BCP-47, message « paquets à installer » par famille de distribution (apt/dnf/pacman) à la place de la commande Windows. Menu contextuel de conversion : `.desktop` « Ouvrir avec » + menu de service Dolphin (`desktop_entry`). Enregistrement : `Video` X11 ou PipeWire (portail) ; l'audio PipeWire et la vidéo Wayland sont derrière la fonctionnalité `pipewire` de l'app (sans elle : message clair, pas d'audio). Pas de widget d'enregistrement (la pastille reste à faire) ni de capture défilante sur Linux. `vixeeny-daemon ctl quit` ajouté pour les scripts. |
| 2026-10-03 | M22, tranche 3 (empaquetage Linux, **non testé sur un vrai bureau**) : `cargo xtask dist-linux` produit l'archive de mise à jour `Vixeeny-<v>-linux-x64.zip` (programmes + `.desktop` + icône, bits d'exécution conservés), le `.deb` (`dpkg-deb`, dépendances `libfontconfig1 libpipewire-0.3-0 libxkbcommon0`, recommandés `tesseract-ocr wl-clipboard xclip libnotify-bin`) et le `AppDir` que le job `linux` de `release.yml` transforme en AppImage (`appimagetool` 1.9.1 épinglé par SHA-256) ; ce job construit sur Ubuntu 24.04 (glibc 2.39 ; 22.04 a été essayé : sa libspa est trop ancienne pour la crate `pipewire` 0.10, d'où un minimum Ubuntu 24.04 / Debian 13 / Fedora 40), signe en minisign si le secret existe et joint `SHA256SUMS-linux` au brouillon. `packaging/arch/PKGBUILD` (`vixeeny-bin`, AUR) installe l'archive publiée. Identifiant d'application `io.github.Xantoom.Vixeeny` (règle Flathub pour un dépôt GitHub) ; icône SVG (carré violet + triangle, comme le tray) et métadonnées AppStream. Mise à jour : `vixeeny-updater` gère `linux-x64` (même schéma que Windows : remplacement des fichiers du dossier) mais refuse, avec un message, une installation par paquet/Flatpak/AppImage (`/usr`, `/opt`, `/app`, dossier non inscriptible) qui se met à jour par son propre canal. Autostart : `auto-launch` (`~/.config/autostart`), et le portail Background dans un bac à sable. **Reste** : Flatpak (manifeste + modules `wl-clipboard`, `xclip`, `tesseract`), icône PNG, pastille d'enregistrement, couche layer-shell. |
| 2026-10-03 | M22, tranche 4 (Flatpak, **non testé sur un vrai bureau** ; les modules d'aide sont compilés et lancés en CI) : `packaging/linux/flatpak/io.github.Xantoom.Vixeeny.yml` repaquette les programmes déjà construits (`dist/stage`) et compile dans le bac à sable `wl-clipboard`, `xclip` (+ `libXmu`), Leptonica et Tesseract 5.5.1 avec les données `eng`, `fra`, `osd` (sources épinglées par SHA-256) : sans eux le presse-papiers et l'OCR ne marcheraient pas dans le Flatpak. Permissions : Wayland/X11, DRI, socket PipeWire, `StatusNotifierWatcher`, Images et Vidéos. Le job CI `flatpak` construit le manifeste avec des programmes factices, vérifie `tesseract --version`, `wl-copy --version`, `xclip -version`, et valide le `.desktop` et l'AppStream ; le job `linux` de `release.yml` produit le vrai `Vixeeny-<v>.flatpak` (signé et joint au brouillon). Notifications : `vixeeny_platform::notify` passe par le portail `Notification` dans un bac à sable (pas de `notify-send`) et par `notify-send` sinon ; le daemon et l'app l'utilisent. **Écart** : seules les langues OCR `eng`/`fra` sont embarquées. **Reste pour M22** : pastille d'enregistrement, couche layer-shell, 🧪 GNOME/KDE/Hyprland/X11. |
| 2026-10-03 | M22, clôture — **deux écarts au plan** : (1) **pas de pastille d'enregistrement sous Linux** : ni X11 ni les portails Wayland ne permettent de laisser une fenêtre hors d'une capture, la pastille finirait dans la vidéo (CA-REC-5) ; le plan §8 prévoyait déjà « exclusion non garantie, option de masquer le widget » : le tray et les raccourcis pilotent l'enregistrement ; (2) **pas de layer-shell** : Slint 1.18 n'offre pas ce protocole, l'overlay de sélection est une fenêtre plein écran ordinaire (dégradé que le plan accepte pour GNOME, étendu aux autres compositeurs). Notés dans `docs/LIMITATIONS.md` (sections Linux et macOS). Les critères 🧪 restent aux volontaires GNOME/KDE/Hyprland/X11. |
| 2026-10-03 | M23 (préparation de la bêta 0.9, **version portée à 0.9.0**, le tag reste à pousser par le mainteneur) : « Copier les infos système » (Réglages → À propos) et `vixeeny-app --system-info` (module `sysinfo`) produisent un bloc Markdown sans donnée personnelle : version et fonctionnalités de la build, système (Linux : distribution, noyau, bureau, type de session, Flatpak/AppImage, présence de `tesseract`/`wl-copy`/`xclip`/`notify-send`), écrans, GPU, encodeurs, langue, format d'image. Modèles d'issues GitHub (`bug.yml`, `feedback.yml`, issues vides désactivées), `docs/BETA.md` (appel à testeurs et parcours), README réel (installation par système, raccourcis par défaut, Wayland et `ctl`). **À noter** : `releases/latest` n'expose pas les pré-versions, donc la bêta doit être publiée comme version normale pour que le contrôle de mise à jour la voie ; l'appel à testeurs lui-même (annonce publique) est une action du mainteneur. |
| 2026-10-03 | Reste du plan, tranche 1 : (1) **capture défilante sur macOS et Linux/X11** — le module `scroll` n'utilisait que `WgcBackend` (l'utilisateur fait défiler lui-même : pas d'injection d'entrée) ; il prend maintenant le moteur de capture de l'OS, `vixeeny-stitch` devient une dépendance commune ; **sous Wayland la capture défilante est refusée avec un message** (un portail ne peut pas capturer une zone 15 fois par seconde, et un flux ScreenCast n'est pas un moteur d'images fixes) ; (2) **pastille d'enregistrement sur macOS** : même processus fils que Windows, exclue de la capture par `sharingType = none` (déjà là), niveau flottant + tous les Spaces + au-dessus du plein écran (`set_noactivate_tool_window` macOS ; une `NSWindow` ne peut pas être rendue non activante après création) ; Linux reste sans pastille (écart du 2026-10-03) ; (3) **réduction de bruit du micro (`afftdn`)** : `vixeeny_encode::denoise::Denoiser` (graphe FFmpeg `abuffer → afftdn → aformat → abuffersink`, fonctionnalité `filter` de `ffmpeg-next`), appliqué par source micro dans `audio_rig` (les échantillons de sortie sont placés sur la ligne de temps en les comptant depuis le début du tronçon continu ; un trou > 50 ms relance le filtre), réglage de profil `[audio] mic_noise_reduction` (faux par défaut, 12 dB, plancher initial −45 dBFS, suivi du bruit `tn=1`) et case dans Réglages → Audio. Test : 2 s de bruit à −51 dBFS → au moins −20 % de RMS en fin de séquence, un sinus de 440 Hz conservé. |
| 2026-10-03 | Reste du plan, tranche 2 : **icône d'application** dessinée par `cargo xtask icons` (même dessin que le tray : `tiny-skia`, PNG 16 à 1024, `vixeeny.ico`, `vixeeny.icns` écrits à la main, fichiers versionnés dans `packaging/icons/`) : ressource Windows des deux exécutables (`winresource` dans un `build.rs` exécuté seulement quand on compile sous Windows ; icône, nom du produit, description) et `SetupIconFile` de l'installeur ; `Vixeeny.icns` + `CFBundleIconFile` dans le bundle macOS ; PNG hicolor dans le `.deb`, `<id>.png` + `.DirIcon` dans l'AppDir, PNG 256 dans le Flatpak. Documentation de la 1.0 : `docs/ARCHITECTURE.md` réécrit (deux processus, crates, capture/enregistrement, fichiers, packaging), `CONTRIBUTING.md`, `docs/LIMITATIONS.md` mis à jour. **Non faits, avec leur raison** : binaire macOS universel / Intel (les bibliothèques natives devraient être recompilées pour x86_64, et il n'y a plus de runner Intel gratuit), signature Developer ID + notarisation (compte Apple du mainteneur), copie sans passage par la mémoire (DMA-BUF) sous Linux (invérifiable sans GPU ni compositeur, et le risque de régression sur le chemin qui marche l'emporte), pastille et layer-shell sous Linux (voir la clôture de M22), capture défilante sous Wayland. |
| 2026-10-04 | Reste du plan, tranche 3 (retour du mainteneur : les secrets peuvent être posés par l'agent, et la 1.0 reste sa décision, pas la fin du plan). **macOS Intel** : plus de binaire universel, mais un build par architecture, chacun sur son runner (`macos-latest` arm64, `macos-15-intel` x64 : les bibliothèques natives sont compilées sur place), en CI (`native-macos` en matrice) et en release (`Vixeeny-<v>-macos-{arm64,x64}.{zip,dmg}`, `SHA256SUMS-macos-<arch>`) ; l'updater cherche d'abord la liste `SHA256SUMS-<plateforme>` (test ajouté). **Signature Developer ID + notarisation** préparées : `cargo xtask dist-mac` signe (runtime durci, horodatage, droit micro) quand `VIXEENY_SIGN_IDENTITY` est défini ; la release importe le certificat et notarise le dmg quand les secrets `APPLE_*` existent (documentés dans `packaging/README.md`), sinon signature ad hoc comme avant. **Reste** : fournir ces 5 secrets (compte Apple payant) ; DMA-BUF, pastille/layer-shell et capture défilante Wayland inchangés (invérifiables ici / limites de la plateforme). |
| 2026-10-04 | Retour de test Windows sur la v0.9.0 : refonte de l'interface et corrections. **F7 « ne fait rien » : cause trouvée** (reproduite en lançant la build instrumentée depuis WSL avec `VIXEENY_IPC_NAME` / `VIXEENY_CONFIG_DIR` pour ne pas toucher au démon installé) : quand l'app a quitté sur inactivité, le démon la relance avec l'action sur la ligne de commande (`--action`), mais l'app **ignorait** cette action (`first_action` n'était qu'affichée) : la première touche après 30 s sans usage était perdue (curseur occupé, rien). L'app exécute maintenant l'action de lancement avant d'attendre le démon. **Design** : thème Fluent (Windows 11) clair/sombre en un seul endroit (`theme.slint` : surfaces, texte, accent système lu dans le registre, mouvement coupé si les animations sont désactivées ; police Segoe UI, Inter ailleurs), contrôles maison (`controls.slint` : boutons, interrupteurs, listes, champs, segmentés, curseurs, cartes de réglage dont la hauteur suit le texte, barre d'info), **89 icônes Fluent System Icons (MIT)** générées par `tools/gen_icons.py`, icône de l'app sur toutes les fenêtres, barre de titre sombre/claire et couleur de légende via DWM (`style_window`), DPI par le facteur d'échelle de la fenêtre. Toutes les fenêtres refaites : réglages (navigation latérale, galerie double-clic = ouvrir), bandeau d'actions, barre d'outils de la capture (avec le bouton **capture défilante**), carte de notification (texte entier, hauteur selon le texte), pastille d'enregistrement, assistant, OCR, capture défilante. **Raccourcis** : on enregistre les touches (événement winit `KeyboardInput`, Échap annule, Retour arrière efface) au lieu de les taper ; les raccourcis globaux du démon sont suspendus pendant l'enregistrement (`ControlRequest::PauseHotkeys`, rendus par tout rechargement). **Vidéo** : détection des encodeurs au démarrage du démon en arrière-plan (`vixeeny-app --warm-probe`), liste limitée aux encodeurs disponibles, bascule Matériel/Logiciel (matériel par défaut), plus de « auto » (meilleur encodeur matériel), interrupteur 10 bits, préréglages Qualité maximale / Fichiers légers / Personnalisé (options de l'encodeur). **HDR** : interrupteur « Activer le HDR », sinon conversion automatique en SDR. **Audio** : toutes les sources listées (sorties, micros, programmes) avec cases. **Retirés** : fenêtre et crate de conversion, menu contextuel (code, page, installeur, menus Linux), notifications Windows natives et `notification_style` (cartes Slint uniquement, y compris pour la mise à jour), réglages de la capture défilante et son raccourci de la page (reste dans l'overlay). Widget d'enregistrement : le handle natif est redemandé après le démarrage de la boucle (l'exclusion de la capture échouait). **À valider 🧪** sur Windows. |
| 2026-10-04 | Branche `1.0`, demande du mainteneur : **Windows uniquement**, refonte de l'interface, gel de l'écran, mises à jour sans terminal, nettoyage. **Plateformes** : code, paquets, encodeurs et CI macOS / Linux retirés (les sections du plan qui les concernent sont caduques). **Nom** : le démon s'appelle `Vixeeny.exe` (gestionnaire des tâches, démarrage auto, raccourci avec l'AppUserModelID `Xantoom.Vixeeny`) ; l'archive 1.0 garde une copie `vixeeny-daemon.exe` qui passe la main, pour le programme de mise à jour de la 0.9 ; l'entrée de démarrage auto est réécrite à chaque lancement. Nouveau logo dans l'esprit de Vixely. **Gel** : une fenêtre par écran montre l'écran tel qu'il était puis l'assombrit en douceur (160 ms) ; loupe seulement pendant la sélection ; barre d'outils plus sobre qui monte en place. **Réglages** : sept pages (Galerie, Général, Raccourcis, Capture, Vidéo, Audio, À propos), groupes en cartes, interrupteurs, sélecteurs segmentés, navigation compacte sous 820 px, transitions courtes ; les lignes sans objet ne s'affichent pas ; profils gérés depuis leur ligne. **Mises à jour** : plus de `vixeeny-updater.exe` (console) ; `vixeeny-app --update` vérifie chaque jour, télécharge (barre de progression dans À propos), vérifie (SHA-256 + minisign), remplace les fichiers en cours d'exécution, redémarre `Vixeeny.exe` seul et revient en arrière s'il ne répond pas ; installation silencieuse par défaut (`general.auto_update`), sinon bouton « Redémarrer ». **Nettoyage** : CI et release Windows seules, `build-native` réduit à FFmpeg précompilé + SVT-AV1, dav1d et bibliothèques d'images, `deny.toml` limité à la cible Windows, 39 textes inutilisés supprimés, documentation réécrite. **À valider 🧪** sur Windows : gel multi-écran, réglages, mise à jour 0.9.1 → 0.9.2 (publiée en 0.9.2 à la demande du mainteneur). |
| 2026-10-05 | **0.9.3**, retours du mainteneur : barre de titre dessinée par la fenêtre des réglages ; tout s'anime (interrupteurs mis à jour sur place, coches, listes, curseurs, barre de défilement, étapes de l'assistant) ; galerie groupée par dossier, menu au clic droit, confirmation avant suppression, badges image / vidéo, filtres Captures / Vidéos / Replays ; vidéo : MP4 fragmenté seul, 30 / 60 (120 si l'écran le permet) i/s, HDR grisé sans HDR Windows, curseur masqué par défaut (schéma de réglages 2) ; audio : son du PC, micro (interrupteur + choix), programmes par leur nom ; gel plus fluide (pas d'animation d'ouverture, chaque écran ne redessine que ce qu'il montre) ; overlays refaits (barre d'outils sur une ligne, bande latérale en icônes avec épingle, barre d'enregistrement). **À valider 🧪** sur Windows. |
| 2026-10-05 | **0.9.4**, optimisation : FFmpeg réduit à ce que Vixeeny utilise (`packaging/ffmpeg/build.sh`, image BtbN, ≈ 53 Mo de DLL au lieu de ≈ 180), chargé en différé par `vixeeny-app` (bibliothèques d'import MSVC régénérées) ; `avdevice` et la copie `vixeeny-daemon.exe` retirés ; release construite sur la branche quand la version change (caches du CI réutilisés). |
| 2026-10-05 | **0.9.5**, gel de l'écran refait : le démon fige tous les écrans en parallèle sur le GPU dès l'appui (Windows Graphics Capture sur un device Direct3D 11 gardé prêt, textures partagées, fenêtre au premier plan par écran avec swap chain flip-model, HDR en scRGB), sans passer par le CPU ; l'app lit les textures, crée l'éditeur masqué (DWM cloak), le montre après sa première image puis retire le gel et assombrit seulement alors (plus de clignotement). Bouton « déplacer la zone », actif par défaut pour chaque nouvelle zone. **À valider 🧪** sur Windows. |
| 2026-10-05 | **0.9.6**, plus de flash ni de VRR à la capture (fenêtres de gel en swap chain « blt » et fenêtres de l'éditeur 1 px plus hautes, pour éviter le passage en plein écran exclusif), l'éditeur suit la fréquence maximale de l'écran (v-sync seulement sur la fenêtre active), s'affiche plus vite (lecture GPU groupée, conversion parallèle) ; la zone tout juste tracée se déplace avec le bon curseur, la loupe ne sert qu'à placer un bord. Économies : le widget ne s'anime plus en continu, les miniatures de la galerie sont mises en cache, les notifications passent par un seul processus. **À valider 🧪** sur Windows. |
| 2026-10-05 | **0.9.7**, zone toujours tracée librement (plus de détection de fenêtres), panneau latéral qui glisse depuis son bord, fichiers « Desktop » hors jeu plein écran, widget exclu de la capture dès sa création (plus de bloc noir). Paramètres réorganisés : titre aligné sur la barre, plus de « Réinitialiser », tableau des raccourcis à 3 emplacements, dossiers dans Général, onglet Replay, page Vidéo à la OBS (FPS en liste, 60 par défaut ; plus de VFR ni de carte graphique), HDR seulement si Windows l'affiche, programmes audio avec icônes (Chrome inclus). Barre d'outils de l'éditeur en groupes séparés, avec infobulles. **À valider 🧪** sur Windows. |
| 2026-10-06 | Éditeur de zone **natif** (`vixeeny-overlay`) à la place de l'éditeur Slint. Cause : l'éditeur Slint (OpenGL) redessinait toute l'image figée à chaque image et, pour que les écrans VRR restent à leur fréquence maximale, la fenêtre sous la souris se redessinait en continu (v-sync) : le GPU tournait comme pour un jeu tant que l'écran était assombri. Désormais une fenêtre Win32 par écran (`WS_EX_NOREDIRECTIONBITMAP`, sans swap chain donc jamais prise pour un jeu plein écran) dont le contenu est un arbre DirectComposition : l'image figée est envoyée une fois (tuiles ≤ 4096 px) et n'est plus jamais dessinée ; voile, bordure de la zone et poignées sont des visuels (surfaces 1×1 étirées) que le compositeur déplace ; seules les petites pièces qui changent (barre d'outils, infobulle, panneau de style, loupe, étiquette, champ de texte) sont dessinées en Direct2D/DirectWrite, et seulement quand elles changent. Fondus et montée de la barre = animations DirectComposition (exécutées par DWM). Au repos : aucun dessin, aucun timer (hors clignotement du curseur de texte). Mêmes `Session`, raccourcis, icônes (générées depuis `icons.slint`), couleurs et mise en page que l'ancien éditeur ; le champ de texte utilise la police Inter des annotations. Tests : pièces rendues hors écran (Direct2D, PNG), et intégration avec fenêtres masquées pilotées par messages (zone, barre, texte, panneau de style). `editor.slint` devient `app.slint` (index des fenêtres Slint). 🧪 à valider par le mainteneur : fluidité, absence de flash, clavier/curseurs, multi-écrans/DPI. | |
| 2026-10-06 | **0.9.8**, éditeur de zone natif (DirectComposition + Direct2D) : l'image figée n'est plus redessinée, le voile et la zone sont déplacés par le compositeur, plus de dessin continu ni de fenêtre prise pour un jeu plein écran. | |
| 2026-10-06 | **0.9.9**, correction de l'éditeur natif : l'image figée passait au-dessus du voile, de la zone et de la barre (ordre d'empilement DirectComposition inversé) ; le démon cède le premier plan à l'app avec l'action pour que l'éditeur ait le clavier. Exemple `smoke` (affichage réel bref + capture) ajouté. | |
| 2026-10-06 | **0.9.10**, mise à jour intégrée : une installation demandée par une ancienne fenêtre restée ouverte (version déjà en place) réussit au lieu d'échouer (« no update is ready ») ; une erreur périmée n'est plus affichée sur la carte de mise à jour. | |
| 2026-10-08 | **0.9.11** : copie Ctrl+C au format d'image choisi (JPEG collé en JPEG) ; **retraits** de la galerie, de l'OCR (crate `vixeeny-ocr`, `ocr_region`, `Ctrl+T`, bouton de l'overlay) et des profils d'enregistrement (le replay suit les réglages vidéo) ; **paramètres refaits** : onglets Général, Overlay, Image, Vidéo, Son, Replay, Raccourcis, Mise à jour, À propos, un seul titre par page, une carte par réglage (style Windows 11), dossier par onglet (choisir / ouvrir dans l'Explorateur), programmes audio ajoutés depuis la liste des programmes ouverts, raccourcis en touches séparées sans en-tête de tableau, transition d'onglet sans flash (glissement sans fondu), textes d'aide superflus retirés. Les miniatures des notifications ne passent plus par un cache disque. | |
| 2026-10-08 | Nettoyage : code mort retiré. Supprimés : l'outil de recadrage de l'éditeur (jamais accessible), une dizaine de fonctions inutilisées, `replay.profile` dans la config, la lecture BMP/TIFF/GIF (le décodeur ne sert plus qu'aux miniatures des notifications), 33 icônes inutilisées, les propriétés Slint jamais lues et deux dépendances (`thiserror` du démon, `vixeeny-capture` en dev de l'encodeur). Les dépendances sont compilées sans infos de débogage en dev. | |
| | *(à compléter par l'agent)* | |
