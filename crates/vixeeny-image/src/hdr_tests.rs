// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR kept as HDR: what the files say about their colours, that the highlights survive, and that their thumbnails look
//! like the SDR rendering.

use super::*;

/// SDR white on the captured screen.
const WHITE_NITS: f32 = 240.0;

/// An HDR desktop as scRGB (1.0 = 80 nits): the left half is SDR content (a grey ramp up to SDR white), the right half
/// a game: a 1000-nit sun on a dark sky, and a BT.2020 green outside sRGB.
fn scene(w: u32, h: u32) -> Vec<f32> {
    let white = WHITE_NITS / 80.0;
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let px = if x < w / 2 {
                let v = white * x as f32 / (w / 2 - 1) as f32;
                [v, v, v]
            } else if y < h / 2 {
                [1000.0 / 80.0; 3]
            } else if x < w * 3 / 4 {
                [0.02, 0.03, 0.08]
            } else {
                [-1.175, 2.266, -0.201]
            };
            rgba.extend_from_slice(&[px[0], px[1], px[2], 1.0]);
        }
    }
    rgba
}

fn hdr(rgba: &[f32], w: u32, h: u32) -> Hdr<'_> {
    Hdr { width: w, height: h, rgba, sdr_white_nits: WHITE_NITS, peak_nits: 1000.0 }
}

/// PQ signal (0…1) of `nits` grey.
fn pq_of(nits: f32) -> f32 {
    f32::from(vixeeny_common::colour::scrgb_to_pq([nits / 80.0; 3])[0]) / 65535.0
}

/// The 16-bit RGB samples of a PNG, as 0…1.
fn png_samples(bytes: &[u8]) -> (u32, Vec<[f32; 3]>) {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Rgb, png::BitDepth::Sixteen));
    let px = buf[..info.buffer_size()]
        .as_chunks::<6>()
        .0
        .iter()
        .map(|p| {
            let c = |i: usize| f32::from(u16::from_be_bytes([p[i], p[i + 1]])) / 65535.0;
            [c(0), c(2), c(4)]
        })
        .collect();
    (info.width, px)
}

fn bgra_at(d: &Decoded, x: u32, y: u32) -> [u8; 3] {
    let i = (y * d.width + x) as usize * 4;
    [d.bgra[i + 2], d.bgra[i + 1], d.bgra[i]]
}

#[test]
fn an_hdr_png_keeps_the_highlights_and_says_it_is_pq() {
    let (w, h) = (64, 32);
    let rgba = scene(w, h);
    for oxipng_level in [None, Some(2)] {
        let settings =
            Settings { png: PngSettings { compression: PngCompression::Fast, oxipng_level }, ..Settings::default() };
        let bytes = encode_hdr(ImageFormat::Png, &hdr(&rgba, w, h), &settings).unwrap();
        // BT.2020, PQ, RGB, full range; no sRGB chunk contradicting it.
        let at = bytes.windows(4).position(|c| c == b"cICP").unwrap();
        assert_eq!(&bytes[at + 4..at + 8], &[9, 16, 0, 1]);
        assert!(!bytes.windows(4).any(|c| c == b"sRGB"));
        // Its light and the screen's, in 0.0001 nits: the sun, rounded up, and a 1000-nit peak.
        let reader = png::Decoder::new(std::io::Cursor::new(&bytes)).read_info().unwrap();
        let info = reader.info();
        let cll = info.content_light_level.unwrap();
        assert!((10_000_000..=10_000_100).contains(&cll.max_content_light_level), "{cll:?}");
        assert!(cll.max_frame_average_light_level < cll.max_content_light_level);
        let screen = info.mastering_display_color_volume.unwrap();
        assert_eq!((screen.max_luminance, screen.min_luminance), (10_000_000, 1));
        assert_eq!(screen.chromaticities.green.1.into_scaled(), 79_700);
        let (width, px) = png_samples(&bytes);
        let at = |x: u32, y: u32| px[(y * width + x) as usize];
        // SDR white keeps its 240 nits, the sun its 1000: nothing is clipped at SDR white.
        let white = at(w / 2 - 1, 0);
        let sun = at(w - 1, 0);
        assert!((white[1] - pq_of(WHITE_NITS)).abs() < 2e-3, "{white:?}");
        assert!((sun[1] - pq_of(1000.0)).abs() < 2e-3, "{sun:?}");
        // The wide green stays the BT.2020 primary: red and blue under 0.1 nit (PQ is steep near black), green at 180.
        let green = at(w - 1, h - 1);
        assert!(green[1] > 0.5 && green[0] < pq_of(0.1) && green[2] < pq_of(0.1), "{green:?}");
    }
}

#[test]
fn the_light_is_the_brightest_channel_and_its_mean() {
    // Half a 1000-nit white, half black; a stray NaN counts as black.
    let mut rgba: Vec<f32> = (0..8)
        .flat_map(|i| {
            let v = if i < 4 { 1000.0 / 80.0 } else { 0.0 };
            [v, v, v, 1.0]
        })
        .collect();
    rgba[4 * 7] = f32::NAN;
    let light = hdr(&rgba, 4, 2).light();
    assert!((light.max_cll - 1000.0).abs() < 0.1, "{light:?}");
    assert!((light.max_fall - 500.0).abs() < 0.1, "{light:?}");
    assert_eq!(light.peak, 1000.0);
}

