// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel layouts. Tracks are mono-free: 2 (stereo), 6 (5.1) or 8 (7.1) interleaved channels,
//! in the order Windows and FFmpeg both use: FL FR FC LFE BL BR (SL SR).

use std::borrow::Cow;

/// Gain of the centre and surround channels when folded into the front pair (-3 dB).
const MIX: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The channel count a track uses for a source of `native` channels, at most `max`: stereo,
/// 5.1 or 7.1 (anything else is not worth a track of its own).
pub fn track_channels(native: usize, max: usize) -> usize {
    let n = native.min(max);
    if n >= 8 {
        8
    } else if n >= 6 {
        6
    } else {
        2
    }
}

/// `samples` (interleaved, `from` channels) as `to` channels.
pub fn convert(samples: &[f32], from: usize, to: usize) -> Cow<'_, [f32]> {
    if from == to || from == 0 || to == 0 {
        return Cow::Borrowed(samples);
    }
    let frames = samples.len() / from;
    let mut out = vec![0.0f32; frames * to];
    for (src, dst) in samples.chunks_exact(from).zip(out.chunks_exact_mut(to)) {
        match (from, to) {
            (1, _) => {
                dst[0] = src[0];
                if to > 1 {
                    dst[1] = src[0];
                }
            }
            (_, 1) => dst[0] = front(src).iter().sum::<f32>() / 2.0,
            (6 | 8, 2) => dst.copy_from_slice(&front(src)),
            (6, 8) => {
                dst[..6].copy_from_slice(src);
            }
            (8, 6) => {
                dst[..4].copy_from_slice(&src[..4]);
                dst[4] = src[4] + MIX * src[6];
                dst[5] = src[5] + MIX * src[7];
            }
            // Stereo into surround: the front pair only.
            (2, _) => dst[..2].copy_from_slice(src),
            // Anything else: the channels both have.
            _ => {
                let common = from.min(to);
                dst[..common].copy_from_slice(&src[..common]);
            }
        }
    }
    Cow::Owned(out)
}

/// One frame of 2, 6 or 8 channels as a left/right pair.
fn front(frame: &[f32]) -> [f32; 2] {
    match frame.len() {
        n if n >= 6 => {
            let sides = if n >= 8 { 1.0 } else { 0.0 };
            let norm = 1.0 / (1.0 + MIX + MIX + MIX * sides);
            let side = |i: usize| if n >= 8 { frame[i] } else { 0.0 };
            [
                (frame[0] + MIX * frame[2] + MIX * frame[4] + MIX * side(6)) * norm,
                (frame[1] + MIX * frame[2] + MIX * frame[5] + MIX * side(7)) * norm,
            ]
        }
        _ => [frame[0], frame.get(1).copied().unwrap_or(frame[0])],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_are_stereo_5_1_or_7_1() {
        assert_eq!(track_channels(1, 8), 2);
        assert_eq!(track_channels(2, 8), 2);
        assert_eq!(track_channels(6, 8), 6);
        assert_eq!(track_channels(8, 8), 8);
        assert_eq!(track_channels(8, 6), 6);
        assert_eq!(track_channels(8, 2), 2);
        assert_eq!(track_channels(4, 8), 2);
    }

    #[test]
    fn same_layout_is_not_copied() {
        let s = [0.1, 0.2, 0.3, 0.4];
        assert!(matches!(convert(&s, 2, 2), Cow::Borrowed(_)));
    }

    #[test]
    fn stereo_and_mono_go_to_the_front_pair_of_a_surround_track() {
        let out = convert(&[0.5, -0.5], 2, 6);
        assert_eq!(&out[..], &[0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        let out = convert(&[0.25], 1, 8);
        assert_eq!(&out[..], &[0.25, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&convert(&[0.25], 1, 2)[..], &[0.25, 0.25]);
    }

    #[test]
    fn a_surround_mix_folds_into_stereo_without_clipping() {
        // Everything at full scale: the fold-down stays within -1…1.
        let loud = convert(&[1.0; 8], 8, 2);
        assert!(loud.iter().all(|v| *v <= 1.0 + 1e-6), "{loud:?}");
        // The centre alone lands equally on both sides; the LFE is dropped.
        let centre = convert(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 6, 2);
        assert!((centre[0] - centre[1]).abs() < 1e-6 && centre[0] > 0.2);
        let lfe = convert(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], 6, 2);
        assert_eq!(&lfe[..], &[0.0, 0.0]);
        // A left-only signal stays on the left.
        let left = convert(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 8, 2);
        assert!(left[0] > 0.2 && left[1] == 0.0);
    }

    #[test]
    fn seven_one_and_five_one_convert_both_ways() {
        let seven = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.2, 0.2];
        let five = convert(&seven, 8, 6);
        assert_eq!(&five[..4], &seven[..4]);
        assert!((five[4] - (0.5 + MIX * 0.2)).abs() < 1e-6);
        let back = convert(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 6, 8);
        assert_eq!(&back[..], &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.0, 0.0]);
    }
}
