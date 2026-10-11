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
            "Higher saves faster, with a larger file.",
            "Plus haut = enregistrement plus rapide, fichier plus gros.",
        ),
        "webp_effort" | "jxl_effort" => pick(
            "More effort makes a smaller file at the same quality.\nSaving takes longer.",
            "Plus d'effort donne un fichier plus petit, à qualité égale.\nL'enregistrement est plus long.",
        ),
        "avif_depth" => pick(
            "10 bits avoids bands in gradients.\nNeeded for HDR.",
            "10 bits évite les bandes dans les dégradés.\nNécessaire pour le HDR.",
        ),
        "image_hdr" | "hdr" => pick(
            "Keeps the HDR colours and brightness of the screen.\nOff: the picture is converted to show right everywhere.",
            "Garde les couleurs et la luminosité HDR de l'écran.\nDésactivé : l'image est convertie pour s'afficher correctement partout.",
        ),
        "encoder_kind" => pick(
            "Hardware: the graphics card encodes, the game does not slow down.\nSoftware: the processor encodes, which can slow the game.",
            "Matériel : la carte graphique encode, le jeu ne ralentit pas.\nLogiciel : le processeur encode, ce qui peut ralentir le jeu.",
        ),
        "encoder" => pick(
            "AV1: the lightest files.\nHEVC: a little heavier.\nH.264: plays everywhere, even on old devices.",
            "AV1 : les fichiers les plus légers.\nHEVC : un peu plus lourds.\nH.264 : lu partout, même sur les vieux appareils.",
        ),
        "container" => pick(
            "The kind of video file.\nMP4: plays everywhere; a video cut by a crash still plays.\nMKV: takes every sound format.",
            "Le type de fichier vidéo.\nMP4 : lu partout ; une vidéo coupée par un plantage reste lisible.\nMKV : accepte tous les formats de son.",
        ),
        "chroma" => pick(
            "4:2:0: plays everywhere.\n4:4:4: more precise colours on fine text, but rarely read and heavier.",
            "4:2:0 : lu partout.\n4:4:4 : couleurs plus précises sur le texte fin, mais rarement lu et plus lourd.",
        ),
        "p:rc.mode" => pick(
            "Constant quality: the quality stays the same, the file size varies.\nConstant QP: simpler, heavier.\nVBR: aims at a size.\nCBR: a fixed size, for streaming.",
            "Qualité constante : la qualité reste la même, la taille du fichier varie.\nQP constant : plus simple, plus lourd.\nVBR : vise une taille.\nCBR : taille fixe, pour le streaming.",
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
                "Lower is better quality, but much larger files.",
                "Plus bas = meilleure qualité, mais fichiers beaucoup plus volumineux.",
            )?;
            Some(match default {
                Some(d) if fr => format!("{base}\n\nRecommandé : {d}."),
                Some(d) => format!("{base}\n\nRecommended: {d}."),
                None => base,
            })
        }
        "p:rc.bitrate" => pick(
            "The amount of data per second: higher is sharper and heavier.\n1080p60: 10 000 to 20 000.\n4K60: 40 000 to 60 000.",
            "La quantité de données par seconde : plus haut = plus net et plus lourd.\n1080p60 : 10 000 à 20 000.\n4K60 : 40 000 à 60 000.",
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
                .and_then(|e| e.speed_for(crate::pages::pixel_rate(env, config)))
                .filter(|(k, _)| format!("p:{k}") == key)
                .map(|(k, v)| encoders::value_label(k, &v, lang));
            let mut text = pick(
                "Slower is a better picture for the same size, but more computing.\nToo slow and frames are skipped.",
                "Plus lent = meilleure image pour la même taille, mais plus de calcul.\nTrop lent, des images sautent.",
            )?;
            if nvenc {
                text.push_str(if fr {
                    "\n\nP6 et P7 : gain à peine visible, pour beaucoup plus de calcul. En 4K ou au-delà de 60 FPS, l'encodeur ne suit plus.\nP1 et P2 : partagent chaque image entre les encodeurs des grandes cartes, pour la 4K à haute fréquence."
                } else {
                    "\n\nP6 and P7: a barely visible gain for much more computing. In 4K or above 60 FPS, the encoder falls behind.\nP1 and P2: share each frame between the encoders of large cards, for 4K at a high rate."
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
            "Looks quickly at each image before encoding it, to spread the quality better.",
            "Analyse rapidement chaque image avant de l'encoder, pour mieux répartir la qualité.",
        ),
        "p:lookahead" => pick(
            "Looks at the next images to spread the quality better.\nUses graphics memory.",
            "Regarde les images suivantes pour mieux répartir la qualité.\nUtilise de la mémoire vidéo.",
        ),
        "p:spatial-aq" | "p:temporal-aq" | "p:vbaq" => pick(
            "Improves flat areas (skies, walls), where flaws show most.",
            "Améliore les zones unies (ciel, murs), où les défauts se voient le plus.",
        ),
        "p:preanalysis" => pick(
            "Analyses each image before encoding.\nA slight gain, more computing.",
            "Analyse chaque image avant l'encodage.\nGain léger, plus de calcul.",
        ),
        "p:bframes" => pick(
            "Images rebuilt from their neighbours: lighter files.",
            "Images reconstruites à partir de leurs voisines : fichiers plus légers.",
        ),
        "p:keyint" => pick(
            "How often a complete image is stored.\nMore often: smoother seeking, heavier file.\n0: automatic.",
            "Tous les combien une image complète est enregistrée.\nPlus fréquent = avance rapide plus fluide, fichier plus lourd.\n0 : automatique.",
        ),
        "replay_enabled" => pick(
            "Vixeeny keeps the last minutes of play in memory.\nA shortcut saves them as a video, so nothing is missed.\nOnly in full screen.",
            "Vixeeny garde en mémoire les dernières minutes de jeu.\nUn raccourci les enregistre en vidéo, pour ne rien rater.\nActif seulement en plein écran.",
        ),
        "replay_after" => pick(
            "The replay keeps recording a few seconds after the shortcut, to have the end of the action.",
            "Le replay continue d'enregistrer quelques secondes après le raccourci, pour avoir la fin de l'action.",
        ),
        "replay_storage" => pick(
            "Memory: spares the disk, but takes some from the game.\nDisk: for long replays or PCs with little memory.\nAutomatic chooses.",
            "Mémoire : n'use pas le disque, mais en prend au jeu.\nDisque : pour les longs replays ou les PC avec peu de mémoire.\nAutomatique choisit.",
        ),
        _ => None,
    }
}
