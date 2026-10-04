// SPDX-License-Identifier: GPL-3.0-or-later
//! Putting an image on the clipboard (plan 5.3, CA-ED-2): a `PNG` entry for browsers, Discord and
//! most editors, and a `CF_DIBV5` bitmap for everything else (Paint, Office). Both are lossless.

/// UTF-16, NUL-terminated, as little-endian bytes (`CF_UNICODETEXT`). Line breaks become CRLF.
pub fn utf16_z(text: &str) -> Vec<u8> {
    let crlf = text.replace("\r\n", "\n").replace('\n', "\r\n");
    crlf.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A `BITMAPV5HEADER` followed by bottom-up BGRA pixels, from top-down BGRA input.
pub fn dibv5(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    const HEADER: usize = 124;
    let row = width as usize * 4;
    let mut out = Vec::with_capacity(HEADER + row * height as usize);
    let put32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    put32(&mut out, HEADER as u32); // bV5Size
    put32(&mut out, width); // bV5Width
    put32(&mut out, height); // bV5Height (positive: bottom-up)
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bit count
    put32(&mut out, 3); // BI_BITFIELDS
    put32(&mut out, (row * height as usize) as u32); // image size
    put32(&mut out, 2835); // 72 dpi in pixels per metre, x
    put32(&mut out, 2835); // y
    put32(&mut out, 0); // colours used
    put32(&mut out, 0); // important colours
    put32(&mut out, 0x00FF_0000); // red mask
    put32(&mut out, 0x0000_FF00); // green mask
    put32(&mut out, 0x0000_00FF); // blue mask
    put32(&mut out, 0xFF00_0000); // alpha mask
    put32(&mut out, 0x7352_4742); // LCS_sRGB ('sRGB')
    out.resize(HEADER, 0); // colour endpoints, gamma: unused for sRGB; intent, profile fields 0
    for y in (0..height as usize).rev() {
        out.extend_from_slice(&bgra[y * row..(y + 1) * row]);
    }
    out
}

/// Opens the clipboard, empties it, lets `fill` add formats, closes it.
fn with_clipboard(
    fill: impl FnOnce(&dyn Fn(u32, &[u8]) -> crate::Result<()>) -> crate::Result<()>,
) -> crate::Result<()> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

    use crate::PlatformError;

    let os =
        |what: &str, e: windows::core::Error| PlatformError::Os(format!("clipboard {what}: {e}"));

    // Clipboard owners hold it briefly; retry a few times before giving up.
    let mut opened = false;
    for _ in 0..10 {
        // SAFETY: no window handle: the clipboard is then owned by the current task.
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !opened {
        return Err(PlatformError::Os("clipboard is busy".into()));
    }

    let result = (|| {
        // SAFETY: the clipboard is open.
        unsafe { EmptyClipboard() }.map_err(|e| os("empty", e))?;
        let put = |format: u32, bytes: &[u8]| -> crate::Result<()> {
            // SAFETY: allocates a movable block of the right size; the lock gives a writable
            // pointer to it for the copy; on success the system owns the block.
            unsafe {
                let mem: HGLOBAL =
                    GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|e| os("alloc", e))?;
                let ptr = GlobalLock(mem);
                if ptr.is_null() {
                    let _ = GlobalFree(Some(mem));
                    return Err(PlatformError::Os("clipboard: GlobalLock failed".into()));
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
                let _ = GlobalUnlock(mem);
                if let Err(e) = SetClipboardData(format, Some(HANDLE(mem.0))) {
                    let _ = GlobalFree(Some(mem));
                    return Err(os("set", e));
                }
            }
            Ok(())
        };
        fill(&put)
    })();
    // SAFETY: the clipboard was opened above.
    let _ = unsafe { CloseClipboard() };
    result
}

pub fn copy_image(width: u32, height: u32, bgra: &[u8], png: &[u8]) -> crate::Result<()> {
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::core::w;

    const CF_DIBV5: u32 = 17;

    if bgra.len() < width as usize * height as usize * 4 {
        return Err(crate::PlatformError::Os(
            "clipboard: pixel buffer too small".into(),
        ));
    }
    with_clipboard(|put| {
        // SAFETY: registering a named format has no preconditions.
        let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
        if png_format != 0 {
            put(png_format, png)?;
        }
        put(CF_DIBV5, &dibv5(width, height, bgra))
    })
}

/// Plain text, as `CF_UNICODETEXT`.
pub fn copy_text(text: &str) -> crate::Result<()> {
    const CF_UNICODETEXT: u32 = 13;
    with_clipboard(|put| put(CF_UNICODETEXT, &utf16_z(text)))
}

#[cfg(test)]
mod tests {
    use super::{dibv5, utf16_z};

    #[test]
    fn text_is_utf16_with_crlf_and_a_terminator() {
        assert_eq!(
            utf16_z("a\nb"),
            [b'a', 0, b'\r', 0, b'\n', 0, b'b', 0, 0, 0]
        );
        assert_eq!(utf16_z(""), [0, 0]);
        assert_eq!(utf16_z("a\r\nb"), utf16_z("a\nb"));
        // outside the BMP: a surrogate pair
        assert_eq!(utf16_z("😀").len(), 6);
        assert_eq!(utf16_z("日"), [0xE5, 0x65, 0, 0]);
    }

    #[test]
    fn header_and_bottom_up_rows() {
        // 2×2, top row (1,2,3,255)(4,5,6,255), bottom row (7,8,9,255)(10,11,12,255)
        let px = [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let d = dibv5(2, 2, &px);
        assert_eq!(d.len(), 124 + 16);
        let u32_at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
        assert_eq!(u32_at(0), 124);
        assert_eq!((u32_at(4), u32_at(8)), (2, 2));
        assert_eq!(u32_at(16), 3); // BI_BITFIELDS
        assert_eq!(u32_at(40), 0x00FF_0000);
        assert_eq!(u32_at(52), 0xFF00_0000);
        assert_eq!(u32_at(56), 0x7352_4742);
        // the bottom row comes first
        assert_eq!(&d[124..128], &[7, 8, 9, 255]);
        assert_eq!(&d[132..136], &[1, 2, 3, 255]);
    }
}
