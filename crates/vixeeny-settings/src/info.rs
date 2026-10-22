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
            "The name given to each file.\n{app}: the game being played (or Desktop).\n{date}: the day of the capture.\n{time}: the time of the capture.",
            "Le nom donné à chaque fichier.\n{app} : le jeu en cours (ou Desktop).\n{date} : le jour de la capture.\n{time} : l'heure de la capture.",
        );
    }
    match key {
        "webp_lossless" | "jxl_lossless" => pick(
            "Keeps the exact picture, pixel for pixel.\nLarger files.",
            "Garde l'image exacte, pixel par pixel.\nFichiers plus gros.",
        ),
        "avif_speed" => pick(
            "Higher: faster saving, larger file.",
            "Plus haut : enregistrement plus rapide, fichier plus gros.",
        ),
        "webp_effort" | "jxl_effort" => pick(
            "Higher: smaller file at the same quality, longer saving.",
            "Plus haut : fichier plus petit à qualité égale, enregistrement plus long.",
        ),
        "avif_depth" => pick(
            "10 bits avoids bands in gradients.\nNeeded for HDR.",
            "10 bits évite les bandes dans les dégradés.\nNécessaire pour le HDR.",
        ),
        "window_backdrop" => pick(
            "Window captures only: a margin, rounded corners and a shadow, on this background.",
            "Captures de fenêtre seulement : une marge, des coins arrondis et une ombre, sur ce fond.",
        ),
        "image_hdr" if config.image.format == "webp" => pick(
            "WebP cannot keep HDR: its pictures are always converted.\nPNG, JPEG, AVIF and JPEG XL can keep it.",
            "Le WebP ne peut pas garder le HDR : ses images sont toujours converties.\nPNG, JPEG, AVIF et JPEG XL peuvent le garder.",
        ),
        "image_hdr" | "hdr" => pick(
            "Keeps the HDR colours and brightness of the screen.\nOff: the picture is converted to show right everywhere.",
            "Garde les couleurs et la luminosité HDR de l'écran.\nDésactivé : l'image est convertie pour s'afficher correctement partout.",
        ),
        "encoder_kind" => pick(
            "Hardware: encoded by the video chip of the graphics card.\nSoftware: encoded by the processor, which the game also uses.",
            "Matériel : encodé par la puce vidéo de la carte graphique.\nLogiciel : encodé par le processeur, que le jeu utilise aussi.",
        ),
        "encoder" => pick(
            "AV1: smaller files at the same quality, played by recent devices.\nHEVC: in between.\nH.264: larger files, played by nearly every device.",
            "AV1 : fichiers plus petits à qualité égale, lecture plus récente.\nHEVC : intermédiaire.\nH.264 : fichiers plus gros, lu par presque tous les appareils.",
        ),
        "preset" => pick(
            "Quality: for editing and keeping; large files.\nBalanced: in between.\nCompact: for sharing; light files.",
            "Qualité : pour le montage et l'archivage ; fichiers lourds.\nÉquilibré : entre les deux.\nCompact : pour le partage ; fichiers légers.",
        ),
        "container" => pick(
            "MP4: the most common format; still plays after a crash.\nMKV: takes every sound format.",
            "MP4 : format le plus répandu ; reste lisible après un plantage.\nMKV : accepte tous les formats de son.",
        ),
        "chroma" => pick(
            "4:2:0: the standard format.\n4:4:4: more precise colours on fine text; heavier file, few players support it.",
            "4:2:0 : format standard.\n4:4:4 : couleurs plus précises sur le texte fin ; fichier plus lourd, peu de lecteurs le prennent en charge.",
        ),
        "p:rc.mode" => {
            // The quality mode by the encoder's own name: CRF, CQ, ICQ, CQP.
            let name = encoders::resolved(env, &config.video)
                .and_then(|e| encoders::spec(&e.id))
                .map_or_else(|| "CQ".to_owned(), |e| e.rate_control.quality.name.clone());
            Some(if fr {
                format!(
                    "{name} : qualité constante, la taille varie selon l'image.\nVBR : débit moyen visé.\nCBR : débit fixe, pour le streaming."
                )
            } else {
                format!(
                    "{name}: constant quality, the size follows the picture.\nVBR: an average bitrate is aimed at.\nCBR: a fixed bitrate, for streaming."
                )
            })
        }
        "p:rc.quality" => {
            let encoder =
                encoders::resolved(env, &config.video).and_then(|e| encoders::spec(&e.id));
            let default = encoder.map(|e| e.rate_control.quality.default);
            let base = pick(
                "Lower: better quality, larger files.",
                "Plus bas : meilleure qualité, fichiers plus gros.",
            )?;
            Some(match default {
                Some(d) if fr => format!("{base}\n\nRecommandé : {d}."),
                Some(d) => format!("{base}\n\nRecommended: {d}."),
                None => base,
            })
        }
        "p:rc.bitrate" => pick(
            "Data per second of video.\n1080p60: 10,000 to 20,000 kbit/s.\n4K60: 40,000 to 60,000 kbit/s.",
            "Données par seconde de vidéo.\n1080p60 : 10 000 à 20 000 kbit/s.\n4K60 : 40 000 à 60 000 kbit/s.",
        ),
        "p:rc.maxrate" => pick(
            "The ceiling for very busy scenes.\n\nRecommended: 1.5 × the bitrate.",
            "Le plafond pour les scènes très animées.\n\nRecommandé : 1,5 × le débit.",
        ),
        "p:preset" | "p:quality" | "p:deadline" | "p:cpu-used" => {
            let encoder =
                encoders::resolved(env, &config.video).and_then(|e| encoders::spec(&e.id));
            let nvenc = encoder.is_some_and(|e| e.id.starts_with("nvenc"));
            let lang = env.lang;
            // What keeps up with these settings (resolution × frame rate) on this screen.
            let recommended = encoder
                .and_then(|e| {
                    let tools = env.probe.as_ref().and_then(|p| p.tools(&e.id));
                    e.speed_for(crate::pages::pixel_rate(env, config), tools)
                })
                .filter(|(k, _)| format!("p:{k}") == key)
                .map(|(k, v)| encoders::value_label(k, &v, lang));
            let mut text = pick(
                "Slower: better picture at the same size, more computing.\nIf the encoder cannot keep up, frames are lost.",
                "Plus lent : meilleure image à taille égale, plus de calcul.\nSi l'encodeur ne suit pas, des images sont perdues.",
            )?;
            if nvenc {
                text.push_str(if fr {
                    "\n\nP6 et P7 : 2 à 3 fois le calcul de P5, gain très faible.\nP1 et P2 : seuls presets pour la 4K à 120 FPS sur les cartes à plusieurs encodeurs."
                } else {
                    "\n\nP6 and P7: 2 to 3 times the computing of P5, very small gain.\nP1 and P2: the only presets for 4K at 120 FPS on cards with several encoders."
                });
            }
            if let Some(r) = recommended {
                text.push_str(&if fr {
                    format!("\n\nRecommandé ici : {r}.")
                } else {
                    format!("\n\nRecommended here: {r}.")
                });
            }
            Some(text)
        }
        "p:multipass" => pick(
            "Two passes per frame: quality better spread, more computing.",
            "Deux passes par image : qualité mieux répartie, plus de calcul.",
        ),
        "p:lookahead" if nvenc(env, config) => pick(
            "Analyses the next frames to spread the quality and place the B-frames.\nUses graphics memory.\nAuto: 8 frames.",
            "Analyse les images suivantes pour répartir la qualité et placer les images B.\nUtilise de la mémoire vidéo.\nAuto : 8 images.",
        ),
        "p:lookahead" => pick(
            "Analyses the next frames to spread the quality.\nUses graphics memory.",
            "Analyse les images suivantes pour répartir la qualité.\nUtilise de la mémoire vidéo.",
        ),
        "p:spatial-aq" | "p:vbaq" => pick(
            "Gives more quality to flat areas (skies, walls).",
            "Donne plus de qualité aux zones unies (ciel, murs).",
        ),
        "p:temporal-aq" => pick(
            "Gives more quality to areas that stay still from one frame to the next.",
            "Donne plus de qualité aux zones immobiles d'une image à l'autre.",
        ),
        "p:preanalysis" => pick(
            "Analyses each image before encoding.\nA slight gain, about 3 times the computing.",
            "Analyse chaque image avant l'encodage.\nGain léger, environ 3 fois plus de calcul.",
        ),
        "p:preencode" => pick(
            "A quick first pass over each image to spread the bitrate.\nA slight gain, a little more computing.",
            "Une première passe rapide sur chaque image pour répartir le débit.\nGain léger, un peu plus de calcul.",
        ),
        "p:motion-boost" => pick(
            "Fewer blocks in fast movements (camera turns, explosions).",
            "Moins de blocs dans les mouvements rapides (rotations de caméra, explosions).",
        ),
        "p:bframes" if nvenc(env, config) => pick(
            "Frames deduced from their neighbours: smaller files, more computing.\nAuto: up to 4, set scene by scene by the look-ahead (2 without it).",
            "Images déduites de leurs voisines : fichiers plus petits, plus de calcul.\nAuto : jusqu'à 4, choisies scène par scène par l'anticipation (2 sans elle).",
        ),
        "p:bframes" if amf(env, config) => pick(
            "Frames deduced from their neighbours: smaller files, more computing.\nAuto: 2 when the encoder has speed to spare for these settings, else none.",
            "Images déduites de leurs voisines : fichiers plus petits, plus de calcul.\nAuto : 2 si l'encodeur a de la marge pour ces réglages, sinon aucune.",
        ),
        "p:bframes" => pick(
            "Frames deduced from their neighbours: smaller files, more computing.",
            "Images déduites de leurs voisines : fichiers plus petits, plus de calcul.",
        ),
        "p:keyint" => pick(
            "The gap between two complete frames.\nShorter: more precise seeking in the video, heavier file.\nAuto: one every 2 seconds.",
            "Écart entre deux images complètes.\nPlus court : déplacement plus précis dans la vidéo, fichier plus lourd.\nAuto : une toutes les 2 secondes.",
        ),
        "audio_routing" => pick(
            "One track per source: separate tracks in Premiere Pro or DaVinci Resolve; a player such as VLC plays one of them only.\nEverything on one track: a single mix, heard the same everywhere.",
            "Une piste par source : des pistes séparées dans Premiere Pro ou DaVinci Resolve ; un lecteur comme VLC n'en lit qu'une.\nTout sur une piste : un seul mélange, entendu pareil partout.",
        ),
        "p:tune" => pick(
            "Adjusts the encoder to a kind of picture (film, animation, grain…).\nAuto: the encoder's default.\nPSNR, SSIM: for measurements; the picture looks flatter.",
            "Adapte l'encodeur à un type d'image (film, animation, grain…).\nAuto : le réglage par défaut de l'encodeur.\nPSNR, SSIM : pour les mesures ; l'image paraît plus plate.",
        ),
        "auto_update" => pick(
            "Checks when the PC starts and each time it wakes up.\nA new version is downloaded in the background and installed at the next start.",
            "Vérifie au démarrage du PC et à chaque sortie de veille.\nUne nouvelle version est téléchargée en arrière-plan et installée au prochain démarrage.",
        ),
        "replay_enabled" => pick(
            "Keeps the last minutes of play in memory.\nThe shortcut saves them as a video.\nWorks only while a game is in full screen.",
            "Garde en mémoire les dernières minutes de jeu.\nLe raccourci les enregistre en vidéo.\nFonctionne seulement pendant un jeu en plein écran.",
        ),
        "replay_after" => pick(
            "Seconds recorded after the shortcut is pressed.",
            "Secondes enregistrées après l'appui sur le raccourci.",
        ),
        "replay_storage" => pick(
            "Memory: uses RAM, nothing is written to the disk.\nDisk: writes to the disk continuously, little RAM used.\nAuto: chosen from the length and the PC's memory.",
            "Mémoire : utilise de la mémoire vive, rien n'est écrit sur le disque.\nDisque : écrit en continu sur le disque, peu de mémoire vive utilisée.\nAuto : choisit selon la durée et la mémoire du PC.",
        ),
        _ => None,
    }
}

/// Whether the video settings record with an NVIDIA encoder.
fn nvenc(env: &Env, config: &Config) -> bool {
    encoders::resolved(env, &config.video).is_some_and(|e| e.id.starts_with("nvenc"))
}

fn amf(env: &Env, config: &Config) -> bool {
    encoders::resolved(env, &config.video).is_some_and(|e| e.id.starts_with("amf"))
}
