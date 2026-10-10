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
    if template {
        return pick(
            "The name of each file. {app} is the game in full screen, or Desktop when there is none; {date} and {time} are when it was taken.\n\nRecommended: {app}_{date}_{time}.",
            "Le nom de chaque fichier. {app} est le jeu en plein écran, ou Desktop s'il n'y en a pas ; {date} et {time} le moment de la capture.\n\nRecommandé : {app}_{date}_{time}.",
        );
    }
    match key {
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
            "How hard PNG compresses: the same picture, a smaller file, a little slower.\n\nRecommended: fast.",
            "L'effort de compression du PNG : même image, fichier plus petit, un peu plus lent.\n\nRecommandé : rapide.",
        ),
        "png_optimize" => pick(
            "oxipng shrinks the file again after it is written, without any loss. It takes a moment on large screens.",
            "oxipng réduit encore le fichier une fois écrit, sans aucune perte. Cela prend un instant sur les grands écrans.",
        ),
        "jpeg_quality" | "webp_quality" | "avif_quality" | "jxl_quality" => pick(
            "Higher is sharper and larger.\n\nRecommended: 90 (80 for AVIF).",
            "Plus haut = plus net et plus gros.\n\nRecommandé : 90 (80 pour l'AVIF).",
        ),
        "webp_lossless" | "jxl_lossless" => pick(
            "Keeps every pixel as it is: the right choice for text and interfaces.\n\nRecommended: on.",
            "Garde chaque pixel tel quel : le bon choix pour le texte et les interfaces.\n\nRecommandé : activé.",
        ),
        "webp_effort" | "jxl_effort" | "avif_speed" => pick(
            "Time spent to make the file smaller, at the same quality.\n\nRecommended: the default.",
            "Le temps passé à rendre le fichier plus petit, à qualité égale.\n\nRecommandé : la valeur par défaut.",
        ),
        "avif_depth" => pick(
            "10 bits keeps gradients smooth and is needed for HDR.\n\nRecommended: 10 bits.",
            "Le 10 bits garde des dégradés propres et sert au HDR.\n\nRecommandé : 10 bits.",
        ),
        "image_hdr" | "hdr" => pick(
            "Keeps the HDR of the screen. Off, the picture is converted to SDR, which looks right everywhere.",
            "Garde le HDR de l'écran. Désactivé, l'image est convertie en SDR, qui s'affiche bien partout.",
        ),
        "aspect" => pick(
            "Records only the middle of a wide screen in 16:9, the format of video sites and TVs.",
            "N'enregistre que le milieu d'un écran large en 16:9, le format des sites vidéo et des télés.",
        ),
        "encoder_kind" => pick(
            "Hardware: the graphics card encodes, without slowing the game. Software: the processor does it, slower.\n\nRecommended: hardware.",
            "Matériel : la carte graphique encode, sans ralentir le jeu. Logiciel : le processeur s'en charge, plus lentement.\n\nRecommandé : matériel.",
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
            "Fragmented MP4 plays everywhere and a recording cut short (crash, full disk) still plays. MKV holds any audio.\n\nRecommended: MP4.",
            "Le MP4 fragmenté se lit partout et un enregistrement interrompu (plantage, disque plein) reste lisible. Le MKV accepte tous les sons.\n\nRecommandé : MP4.",
        ),
        "chroma" => pick(
            "4:2:0 is what every player reads. 4:4:4 keeps the colour of fine text, but few players read it and files are larger.\n\nRecommended: 4:2:0.",
            "Le 4:2:0 est lu par tous les lecteurs. Le 4:4:4 garde la couleur du texte fin, mais peu de lecteurs le lisent et les fichiers sont plus gros.\n\nRecommandé : 4:2:0.",
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
                Some(d) if fr => format!("{base}\n\nRecommandé : {d}."),
                Some(d) => format!("{base}\n\nRecommended: {d}."),
                None => base,
            })
        }
        "p:rc.bitrate" => pick(
            "The bitrate aimed at. 1080p60: 10 000 to 20 000 kbit/s; 4K60: 40 000 to 60 000.",
            "Le débit visé. 1080p60 : 10 000 à 20 000 kbit/s ; 4K60 : 40 000 à 60 000.",
        ),
        "p:rc.maxrate" => pick(
            "The bitrate busy scenes may reach.\n\nRecommended: about 1.5 times the bitrate.",
            "Le débit que les scènes chargées peuvent atteindre.\n\nRecommandé : environ 1,5 fois le débit.",
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
                    "Plus lent = meilleure image à taille égale, mais plus de travail pour la machine. Trop lent, des images sautent.\n\nRecommandé : {recommended}."
                )
            } else {
                format!(
                    "Slower is a better picture for the size, but more work for the machine. Too slow and frames are dropped.\n\nRecommended: {recommended}."
                )
            })
        }
        "p:multipass" => pick(
            "A first quick look at each image to spread the bits better. Quarter resolution costs little.\n\nRecommended: quarter resolution.",
            "Un premier coup d'œil rapide sur chaque image pour mieux répartir le débit. Le quart de résolution coûte peu.\n\nRecommandé : quart de résolution.",
        ),
        "p:lookahead" => pick(
            "Images looked at in advance to plan the bitrate. Costs graphics memory.\n\nRecommended: 0 (off).",
            "Images regardées à l'avance pour prévoir le débit. Coûte de la mémoire vidéo.\n\nRecommandé : 0 (désactivé).",
        ),
        "p:spatial-aq" | "p:temporal-aq" | "p:vbaq" => pick(
            "Gives more bits to flat areas (skies, walls), where blocks show first.\n\nRecommended: on.",
            "Donne plus de débit aux zones unies (ciel, murs), où les blocs se voient en premier.\n\nRecommandé : activé.",
        ),
        "p:preanalysis" => pick(
            "Analyses each image before encoding it, for a slightly better picture.\n\nRecommended: off.",
            "Analyse chaque image avant de l'encoder, pour une image un peu meilleure.\n\nRecommandé : désactivé.",
        ),
        "p:bframes" => pick(
            "Images built from those before and after: smaller files.\n\nRecommended: auto.",
            "Images construites à partir de celles d'avant et d'après : fichiers plus petits.\n\nRecommandé : auto.",
        ),
        "p:keyint" => pick(
            "How often a complete image is stored. 0 lets Vixeeny choose (every 2 seconds), which keeps seeking quick.\n\nRecommended: 0.",
            "La fréquence des images complètes. 0 laisse Vixeeny choisir (toutes les 2 secondes), ce qui garde l'avance rapide fluide.\n\nRecommandé : 0.",
        ),
        "audio_capture" => pick(
            "All the PC sound: everything you hear. Only the recorded program: the game alone, without Discord or music.\n\nRecommended: all the PC sound.",
            "Tout le son du PC : tout ce que vous entendez. Seulement le programme enregistré : le jeu seul, sans Discord ni musique.\n\nRecommandé : tout le son du PC.",
        ),
        "audio_output" => pick(
            "An output records what that device plays; an input records what comes into it (a capture card, a line input).",
            "Une sortie enregistre ce que l'appareil joue ; une entrée ce qui y arrive (carte d'acquisition, entrée ligne).",
        ),
        "audio_denoise" => pick(
            "Removes the steady noise of the microphone (fan, hum).\n\nRecommended: on.",
            "Enlève le bruit continu du micro (ventilateur, souffle).\n\nRecommandé : activé.",
        ),
        "mic_level" => pick(
            "What the microphone hears now. Speak normally: the bar should reach the middle without turning red.",
            "Ce que le micro entend en ce moment. Parlez normalement : la barre doit atteindre le milieu sans passer au rouge.",
        ),
        "audio_routing" => pick(
            "One track per source lets you set each volume when editing; one mixed track plays the same everywhere.\n\nRecommended: one track per source.",
            "Une piste par source permet de régler chaque volume au montage ; une piste mixée se lit pareil partout.\n\nRecommandé : une piste par source.",
        ),
        "audio_codec" => pick(
            "AAC plays everywhere; Opus is better at the same bitrate; FLAC and PCM keep everything but weigh a lot.\n\nRecommended: automatic.",
            "L'AAC se lit partout ; l'Opus est meilleur à débit égal ; le FLAC et le PCM gardent tout mais pèsent lourd.\n\nRecommandé : automatique.",
        ),
        "audio_bitrate" => pick(
            "Higher keeps more detail. 160 kbit/s cannot be told from the original.\n\nRecommended: 160 kbit/s.",
            "Plus haut garde plus de détails. 160 kbit/s est indiscernable de l'original.\n\nRecommandé : 160 kbit/s.",
        ),
        "audio_channels" => pick(
            "Stereo suits headphones and speakers. 5.1 and 7.1 keep surround sound, in MKV only.\n\nRecommended: stereo.",
            "La stéréo convient aux casques et aux enceintes. Le 5.1 et le 7.1 gardent le son surround, en MKV seulement.\n\nRecommandé : stéréo.",
        ),
        "replay_enabled" => pick(
            "Keeps the last moments of a game: its shortcut saves them as a video. It runs only while a full-screen game is in front.",
            "Garde les derniers instants d'un jeu : son raccourci les enregistre en vidéo. Il ne tourne que pendant un jeu en plein écran.",
        ),
        "replay_duration" => pick(
            "How much is kept, before the moment you save. Longer takes more memory or disk.\n\nRecommended: 30 s to 2 min.",
            "Ce qui est gardé avant le moment où vous enregistrez. Plus long prend plus de mémoire ou de disque.\n\nRecommandé : 30 s à 2 min.",
        ),
        "replay_after" => pick(
            "The replay goes on for a few seconds after its shortcut, so the end of the action is in it.\n\nRecommended: 5 to 10 seconds if you often save too early.",
            "Le replay continue quelques secondes après son raccourci, pour que la fin de l'action y soit.\n\nRecommandé : 5 à 10 secondes si vous enregistrez souvent trop tôt.",
        ),
        "replay_storage" => pick(
            "RAM spares the disk but takes memory from the game; the disk suits long replays and PCs with little RAM. Automatic chooses for you.\n\nRecommended: automatic.",
            "La RAM ménage le disque mais prend de la mémoire au jeu ; le disque convient aux longs replays et aux PC avec peu de RAM. Automatique choisit pour vous.\n\nRecommandé : automatique.",
        ),
        _ => None,
    }
}
