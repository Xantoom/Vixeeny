// SPDX-License-Identifier: GPL-3.0-or-later
//! What a setting does and what to choose, shown behind the "i" beside its label. Only where
//! there is something useful to say: obvious settings have none.

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Lang;

use crate::Env;
use crate::encoders;

/// The library that writes each image format.
fn image_encoder(format: &str) -> &'static str {
    match format {
        "png" => "png + oxipng",
        "jpeg" => "jpegli",
        "webp" => "libwebp",
        "avif" => "libavif + SVT-AV1",
        "jxl" => "libjxl",
        _ => "",
    }
}

/// The info of the row `id`, in the language of `env` (`None`: nothing to add).
pub fn info(id: &str, env: &Env, config: &Config) -> Option<String> {
    let fr = env.lang == Lang::Fr;
    let pick = |en: &str, f: &str| Some(if fr { f } else { en }.to_owned());
    // The encoder's own options are `p:<key>`.
    let key = id;
    let template = id.starts_with("template:");
    let folder_kind = id.strip_prefix("sub_");
    if template {
        return pick(
            "The name of each file. {app} is the game in full screen, or Desktop when there is none; {date} and {time} are when it was taken. Recommended: {app}_{date}_{time}.",
            "Le nom de chaque fichier. {app} est le jeu en plein écran, ou Desktop s'il n'y en a pas ; {date} et {time} le moment de la capture. Recommandé : {app}_{date}_{time}.",
        );
    }
    if folder_kind.is_some() {
        return pick(
            "Puts the files of each game in a folder of its own, named after it.",
            "Range les fichiers de chaque jeu dans un dossier à son nom.",
        );
    }
    match key {
        "autostart" => pick(
            "Vixeeny starts with Windows, so its shortcuts work right away. Recommended: on.",
            "Vixeeny démarre avec Windows, pour que ses raccourcis marchent tout de suite. Recommandé : activé.",
        ),
        "notifications" => pick(
            "A notification when a capture or a recording is saved, to open it in one click.",
            "Une notification quand une capture ou un enregistrement est enregistré, pour l'ouvrir en un clic.",
        ),
        "sounds" => pick(
            "A short sound confirms captures and recordings.",
            "Un petit son confirme les captures et les enregistrements.",
        ),
        "overlay_edge" => pick(
            "The edge of the screen the side strip comes out of.",
            "Le bord de l'écran d'où sort le bandeau latéral.",
        ),
        "widget" => pick(
            "A small window shows the length of the recording and lets you pause or stop it.",
            "Une petite fenêtre montre la durée de l'enregistrement et permet de le mettre en pause ou de l'arrêter.",
        ),
        "image_format" => {
            let encoder = image_encoder(&config.image.format);
            let (en, f) = match config.image.format.as_str() {
                "png" => (
                    "PNG: lossless, read everywhere, larger files. Recommended for screenshots.",
                    "PNG : sans perte, lisible partout, fichiers plus gros. Recommandé pour les captures.",
                ),
                "jpeg" => (
                    "JPEG: small files, read everywhere, with a slight loss on text and edges.",
                    "JPEG : petits fichiers, lisibles partout, avec une légère perte sur le texte et les contours.",
                ),
                "webp" => (
                    "WebP: smaller than PNG and JPEG, read by browsers and most apps.",
                    "WebP : plus petit que PNG et JPEG, lu par les navigateurs et la plupart des applications.",
                ),
                "avif" => (
                    "AVIF: very small files and HDR, but not every app reads it yet.",
                    "AVIF : fichiers très petits et HDR, mais toutes les applications ne le lisent pas encore.",
                ),
                _ => (
                    "JPEG XL: the smallest files, lossless or not, and HDR; few apps read it yet.",
                    "JPEG XL : les fichiers les plus petits, avec ou sans perte, et le HDR ; peu d'applications le lisent encore.",
                ),
            };
            Some(if fr {
                format!("{f}\nEncodeur : {encoder}.")
            } else {
                format!("{en}\nEncoder: {encoder}.")
            })
        }
        "png_compression" => pick(
            "How hard PNG compresses: the same picture, a smaller file, a little slower. Recommended: fast.",
            "L'effort de compression du PNG : même image, fichier plus petit, un peu plus lent. Recommandé : rapide.",
        ),
        "png_optimize" => pick(
            "oxipng shrinks the file again after it is written, without any loss. It takes a moment on large screens.",
            "oxipng réduit encore le fichier une fois écrit, sans aucune perte. Cela prend un instant sur les grands écrans.",
        ),
        "jpeg_quality" | "webp_quality" | "avif_quality" | "jxl_quality" => pick(
            "Higher is sharper and larger. Recommended: 90 (80 for AVIF).",
            "Plus haut = plus net et plus gros. Recommandé : 90 (80 pour l'AVIF).",
        ),
        "webp_lossless" | "jxl_lossless" => pick(
            "Keeps every pixel as it is: the right choice for text and interfaces. Recommended: on.",
            "Garde chaque pixel tel quel : le bon choix pour le texte et les interfaces. Recommandé : activé.",
        ),
        "webp_effort" | "jxl_effort" | "avif_speed" => pick(
            "Time spent to make the file smaller, at the same quality. Recommended: the default.",
            "Le temps passé à rendre le fichier plus petit, à qualité égale. Recommandé : la valeur par défaut.",
        ),
        "avif_depth" => pick(
            "10 bits keeps gradients smooth and is needed for HDR. Recommended: 10 bits.",
            "Le 10 bits garde des dégradés propres et sert au HDR. Recommandé : 10 bits.",
        ),
        "clipboard" => pick(
            "Each capture is also copied, ready to paste in a message.",
            "Chaque capture est aussi copiée, prête à coller dans un message.",
        ),
        "image_hdr" | "hdr" => pick(
            "Keeps the HDR of the screen. Off, the picture is converted to SDR, which looks right everywhere.",
            "Garde le HDR de l'écran. Désactivé, l'image est convertie en SDR, qui s'affiche bien partout.",
        ),
        "aspect" => pick(
            "Records only the middle of a wide screen in 16:9, the format of video sites and TVs.",
            "N'enregistre que le milieu d'un écran large en 16:9, le format des sites vidéo et des télés.",
        ),
        "resolution" => pick(
            "The size of the video. Lower makes lighter files, but less sharp. Recommended: the screen's.",
            "La taille de la vidéo. Plus bas = fichiers plus légers mais moins nets. Recommandé : celle de l'écran.",
        ),
        "fps" => pick(
            "Images per second. 60 is smooth for games; more needs a fast encoder and makes larger files. Recommended: 60.",
            "Images par seconde. 60 est fluide pour les jeux ; plus demande un encodeur rapide et fait des fichiers plus gros. Recommandé : 60.",
        ),
        "encoder_kind" => pick(
            "Hardware: the graphics card encodes, without slowing the game. Software: the processor does it, slower. Recommended: hardware.",
            "Matériel : la carte graphique encode, sans ralentir le jeu. Logiciel : le processeur s'en charge, plus lentement. Recommandé : matériel.",
        ),
        "encoder" => pick(
            "AV1 makes the smallest files at the same quality, then HEVC; H.264 plays everywhere, even on old devices.",
            "L'AV1 fait les fichiers les plus petits à qualité égale, puis le HEVC ; le H.264 se lit partout, même sur de vieux appareils.",
        ),
        "preset" => pick(
            "Best quality: a sharp picture, larger files. Light files: smaller, a little less sharp. Custom: every option by hand.",
            "Meilleure qualité : image nette, fichiers plus gros. Fichiers légers : plus petits, un peu moins nets. Personnalisé : toutes les options à la main.",
        ),
        "container" => pick(
            "Fragmented MP4 plays everywhere and a recording cut short (crash, full disk) still plays. MKV holds any audio. Recommended: MP4.",
            "Le MP4 fragmenté se lit partout et un enregistrement interrompu (plantage, disque plein) reste lisible. Le MKV accepte tous les sons. Recommandé : MP4.",
        ),
        "split" => pick(
            "Starts a new file past a size or a length, for the disks and sites that limit them.",
            "Commence un nouveau fichier au-delà d'une taille ou d'une durée, pour les disques et les sites qui les limitent.",
        ),
        "chroma" => pick(
            "4:2:0 is what every player reads. 4:4:4 keeps the colour of fine text, but few players read it and files are larger. Recommended: 4:2:0.",
            "Le 4:2:0 est lu par tous les lecteurs. Le 4:4:4 garde la couleur du texte fin, mais peu de lecteurs le lisent et les fichiers sont plus gros. Recommandé : 4:2:0.",
        ),
        "p:rc.mode" => pick(
            "Constant quality: the same quality all along, the size follows the picture (recommended). Constant QP: simpler, larger. VBR: aims at a bitrate. CBR: the same bitrate all along, for streaming.",
            "Qualité constante : même qualité tout du long, la taille suit l'image (recommandé). QP constant : plus simple, plus gros. VBR : vise un débit. CBR : même débit tout du long, pour le streaming.",
        ),
        "p:rc.quality" | "p:rc.qp" => {
            let encoder =
                encoders::resolved(env, &config.video).and_then(|e| encoders::spec(&e.id));
            let default = encoder.map(|e| {
                if key == "p:rc.qp" {
                    e.rate_control.qp.as_ref().map_or(0, |q| q.default)
                } else {
                    e.rate_control.quality.default
                }
            });
            let base = pick(
                "Lower is a better picture and larger files.",
                "Plus bas = meilleure image et fichiers plus gros.",
            )?;
            Some(match default {
                Some(d) if fr => format!("{base} Recommandé : {d}."),
                Some(d) => format!("{base} Recommended: {d}."),
                None => base,
            })
        }
        "p:rc.bitrate" => pick(
            "The bitrate aimed at. 1080p60: 10 000 to 20 000 kbit/s; 4K60: 40 000 to 60 000.",
            "Le débit visé. 1080p60 : 10 000 à 20 000 kbit/s ; 4K60 : 40 000 à 60 000.",
        ),
        "p:rc.maxrate" => pick(
            "The bitrate busy scenes may reach. Recommended: about 1.5 times the bitrate.",
            "Le débit que les scènes chargées peuvent atteindre. Recommandé : environ 1,5 fois le débit.",
        ),
        "p:preset" | "p:quality" | "p:deadline" | "p:cpu-used" => {
            let id = encoders::resolved(env, &config.video).map(|e| e.id.as_str());
            let recommended = match id {
                Some(i) if i.starts_with("nvenc") => "P5",
                Some(i) if i.starts_with("amf") => {
                    if fr {
                        "équilibré"
                    } else {
                        "balanced"
                    }
                }
                Some("libx264") => {
                    if fr {
                        "très rapide"
                    } else {
                        "very fast"
                    }
                }
                Some("libx265") => {
                    if fr {
                        "super rapide"
                    } else {
                        "super fast"
                    }
                }
                Some("libsvtav1") => "10",
                Some("libvpx_vp9") => {
                    if fr {
                        "temps réel"
                    } else {
                        "real time"
                    }
                }
                _ => {
                    if fr {
                        "moyen"
                    } else {
                        "medium"
                    }
                }
            };
            Some(if fr {
                format!(
                    "Plus lent = meilleure image à taille égale, mais plus de travail pour la machine. Trop lent, des images sautent. Recommandé : {recommended}."
                )
            } else {
                format!(
                    "Slower is a better picture for the size, but more work for the machine. Too slow and frames are dropped. Recommended: {recommended}."
                )
            })
        }
        "p:multipass" => pick(
            "A first quick look at each image to spread the bits better. Quarter resolution costs little. Recommended: quarter resolution.",
            "Un premier coup d'œil rapide sur chaque image pour mieux répartir le débit. Le quart de résolution coûte peu. Recommandé : quart de résolution.",
        ),
        "p:lookahead" => pick(
            "Images looked at in advance to plan the bitrate. Costs graphics memory. Recommended: 0 (off).",
            "Images regardées à l'avance pour prévoir le débit. Coûte de la mémoire vidéo. Recommandé : 0 (désactivé).",
        ),
        "p:spatial-aq" | "p:temporal-aq" | "p:vbaq" => pick(
            "Gives more bits to flat areas (skies, walls), where blocks show first. Recommended: on.",
            "Donne plus de débit aux zones unies (ciel, murs), où les blocs se voient en premier. Recommandé : activé.",
        ),
        "p:preanalysis" => pick(
            "Analyses each image before encoding it, for a slightly better picture. Recommended: off.",
            "Analyse chaque image avant de l'encoder, pour une image un peu meilleure. Recommandé : désactivé.",
        ),
        "p:bframes" => pick(
            "Images built from those before and after: smaller files. Recommended: auto.",
            "Images construites à partir de celles d'avant et d'après : fichiers plus petits. Recommandé : auto.",
        ),
        "p:keyint" => pick(
            "How often a complete image is stored. 0 lets Vixeeny choose (every 2 seconds), which keeps seeking quick. Recommended: 0.",
            "La fréquence des images complètes. 0 laisse Vixeeny choisir (toutes les 2 secondes), ce qui garde l'avance rapide fluide. Recommandé : 0.",
        ),
        "p:tune" => pick(
            "Adjusts the encoder to a kind of picture. Recommended: auto.",
            "Adapte l'encodeur à un type d'image. Recommandé : auto.",
        ),
        "p:row-mt" => pick(
            "Uses more processor cores. Recommended: on.",
            "Utilise plus de cœurs du processeur. Recommandé : activé.",
        ),
        "audio_capture" => pick(
            "All the PC sound: everything you hear. Only the recorded program: the game alone, without Discord or music. Recommended: all the PC sound.",
            "Tout le son du PC : tout ce que vous entendez. Seulement le programme enregistré : le jeu seul, sans Discord ni musique. Recommandé : tout le son du PC.",
        ),
        "audio_output" => pick(
            "An output records what that device plays; an input records what comes into it (a capture card, a line input).",
            "Une sortie enregistre ce que l'appareil joue ; une entrée ce qui y arrive (carte d'acquisition, entrée ligne).",
        ),
        "audio_denoise" => pick(
            "Removes the steady noise of the microphone (fan, hum). Recommended: on.",
            "Enlève le bruit continu du micro (ventilateur, souffle). Recommandé : activé.",
        ),
        "audio_routing" => pick(
            "One track per source lets you set each volume when editing; one mixed track plays the same everywhere. Recommended: one track per source.",
            "Une piste par source permet de régler chaque volume au montage ; une piste mixée se lit pareil partout. Recommandé : une piste par source.",
        ),
        "audio_codec" => pick(
            "AAC plays everywhere; Opus is better at the same bitrate; FLAC and PCM keep everything but weigh a lot. Recommended: automatic.",
            "L'AAC se lit partout ; l'Opus est meilleur à débit égal ; le FLAC et le PCM gardent tout mais pèsent lourd. Recommandé : automatique.",
        ),
        "audio_bitrate" => pick(
            "Higher keeps more detail. 160 kbit/s cannot be told from the original. Recommended: 160 kbit/s.",
            "Plus haut garde plus de détails. 160 kbit/s est indiscernable de l'original. Recommandé : 160 kbit/s.",
        ),
        "audio_vbr" => pick(
            "Variable spends bits where the sound needs them. Recommended: variable.",
            "Le variable met le débit là où le son en a besoin. Recommandé : variable.",
        ),
        "audio_channels" => pick(
            "Stereo suits headphones and speakers. 5.1 and 7.1 keep surround sound, in MKV only. Recommended: stereo.",
            "La stéréo convient aux casques et aux enceintes. Le 5.1 et le 7.1 gardent le son surround, en MKV seulement. Recommandé : stéréo.",
        ),
        "replay_enabled" => pick(
            "Keeps the last moments of a game: its shortcut saves them as a video. It runs only while a full-screen game is in front.",
            "Garde les derniers instants d'un jeu : son raccourci les enregistre en vidéo. Il ne tourne que pendant un jeu en plein écran.",
        ),
        "replay_duration" => pick(
            "How much is kept, before the moment you save. Longer takes more memory or disk. Recommended: 30 s to 2 min.",
            "Ce qui est gardé avant le moment où vous enregistrez. Plus long prend plus de mémoire ou de disque. Recommandé : 30 s à 2 min.",
        ),
        "replay_storage" => pick(
            "RAM spares the disk but takes memory from the game; the disk suits long replays and PCs with little RAM. Automatic chooses for you. Recommended: automatic.",
            "La RAM ménage le disque mais prend de la mémoire au jeu ; le disque convient aux longs replays et aux PC avec peu de RAM. Automatique choisit pour vous. Recommandé : automatique.",
        ),
        _ => None,
    }
}
