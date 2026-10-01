// SPDX-License-Identifier: GPL-3.0-or-later
//! Round trips (CA-FMT-2): encode, decode, compare dimensions and, for lossless formats,
//! pixels.

use super::*;

/// A deterministic image with smooth and noisy areas; BGRA with stride padding.
fn sample(width: u32, height: u32) -> (Vec<u8>, usize) {
    let stride = width as usize * 4 + 8;
    let mut data = vec![0u8; stride * height as usize];
    let mut seed = 12345u32;
    for y in 0..height as usize {
        for x in 0..width as usize {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 24) as u8 / 8;
            let i = y * stride + x * 4;
            data[i..i + 4].copy_from_slice(&[
                (x * 255 / width as usize) as u8,
                (y * 255 / height as usize) as u8,
                noise.wrapping_add((x + y) as u8),
                255,
            ]);
        }
    }
    (data, stride)
}

fn rgb_of(data: &[u8], stride: usize, width: u32, height: u32) -> Vec<u8> {
    Bgra::new(width, height, stride, data).to_rgb()
}

fn decode(bytes: &[u8], format: image::ImageFormat) -> image::RgbImage {
    image::load_from_memory_with_format(bytes, format)
        .unwrap()
        .to_rgb8()
}

#[test]
fn png_is_lossless_and_oxipng_keeps_it_so() {
    let (data, stride) = sample(61, 37);
    let img = Bgra::new(61, 37, stride, &data);
    let want = rgb_of(&data, stride, 61, 37);
    for settings in [
        PngSettings::default(),
        PngSettings {
            compression: PngCompression::High,
            oxipng_level: Some(1),
        },
    ] {
        let bytes = encode(
            ImageFormat::Png,
            &img,
            &Settings {
                png: settings,
                ..Settings::default()
            },
        )
        .unwrap();
        assert!(bytes.windows(4).any(|w| w == b"sRGB"));
        let got = decode(&bytes, image::ImageFormat::Png);
        assert_eq!((got.width(), got.height()), (61, 37));
        assert_eq!(got.as_raw(), &want);
    }
}

#[test]
fn oxipng_never_grows_the_file() {
    let (data, stride) = sample(128, 128);
    let img = Bgra::new(128, 128, stride, &data);
    let plain = encode(ImageFormat::Png, &img, &Settings::default()).unwrap();
    let optimised = encode(
        ImageFormat::Png,
        &img,
        &Settings {
            png: PngSettings {
                compression: PngCompression::Fast,
                oxipng_level: Some(2),
            },
            ..Settings::default()
        },
    )
    .unwrap();
    assert!(optimised.len() <= plain.len());
}

#[test]
fn jpeg_has_right_size_icc_and_reasonable_error() {
    let (data, stride) = sample(64, 48);
    let img = Bgra::new(64, 48, stride, &data);
    let want = rgb_of(&data, stride, 64, 48);
    for chroma in [Chroma::Yuv444, Chroma::Yuv420] {
        let settings = Settings {
            jpeg: JpegSettings {
                quality: 95,
                chroma,
                progressive: chroma == Chroma::Yuv420,
            },
            ..Settings::default()
        };
        let bytes = encode(ImageFormat::Jpeg, &img, &settings).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        assert!(bytes.windows(11).any(|w| w == b"ICC_PROFILE"));
        let got = decode(&bytes, image::ImageFormat::Jpeg);
        assert_eq!((got.width(), got.height()), (64, 48));
        let err: u64 = got
            .as_raw()
            .iter()
            .zip(&want)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        let mean = err as f64 / want.len() as f64;
        let limit = if chroma == Chroma::Yuv444 { 6.0 } else { 14.0 }; // noisy test image
        assert!(mean < limit, "{chroma:?}: mean error {mean}");
    }
}

#[test]
fn webp_lossless_is_bit_exact_and_tagged() {
    let (data, stride) = sample(53, 41);
    let img = Bgra::new(53, 41, stride, &data);
    let bytes = encode(ImageFormat::WebP, &img, &Settings::default()).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..16], b"WEBPVP8X");
    assert!(bytes.windows(4).any(|w| w == b"ICCP"));
    let riff_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!(riff_len, bytes.len() - 8);
    let got = decode(&bytes, image::ImageFormat::WebP);
    assert_eq!((got.width(), got.height()), (53, 41));
    assert_eq!(got.as_raw(), &rgb_of(&data, stride, 53, 41));
}

