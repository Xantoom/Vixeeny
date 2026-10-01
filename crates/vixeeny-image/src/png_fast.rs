// SPDX-License-Identifier: GPL-3.0-or-later
//! Quick PNG for direct captures (CA-IMG-1: a 4K screenshot on disk in under 300 ms). The
//! optimised (oxipng) path for the editor arrives with M5.

use png::{BitDepth, ColorType, Compression, Encoder, Filter};

use crate::ImageError;

/// Encodes opaque BGRA pixels (rows `stride` bytes apart) as an RGB PNG. Alpha is dropped:
/// screen captures are opaque, and RGB is a quarter smaller to compress.
pub fn encode_bgra(
    width: u32,
    height: u32,
    stride: usize,
    bgra: &[u8],
) -> Result<Vec<u8>, ImageError> {
    let row_bytes = width as usize * 4;
    if width == 0 || height == 0 {
        return Err(ImageError::BadBuffer("empty image"));
    }
    if stride < row_bytes || bgra.len() < stride * (height as usize - 1) + row_bytes {
        return Err(ImageError::BadBuffer("buffer smaller than width × height"));
    }
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for y in 0..height as usize {
        let row = &bgra[y * stride..y * stride + row_bytes];
        for px in row.as_chunks::<4>().0 {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    let mut out = Vec::with_capacity(rgb.len() / 3);
    let mut encoder = Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    encoder.set_compression(Compression::Fast);
    encoder.set_filter(Filter::Sub);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgb)?;
    writer.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_swaps_channels_and_honours_stride() {
        // 2×2, stride 12 (4 padding bytes per row). Pixels are B, G, R, A.
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&[10, 20, 30, 255]);
        data[4..8].copy_from_slice(&[11, 21, 31, 255]);
        data[12..16].copy_from_slice(&[12, 22, 32, 255]);
        data[16..20].copy_from_slice(&[13, 23, 33, 255]);
        let bytes = encode_bgra(2, 2, 12, &data).unwrap();

        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&buf[0..3], &[30, 20, 10]);
        assert_eq!(&buf[3..6], &[31, 21, 11]);
        assert_eq!(&buf[6..9], &[32, 22, 12]);
        assert_eq!(&buf[9..12], &[33, 23, 13]);
    }

    #[test]
    fn rejects_short_buffers() {
        assert!(encode_bgra(2, 2, 8, &[0; 15]).is_err());
        assert!(encode_bgra(0, 2, 0, &[]).is_err());
    }
}
