// SPDX-License-Identifier: GPL-3.0-or-later
//! The look of a window: light or dark, the accent colour of the system, the font and whether
//! animations run. Every window sets it once, through the `Theme` global of its `.slint` file.

use slint::ComponentHandle;

use crate::Theme;

pub use vixeeny_common::look::{DEFAULT_ACCENT, Look};

static DEFAULT: std::sync::Mutex<Look> = std::sync::Mutex::new(Look::dark());

/// The look every window uses when it is not given one: set once by the host at start-up.
pub fn set_default(look: Look) {
    if let Ok(mut d) = DEFAULT.lock() {
        *d = look;
    }
}

pub fn default_look() -> Look {
    DEFAULT.lock().map_or(Look::dark(), |d| *d)
}

/// The font of every window: Segoe UI Variable (Windows 11), which falls back to Segoe UI.
pub const fn font() -> &'static str {
    "Segoe UI Variable Text"
}

/// Runs `f` with the native handle as soon as the event loop shows the window (the handle does
/// not exist before): the backend wakes this when it creates the window, nothing is polled.
#[cfg(feature = "desktop")]
pub fn when_native<C: ComponentHandle + 'static>(window: &C, f: impl Fn(u64) + 'static) {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let weak = window.as_weak();
    let _ = slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Ok(native) = window.window().winit_window().await else {
            return;
        };
        if let Ok(handle) = native.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            f(h.hwnd.get() as u64);
        }
    });
}

/// Sets the `Theme` of `window`.
pub fn apply<C>(window: &C, look: Look)
where
    C: ComponentHandle + 'static,
    for<'a> Theme<'a>: slint::Global<'a, C>,
{
    let theme = window.global::<Theme>();
    theme.set_dark(look.dark);
    theme.set_accent_base(slint::Color::from_rgb_u8(
        look.accent[0],
        look.accent[1],
        look.accent[2],
    ));
    theme.set_font(font().into());
    theme.set_motion(if look.animations { 1.0 } else { 0.0 });
    theme.set_app_icon(app_icon());
    #[cfg(feature = "desktop")]
    if let Some(dresser) = DRESSER.get() {
        let dresser = *dresser;
        let weak = window.as_weak();
        when_native(window, move |handle| {
            let mica = dresser(handle, look);
            if let Some(window) = weak.upgrade() {
                window.global::<Theme>().set_mica(mica);
            }
        });
    }
}

/// What the host does to a native window once it exists (the title bar of Windows 11 follows the
/// theme), true when it put Mica behind it. Set once at start-up; every window that gets a
/// [`Look`] is dressed with it.
static DRESSER: std::sync::OnceLock<fn(u64, Look) -> bool> = std::sync::OnceLock::new();

pub fn set_dresser(f: fn(u64, Look) -> bool) {
    let _ = DRESSER.set(f);
}

/// The program's icon (64 px), decoded from the PNG that ships inside the binary.
pub fn app_icon() -> slint::Image {
    const PNG: &[u8] = include_bytes!("../../../packaging/icons/vixeeny-64.png");
    let decoded = (|| {
        let mut reader = png::Decoder::new(std::io::Cursor::new(PNG))
            .read_info()
            .ok()?;
        let mut data = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut data).ok()?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let mut buffer =
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(info.width, info.height);
        buffer
            .make_mut_bytes()
            .copy_from_slice(data.get(..info.buffer_size())?);
        Some(slint::Image::from_rgba8(buffer))
    })();
    decoded.unwrap_or_default()
}