#[test]
fn webp_lossy_decodes_to_the_same_size() {
    let (data, stride) = sample(40, 30);
    let img = Bgra::new(40, 30, stride, &data);
    let settings = Settings {
        webp: WebpSettings {
            lossless: false,
            quality: 80.0,
            effort: 2,
        },
        ..Settings::default()
    };
    let bytes = encode(ImageFormat::WebP, &img, &settings).unwrap();
    let got = decode(&bytes, image::ImageFormat::WebP);
    assert_eq!((got.width(), got.height()), (40, 30));
}

#[test]
fn bad_buffers_are_rejected() {
    assert!(
        encode(
            ImageFormat::Png,
            &Bgra::new(2, 2, 8, &[0; 15]),
            &Settings::default()
        )
        .is_err()
    );
    assert!(
        encode(
            ImageFormat::Jpeg,
            &Bgra::new(0, 2, 0, &[]),
            &Settings::default()
        )
        .is_err()
    );
}

#[test]
fn format_names() {
    assert_eq!(ImageFormat::from_name("PNG"), Some(ImageFormat::Png));
    assert_eq!(ImageFormat::from_name("jpg"), Some(ImageFormat::Jpeg));
    assert_eq!(ImageFormat::from_name("bmp"), None);
    assert_eq!(ImageFormat::WebP.extension(), "webp");
    assert_eq!(Chroma::from_name("420"), Some(Chroma::Yuv420));
}

#[cfg(feature = "native-codecs")]
mod native {
    use super::*;

    fn smooth(width: u32, height: u32) -> (Vec<u8>, usize) {
        let stride = width as usize * 4;
        let mut data = vec![255u8; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let i = y * stride + x * 4;
                data[i] = (x * 255 / width as usize) as u8;
                data[i + 1] = (y * 255 / height as usize) as u8;
                data[i + 2] = ((x + y) * 255 / (width + height) as usize) as u8;
            }
        }
        (data, stride)
    }

    #[test]
    fn avif_round_trip_in_both_depths_and_chromas() {
        let (data, stride) = smooth(96, 64);
        let img = Bgra::new(96, 64, stride, &data);
        let want = rgb_of(&data, stride, 96, 64);
        for (depth, chroma) in [
            (10, Chroma::Yuv444),
            (8, Chroma::Yuv444),
            (10, Chroma::Yuv420),
        ] {
            let settings = Settings {
                avif: AvifSettings {
                    quality: 90,
                    speed: 10,
                    depth,
                    chroma,
                },
                ..Settings::default()
            };
            let bytes = encode(ImageFormat::Avif, &img, &settings).unwrap();
            assert_eq!(&bytes[4..12], b"ftypavif");
            let (w, h, got) = crate::avif::decode_rgb(&bytes).unwrap();
            assert_eq!((w, h), (96, 64));
            let err: u64 = got
                .iter()
                .zip(&want)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            let mean = err as f64 / want.len() as f64;
            assert!(mean < 4.0, "{depth}-bit {chroma:?}: mean error {mean}");
        }
    }

    #[test]
    fn jxl_lossless_is_bit_exact() {
        let (data, stride) = sample(57, 43);
        let img = Bgra::new(57, 43, stride, &data);
        let bytes = encode(ImageFormat::Jxl, &img, &Settings::default()).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(b"\0\0\0\x0cJXL "));
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(&bytes))
            .unwrap();
        let render = image.render_frame(0).unwrap();
        let fb = render.image_all_channels();
        assert_eq!((fb.width(), fb.height()), (57, 43));
        let got: Vec<u8> = fb.buf().iter().map(|v| (v * 255.0).round() as u8).collect();
        assert_eq!(got, rgb_of(&data, stride, 57, 43));
    }

    #[test]
    fn jxl_lossy_is_smaller() {
        let (data, stride) = sample(128, 128);
        let img = Bgra::new(128, 128, stride, &data);
        let lossless = encode(ImageFormat::Jxl, &img, &Settings::default()).unwrap();
        let lossy = encode(
            ImageFormat::Jxl,
            &img,
            &Settings {
                jxl: JxlSettings {
                    lossless: false,
                    distance: 2.0,
                    effort: 3,
                },
                ..Settings::default()
            },
        )
        .unwrap();
        assert!(lossy.len() < lossless.len());
    }
}