#[test]
fn hdr_thumbnails_look_like_the_sdr_rendering() {
    let (w, h) = (64, 32);
    let rgba = scene(w, h);
    let png = encode_hdr(ImageFormat::Png, &hdr(&rgba, w, h), &Settings::default()).unwrap();
    let d = crate::decode(&png).unwrap();
    assert_eq!((d.width, d.height), (w, h));
    let white = bgra_at(&d, w / 2 - 1, 0);
    let black = bgra_at(&d, 0, 0);
    let sun = bgra_at(&d, w - 1, 0);
    let sky = bgra_at(&d, w / 2 + 2, h - 1);
    // Not the washed-out grey of PQ codes shown as sRGB: black is black, white is near white, the sun is the brightest,
    // the sky stays dark.
    assert_eq!(black, [0, 0, 0]);
    assert!(white.iter().all(|&c| c > 225), "{white:?}");
    assert!(sun.iter().all(|&c| c >= white[0]), "{sun:?}");
    assert!(sky.iter().all(|&c| c < 80), "{sky:?}");
}

#[test]
fn an_hdr_avif_is_tagged_bt2100_pq_and_reads_back_like_the_png() {
    let (w, h) = (64, 32);
    let rgba = scene(w, h);
    let settings = Settings { avif: AvifSettings { quality: 95, ..AvifSettings::default() }, ..Settings::default() };
    let avif = encode_hdr(ImageFormat::Avif, &hdr(&rgba, w, h), &settings).unwrap();
    let at = avif.windows(4).position(|c| c == b"colr").unwrap();
    assert_eq!(&avif[at + 4..at + 8], b"nclx");
    // Primaries 9 (BT.2020), transfer 16 (PQ), matrix 9 (BT.2020 NCL), full range.
    assert_eq!(&avif[at + 8..at + 15], &[0, 9, 0, 16, 0, 9, 0x80]);
    // Its light in nits (the sun) and the screen's in 0.0001 nits...
    let at = avif.windows(4).position(|c| c == b"clli").unwrap();
    let cll = u16::from_be_bytes([avif[at + 4], avif[at + 5]]);
    assert!((1000..=1001).contains(&cll), "{cll}");
    let at = avif.windows(4).position(|c| c == b"mdcv").unwrap();
    let nits = |i: usize| u32::from_be_bytes(avif[at + i..at + i + 4].try_into().unwrap());
    assert_eq!((nits(20), nits(24)), (10_000_000, 1));
    // ... given to the picture: after its size, depth, AV1 configuration and colours.
    let at = avif.windows(4).position(|c| c == b"ipma").unwrap();
    assert_eq!(&avif[at + 12..at + 21], &[0, 1, 6, 1, 2, 0x83, 4, 5, 6]);
    // The YCbCr conversion is Vixeeny's and the decoder's is FFmpeg's: the same thumbnail as the PNG (lossless) says
    // both use the same matrix.
    let from_avif = crate::decode(&avif).unwrap();
    let png = encode_hdr(ImageFormat::Png, &hdr(&rgba, w, h), &Settings::default()).unwrap();
    let from_png = crate::decode(&png).unwrap();
    // Away from edges (4:2:0 chroma) and the ramp's steep end.
    for (x, y) in [(8, 8), (w / 4, 20), (w - 6, 6), (w / 2 + 6, h - 6), (w - 6, h - 6)] {
        let (a, b) = (bgra_at(&from_avif, x, y), bgra_at(&from_png, x, y));
        for (p, q) in a.iter().zip(b) {
            assert!(p.abs_diff(q) <= 8, "({x}, {y}): avif {a:?} png {b:?}");
        }
    }
}

#[test]
fn webp_cannot_keep_hdr() {
    let rgba = scene(8, 8);
    assert!(!ImageFormat::WebP.holds_hdr());
    assert!(matches!(
        encode_hdr(ImageFormat::WebP, &hdr(&rgba, 8, 8), &Settings::default()),
        Err(ImageError::Unavailable(ImageFormat::WebP))
    ));
}

#[cfg(feature = "native-codecs")]
mod native {
    use super::*;

    #[test]
    fn an_hdr_jpeg_xl_is_pq_and_keeps_the_highlights() {
        use jxl_oxide::JxlImage;
        let (w, h) = (64, 32);
        let rgba = scene(w, h);
        let bytes = encode_hdr(ImageFormat::Jxl, &hdr(&rgba, w, h), &Settings::default()).unwrap();
        let image = JxlImage::builder().read(std::io::Cursor::new(&bytes)).unwrap();
        // Tone mapped from the sun rather than PQ's 10 000 nits.
        let target = image.image_header().metadata.tone_mapping.intensity_target;
        assert!((target - 1000.0).abs() < 1.0, "{target}");
        let encoding = format!("{:?}", image.image_header().metadata.colour_encoding);
        assert!(encoding.contains("Pq") && encoding.contains("Bt2100"), "{encoding}");
        // Lossless: the samples are the PQ codes, the sun above SDR white.
        let render = image.render_frame(0).unwrap();
        let fb = render.image_all_channels();
        let px = |x: usize, y: usize| fb.buf()[(y * w as usize + x) * fb.channels() + 1];
        assert!((px(w as usize / 2 - 1, 0) - pq_of(WHITE_NITS)).abs() < 2e-3);
        assert!((px(w as usize - 1, 0) - pq_of(1000.0)).abs() < 2e-3);
    }

