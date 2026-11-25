// SPDX-License-Identifier: GPL-3.0-or-later
//! The look of a window: light or dark, the accent colour of the system, the font and whether animations run. Every
//! window sets it once, through the `Theme` global of its `.slint` file.

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

/// Runs `f` with the native handle as soon as the event loop shows the window (the handle does not exist before): the
/// backend wakes this when it creates the window, nothing is polled.
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
    theme.set_accent_base(slint::Color::from_rgb_u8(look.accent[0], look.accent[1], look.accent[2]));
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
                let theme = window.global::<Theme>();
                theme.set_mica_available(mica);
                theme.set_mica(mica && window.window().is_visible() && is_foreground(handle));
            }
            if let Some(watcher) = COLOUR_WATCHER.get() {
                let weak = weak.clone();
                // Windows' colours changed: the look again, once the window is done with the message.
                watcher(
                    handle,
                    Box::new(move || {
                        let weak = weak.clone();
                        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                            if let (Some(window), Some(source)) = (weak.upgrade(), LOOK_SOURCE.get()) {
                                apply(&window, source());
                            }
                        });
                    }),
                );
            }
        });
    }
}

/// Whether the native window `handle` is the one in front (the one the user works in).
#[cfg(feature = "desktop")]
fn is_foreground(handle: u64) -> bool {
    FOREGROUND.get().is_some_and(|f| f() == handle)
}

/// The native window in front, from the host (set once at start-up).
static FOREGROUND: std::sync::OnceLock<fn() -> u64> = std::sync::OnceLock::new();

/// The look of the windows now, from the host: read again when Windows' theme or accent changes (set once at start-up).
static LOOK_SOURCE: std::sync::OnceLock<fn() -> Look> = std::sync::OnceLock::new();

pub fn set_foreground(f: fn() -> u64) {
    let _ = FOREGROUND.set(f);
}

pub fn set_look_source(f: fn() -> Look) {
    let _ = LOOK_SOURCE.set(f);
}

/// How the host has a native window tell when Windows' colours change (set once at start-up): every window dressed with
/// a [`Look`] takes the look of [`set_look_source`] again then.
static COLOUR_WATCHER: std::sync::OnceLock<ColourWatcher> = std::sync::OnceLock::new();

/// Has the native window (its handle) call the function given when Windows' colours change.
pub type ColourWatcher = fn(u64, Box<dyn Fn()>);

pub fn set_colour_watcher(f: ColourWatcher) {
    let _ = COLOUR_WATCHER.set(f);
}

/// What the theme does with an event of the window: Mica only while it is in front. Called by the window's one event
/// hook (the backend keeps one per window: see [`watch`] for a window without one of its own).
#[cfg(feature = "desktop")]
pub fn on_event<C>(window: &C, event: &slint::winit_030::winit::event::WindowEvent)
where
    C: ComponentHandle + 'static,
    for<'a> Theme<'a>: slint::Global<'a, C>,
{
    if let slint::winit_030::winit::event::WindowEvent::Focused(focused) = event {
        let theme = window.global::<Theme>();
        theme.set_mica(*focused && theme.get_mica_available());
    }
}

/// Hooks [`on_event`] to a window that has no event hook of its own.
#[cfg(feature = "desktop")]
pub fn watch<C>(window: &C)
where
    C: ComponentHandle + 'static,
    for<'a> Theme<'a>: slint::Global<'a, C>,
{
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    let weak = window.as_weak();
    window.window().on_winit_window_event(move |_, event| {
        if let Some(window) = weak.upgrade() {
            on_event(&window, event);
        }
        EventResult::Propagate
    });
}

/// What the host does to a native window once it exists (the title bar of Windows 11 follows the theme), true when it
/// put Mica behind it. Set once at start-up; every window that gets a [`Look`] is dressed with it.
static DRESSER: std::sync::OnceLock<fn(u64, Look) -> bool> = std::sync::OnceLock::new();

pub fn set_dresser(f: fn(u64, Look) -> bool) {
    let _ = DRESSER.set(f);
}

/// The program's icon (64 px), decoded from the PNG that ships inside the binary.
pub fn app_icon() -> slint::Image {
    const PNG: &[u8] = include_bytes!("../../../packaging/icons/vixeeny-64.png");
    let decoded = (|| {
        let mut reader = png::Decoder::new(std::io::Cursor::new(PNG)).read_info().ok()?;
        let mut data = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut data).ok()?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(info.width, info.height);
        buffer.make_mut_bytes().copy_from_slice(data.get(..info.buffer_size())?);
        Some(slint::Image::from_rgba8(buffer))
    })();
    decoded.unwrap_or_default()
}
