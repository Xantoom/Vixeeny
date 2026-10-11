// SPDX-License-Identifier: GPL-3.0-or-later
//! What a setting does and what to choose, shown behind the "i" beside its label. Only where
//! there is something useful to say: obvious settings have none.

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Lang;

use crate::Env;
use crate::encoders;

/// The info of the row `id`, in the language of `env` (`None`: nothing to add).
pub fn info(id: &str, env: &Env, config: &Config) -> Option<String> {
    let fr = env.lang == Lang::Fr;
    let pick = |en: &str, f: &str| Some(if fr { f } else { en }.to_owned());
    // The encoder's own options are `p:<key>`.
    let key = id;
    if id.starts_with("template:") {
        return pick(
            "The name given to each file. {app}: the game being played (or Desktop). {date} and {time}: when it was taken.",
            "Le nom donné à chaque fichier. {app} : le jeu en cours (ou Desktop). {date} et {time} : le moment de la capture.",
        );
    }
    match key {
        "image_format" => match config.image.format.as_str() {
            "png" => pick(
                "No loss, read everywhere, larger files.",
                "Aucune perte, lu partout, fichiers plus gros.",
            ),
            "jpeg" => pick(
                "Light files, read everywhere. Text may blur a little.",
                "Fichiers légers, lus partout. Le texte peut baver un peu.",
            ),
            "webp" => pick(
                "Lighter than PNG and JPEG, read by browsers.",
                "Plus léger que PNG et JPEG, lu par les navigateurs.",
            ),
            "avif" => pick(
                "Very light and HDR-capable, but not read everywhere yet.",
                "Très léger et compatible HDR, mais pas encore lu partout.",
            ),
            _ => pick(
                "The lightest, HDR-capable, but rarely read.",
                "Le plus léger, compatible HDR, mais rarement lu.",
            ),
        },
        "webp_lossless" | "jxl_lossless" => pick(
            "Keeps the exact picture, pixel for pixel. Larger files.",
            "Garde l'image exacte, pixel par pixel. Fichiers plus gros.",
        ),
        "avif_speed" => pick(
            "Higher saves faster, with a larger file.",
            "Plus haut = enregistrement plus rapide, fichier plus gros.",
        ),
        "webp_effort" | "jxl_effort" => pick(
            "More effort makes a smaller file at the same quality, but saving takes longer.",
            "Plus d'effort donne un fichier plus petit, à qualité égale, mais l'enregistrement est plus long.",
        ),
        "avif_depth" => pick(
            "10 bits avoids bands in gradients. Needed for HDR.",
            "10 bits évite les bandes dans les dégradés. Nécessaire pour le HDR.",
        ),
        "image_hdr" | "hdr" => pick(
            "Keeps the HDR colours and brightness of the screen. Off, the picture is converted to show right everywhere.",
            "Garde les couleurs et la luminosité HDR de l'écran. Désactivé, l'image est convertie pour s'afficher correctement partout.",
        ),
        "encoder_kind" => pick(
            "Hardware: the graphics card encodes, the game does not slow down. Software: the processor encodes, which can slow the game.",
            "Matériel : la carte graphique encode, le jeu ne ralentit pas. Logiciel : le processeur encode, ce qui peut ralentir le jeu.",
        ),
        "encoder" => pick(
            "AV1: the lightest files. HEVC: a little heavier. H.264: plays everywhere, even on old devices.",
            "AV1 : les fichiers les plus légers. HEVC : un peu plus lourds. H.264 : lu partout, même sur les vieux appareils.",
        ),
        "container" => pick(
            "The kind of video file. MP4: plays everywhere, and a video cut by a crash still plays. MKV: takes every sound format.",
            "Le type de fichier vidéo. MP4 : lu partout, et une vidéo coupée par un plantage reste lisible. MKV : accepte tous les formats de son.",
        ),
        "chroma" => pick(
            "4:2:0: plays everywhere. 4:4:4: more precise colours on fine text, but rarely read and heavier.",
            "4:2:0 : lu partout. 4:4:4 : couleurs plus précises sur le texte fin, mais rarement lu et plus lourd.",
        ),
        "p:rc.mode" => pick(
            "Constant quality: the quality stays the same, the file size varies. Constant QP: simpler, heavier. VBR: aims at a size. CBR: a fixed size, for streaming.",
            "Qualité constante : la qualité reste la même, la taille du fichier varie. QP constant : plus simple, plus lourd. VBR : vise une taille. CBR : taille fixe, pour le streaming.",
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
                "Lower is sharper and heavier.",
                "Plus bas = plus net et plus lourd.",
            )?;
            Some(match default {
                Some(d) if fr => format!("{base}\n\nRecommandé : {d}."),
                Some(d) => format!("{base}\n\nRecommended: {d}."),
                None => base,
            })
        }
        "p:rc.bitrate" => pick(
            "The amount of data per second: higher is sharper and heavier. For reference: 10 000 to 20 000 in 1080p60, 40 000 to 60 000 in 4K60.",
            "La quantité de données par seconde : plus haut = plus net et plus lourd. Repères : 10 000 à 20 000 en 1080p60, 40 000 à 60 000 en 4K60.",
        ),
        "p:rc.maxrate" => pick(
            "The ceiling for very busy scenes.\n\nRecommended: 1.5 × the bitrate.",
            "Le plafond pour les scènes très animées.\n\nRecommandé : 1,5 × le débit.",
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
                    "Plus lent = meilleure image pour la même taille, mais plus de calcul. Trop lent, des images sautent.\n\nRecommandé : {recommended}."
                )
            } else {
                format!(
                    "Slower is a better picture for the same size, but more computing. Too slow and frames are skipped.\n\nRecommended: {recommended}."
                )
            })
        }
        "p:multipass" => pick(
            "Looks quickly at each image before encoding it, to spread the quality better.",
            "Analyse rapidement chaque image avant de l'encoder, pour mieux répartir la qualité.",
        ),
        "p:lookahead" => pick(
            "Looks at the next images to spread the quality better. Uses graphics memory.",
            "Regarde les images suivantes pour mieux répartir la qualité. Utilise de la mémoire vidéo.",
        ),
        "p:spatial-aq" | "p:temporal-aq" | "p:vbaq" => pick(
            "Improves flat areas (skies, walls), where flaws show most.",
            "Améliore les zones unies (ciel, murs), où les défauts se voient le plus.",
        ),
        "p:preanalysis" => pick(
            "Analyses each image before encoding. A slight gain, more computing.",
            "Analyse chaque image avant l'encodage. Gain léger, plus de calcul.",
        ),
        "p:bframes" => pick(
            "Images rebuilt from their neighbours: lighter files.",
            "Images reconstruites à partir de leurs voisines : fichiers plus légers.",
        ),
        "p:keyint" => pick(
            "How often a complete image is stored. More often: smoother seeking, heavier file. 0: automatic.",
            "Tous les combien une image complète est enregistrée. Plus fréquent = avance rapide plus fluide, fichier plus lourd. 0 : automatique.",
        ),
        "audio_capture" => pick(
            "All the PC sound: what you hear. The program alone: the game, without Discord or music.",
            "Tout le son du PC : ce que vous entendez. Le programme seul : le jeu, sans Discord ni musique.",
        ),
        "audio_output" => pick(
            "An output records what the device plays, an input what it receives (capture card, line input).",
            "Une sortie enregistre ce que l'appareil joue, une entrée ce qu'il reçoit (carte d'acquisition, entrée ligne).",
        ),
        "mic_level" => pick(
            "Speak normally: the bar should reach the middle without turning red.",
            "Parlez normalement : la barre doit atteindre le milieu sans passer au rouge.",
        ),
        "audio_routing" => pick(
            "One track per source: each volume can be set when editing. One mixed track: plays the same everywhere.",
            "Une piste par source : chaque volume se règle au montage. Une seule piste mixée : se lit pareil partout.",
        ),
        "audio_codec" => pick(
            "AAC: plays everywhere. Opus: better for the same size. FLAC and PCM: no loss, very heavy.",
            "AAC : lu partout. Opus : meilleur pour la même taille. FLAC et PCM : aucune perte, très lourds.",
        ),
        "audio_bitrate" => pick(
            "Higher is more faithful sound. From 160 kbit/s, the difference cannot be heard.",
            "Plus haut = son plus fidèle. Dès 160 kbit/s, la différence ne s'entend plus.",
        ),
        "audio_channels" => pick(
            "Stereo for headphones or speakers. 5.1 and 7.1 keep surround sound (MKV only).",
            "Stéréo pour un casque ou des enceintes. 5.1 et 7.1 gardent le son surround (MKV seulement).",
        ),
        "replay_enabled" => pick(
            "Vixeeny keeps the last minutes of play in memory. A shortcut saves them as a video, so nothing is missed. Only in full screen.",
            "Vixeeny garde en mémoire les dernières minutes de jeu. Un raccourci les enregistre en vidéo, pour ne rien rater. Actif seulement en plein écran.",
        ),
        "replay_after" => pick(
            "The replay keeps recording a few seconds after the shortcut, to have the end of the action.",
            "Le replay continue d'enregistrer quelques secondes après le raccourci, pour avoir la fin de l'action.",
        ),
        "replay_storage" => pick(
            "Memory: spares the disk, but takes some from the game. Disk: for long replays or PCs with little memory. Automatic chooses.",
            "Mémoire : n'use pas le disque, mais en prend au jeu. Disque : pour les longs replays ou les PC avec peu de mémoire. Automatique choisit.",
        ),
        _ => None,
    }
}
