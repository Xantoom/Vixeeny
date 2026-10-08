// SPDX-License-Identifier: GPL-3.0-or-later
//! CA-CONV-2 and the readers: orientation, colour profiles, transparency, every format.

use crate::{Bgra, ImageFormat, Settings, SourceFormat, decode, encode};

/// 4×2 image whose pixels are all different (BGRA).
fn tagged() -> Vec<u8> {
    (0..8u8)
        .flat_map(|i| [i * 30, 255 - i * 20, i * 7 + 3, 255])
        .collect()
}

fn png_bytes(rgb_like: &[u8], w: u32, h: u32) -> Vec<u8> {
    encode(
        ImageFormat::Png,
        &Bgra::new(w, h, w as usize * 4, rgb_like),
        &Settings::default(),
    )
    .unwrap()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Inserts a chunk right after IHDR.
fn with_chunk(png: &[u8], kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let at = 8 + 12 + 13; // signature + IHDR chunk
    let mut chunk = (body.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(body);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(body);
    chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    [&png[..at], &chunk, &png[at..]].concat()
}

fn exif(orientation: u16) -> Vec<u8> {
    let mut v = b"MM\0*\0\0\0\x08".to_vec();
    v.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1]);
    v.extend_from_slice(&orientation.to_be_bytes());
    v.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    v
}

fn px(d: &crate::Decoded, x: u32, y: u32) -> [u8; 4] {
    let i = (y * d.width + x) as usize * 4;
    d.bgra[i..i + 4].try_into().unwrap()
}

#[test]
fn sniffing() {
    assert_eq!(
        SourceFormat::sniff(&png_bytes(&tagged(), 4, 2)),
        Some(SourceFormat::Png)
    );
    assert_eq!(SourceFormat::sniff(b"hello"), None);
    assert!(decode(b"not an image").is_err());
}

#[test]
fn png_round_trip_is_exact() {
    let src = tagged();
    let d = decode(&png_bytes(&src, 4, 2)).unwrap();
    assert_eq!((d.width, d.height), (4, 2));
    assert_eq!(d.bgra, src);
}

#[test]
fn exif_orientation_is_applied() {
    let src = tagged();
    let base = png_bytes(&src, 4, 2);
    // 1 = as is, 3 = rotated 180°, 6 = needs 90° clockwise (dimensions swap).
    let d1 = decode(&with_chunk(&base, b"eXIf", &exif(1))).unwrap();
    assert_eq!(d1.bgra, src);
    let d3 = decode(&with_chunk(&base, b"eXIf", &exif(3))).unwrap();
    assert_eq!(px(&d3, 0, 0), px(&d1, 3, 1));
    assert_eq!(px(&d3, 3, 1), px(&d1, 0, 0));
    let d6 = decode(&with_chunk(&base, b"eXIf", &exif(6))).unwrap();
    assert_eq!((d6.width, d6.height), (2, 4));
    // Clockwise: the old bottom-left pixel becomes the top-left one.
    assert_eq!(px(&d6, 0, 0), px(&d1, 0, 1));
    assert_eq!(px(&d6, 1, 0), px(&d1, 0, 0));
}

#[test]
fn icc_profile_is_converted_to_srgb() {
    use moxcms::ColorProfile;
    let src = [50u8, 100, 200, 255]; // BGRA: R=200 G=100 B=50
    let png = png_bytes(&src, 1, 1);
    // Same pixel tagged with the sRGB profile: untouched.
    let srgb = ColorProfile::new_srgb().encode().unwrap();
    let plain = decode(&with_chunk(&png, b"iCCP", &iccp(&srgb))).unwrap();
    assert_eq!(plain.bgra, src);
    // Tagged Display P3: the same numbers mean a different colour in sRGB.
    let p3 = ColorProfile::new_display_p3().encode().unwrap();
    let wide = decode(&with_chunk(&png, b"iCCP", &iccp(&p3))).unwrap();
    assert_ne!(wide.bgra, src);
    assert!(
        wide.bgra[2] >= 200,
        "red gets more saturated: {:?}",
        wide.bgra
    );
}

/// iCCP body: name, NUL, method 0, zlib stream.
fn iccp(profile: &[u8]) -> Vec<u8> {
    let mut v = b"icc\0\0".to_vec();
    v.extend_from_slice(&miniz_store(profile));
    v
}

/// A zlib stream made of stored blocks.
fn miniz_store(data: &[u8]) -> Vec<u8> {
    let mut v = vec![0x78, 0x01];
    let mut chunks = data.chunks(65_535).peekable();
    while let Some(c) = chunks.next() {
        v.push(u8::from(chunks.peek().is_none()));
        v.extend_from_slice(&(c.len() as u16).to_le_bytes());
        v.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        v.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    v.extend_from_slice(&((b << 16) | a).to_be_bytes());
    v
}

#[test]
fn transparency_is_flattened_on_white() {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 2, 1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(&[255, 0, 0, 0, 0, 0, 255, 255]).unwrap();
    w.finish().unwrap();
    let d = decode(&out).unwrap();
    assert_eq!(px(&d, 0, 0), [255, 255, 255, 255]); // fully transparent → white
    assert_eq!(px(&d, 1, 0), [255, 0, 0, 255]); // opaque blue stays (BGRA)
}

#[test]
fn webp_reads_back_exactly() {
    let src = tagged();
    let img = Bgra::new(4, 2, 16, &src);
    let webp = encode(ImageFormat::WebP, &img, &Settings::default()).unwrap();
    assert_eq!(decode(&webp).unwrap().bgra, src);
}

#[cfg(feature = "native-codecs")]
#[test]
fn native_formats_read() {
    // SVT-AV1 misbehaves on tiny frames: use a size the encoder tests also use.
    let (w, h) = (96u32, 64u32);
    let src: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            [
                (i % 251) as u8,
                ((i / w) * 3) as u8,
                (i % 97) as u8 * 2,
                255,
            ]
        })
        .collect();
    let img = Bgra::new(w, h, w as usize * 4, &src);
    let jxl = encode(ImageFormat::Jxl, &img, &Settings::default()).unwrap();
    let d = decode(&jxl).unwrap();
    for (a, b) in d.bgra.iter().zip(&src) {
        assert!(a.abs_diff(*b) <= 1, "jxl {a} vs {b}");
    }
    let avif = encode(ImageFormat::Avif, &img, &Settings::default()).unwrap();
    assert_eq!(SourceFormat::sniff(&avif), Some(SourceFormat::Avif));
    let d = decode(&avif).unwrap();
    assert_eq!((d.width, d.height), (w, h));
    let jpeg = encode(ImageFormat::Jpeg, &img, &Settings::default()).unwrap();
    assert_eq!(decode(&jpeg).unwrap().width, w);
}