    #[test]
    fn an_ultra_hdr_jpeg_is_the_sdr_picture_plus_a_gain_map() {
        let (w, h) = (64, 32);
        let rgba = scene(w, h);
        let image = hdr(&rgba, w, h);
        let bytes = encode_hdr(ImageFormat::Jpeg, &image, &Settings::default()).unwrap();
        // Two JPEG streams tied by an MPF index, the gain map described by ISO 21496-1.
        assert!(bytes.windows(4).any(|c| c == b"MPF\0"));
        let iso = b"urn:iso:std:iso:ts:21496:-1";
        assert!(bytes.windows(iso.len()).any(|c| c == iso));
        assert!(bytes.windows(3).filter(|c| *c == [0xff, 0xd8, 0xff]).count() >= 2);
        // Every viewer shows the primary picture: the SDR rendering.
        let primary = crate::decode(&bytes).unwrap();
        let sdr = image.to_sdr();
        for (x, y) in [(8, 8), (w / 2 - 2, 4), (w - 6, 6), (w / 2 + 6, h - 6)] {
            let got = bgra_at(&primary, x, y);
            let i = (y * w + x) as usize * 4;
            let want = [sdr[i + 2], sdr[i + 1], sdr[i]];
            for (p, q) in got.iter().zip(want) {
                assert!(p.abs_diff(q) <= 6, "({x}, {y}): {got:?} vs {want:?}");
            }
        }
        // An HDR screen gets the sun back, and the green as the BT.2020 primary, not the sRGB green it is on the SDR
        // picture.
        let (cg, at) = ultra_hdr_rendition(&bytes);
        assert_eq!(cg, crate::native::uhdr_color_gamut_UHDR_CG_BT_2100);
        let sun = at(w - 6, 6);
        assert!((sun[1] - 1000.0).abs() < 100.0, "{sun:?}");
        let green = at(w - 6, h - 6);
        assert!((green[1] - 160.0).abs() < 25.0, "{green:?}");
        assert!(green[0] < 8.0 && green[2] < 8.0, "{green:?}");
    }

    /// The HDR picture libultrahdr rebuilds from an Ultra HDR JPEG for a 1000-nit screen: its gamut, and its linear RGB
    /// in nits at a pixel.
    fn ultra_hdr_rendition(bytes: &[u8]) -> (crate::native::uhdr_color_gamut_t, impl Fn(u32, u32) -> [f32; 3]) {
        use crate::native as ffi;
        // SAFETY: the decoder is released below, after its picture is copied out; the input outlives the decode.
        unsafe {
            let dec = ffi::uhdr_create_decoder();
            let mut input = ffi::uhdr_compressed_image_t {
                data: bytes.as_ptr().cast_mut().cast(),
                data_sz: bytes.len(),
                capacity: bytes.len(),
                cg: ffi::uhdr_color_gamut_UHDR_CG_UNSPECIFIED,
                ct: ffi::uhdr_color_transfer_UHDR_CT_UNSPECIFIED,
                range: ffi::uhdr_color_range_UHDR_CR_UNSPECIFIED,
            };
            let ok = |e: ffi::uhdr_error_info_t| {
                assert_eq!(e.error_code, ffi::uhdr_codec_err_UHDR_CODEC_OK);
            };
            ok(ffi::uhdr_dec_set_image(dec, &raw mut input));
            ok(ffi::uhdr_dec_set_out_img_format(dec, ffi::uhdr_img_fmt_UHDR_IMG_FMT_64bppRGBAHalfFloat));
            ok(ffi::uhdr_dec_set_out_color_transfer(dec, ffi::uhdr_color_transfer_UHDR_CT_LINEAR));
            ok(ffi::uhdr_dec_set_out_max_display_boost(dec, 1000.0 / 203.0));
            ok(ffi::uhdr_decode(dec));
            let out = &*ffi::uhdr_get_decoded_image(dec);
            let stride = out.stride[0] as usize;
            let halves = std::slice::from_raw_parts(out.planes[0].cast::<u16>(), stride * out.h as usize * 4).to_vec();
            let cg = out.cg;
            ffi::uhdr_release_decoder(dec);
            // 1.0 is 203 nits.
            let at = move |x: u32, y: u32| {
                let i = (y as usize * stride + x as usize) * 4;
                [0, 1, 2].map(|c| vixeeny_common::half::to_f32(halves[i + c]) * 203.0)
            };
            (cg, at)
        }
    }
}
