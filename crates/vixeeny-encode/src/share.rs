// SPDX-License-Identifier: GPL-3.0-or-later
//! Where a part of a replay is sent: each network's limits (October 2026), and what a copy for it is written with.
//!
//! A network that limits the size of a file (Discord, Bluesky, Reddit, Telegram...) gets a bitrate that fills it, and
//! the part kept is held to the length that size leaves at a bitrate where the picture still holds ([`floor_kbps`]).
//! The others get the constant quality of the recordings. Never more than 1080p: the networks show no more (X and
//! Bluesky not even that).

use crate::registry::Family;

/// A network a copy is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Network {
    Discord,
    X,
    YouTube,
    TikTok,
    Bluesky,
    Reddit,
    Instagram,
    Threads,
    Telegram,
}

impl Network {
    /// Its name in lower case, as its mark is found by.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::X => "x",
            Self::YouTube => "youtube",
            Self::TikTok => "tiktok",
            Self::Bluesky => "bluesky",
            Self::Reddit => "reddit",
            Self::Instagram => "instagram",
            Self::Threads => "threads",
            Self::Telegram => "telegram",
        }
    }

    /// Another name it is searched by.
    pub const fn also(self) -> &'static str {
        match self {
            Self::X => "twitter",
            Self::YouTube => "shorts",
            Self::Instagram => "reels",
            _ => "",
        }
    }
}

/// What a network takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Stored in the settings.
    pub id: &'static str,
    pub network: Network,
    /// What the user reads (brand names, the same in every language).
    pub name: &'static str,
    /// The largest file, in megabytes (10⁶ bytes).
    pub max_mb: Option<u32>,
    /// The longest video, in seconds.
    pub max_seconds: Option<u32>,
    /// The picture's height and frames per second the copy is made at (lower when the replay is).
    pub height: u32,
    pub fps: u32,
}

impl Target {
    const fn new(id: &'static str, network: Network, name: &'static str, height: u32, fps: u32) -> Self {
        Self { id, network, name, max_mb: None, max_seconds: None, height, fps }
    }

    const fn mb(self, mb: u32) -> Self {
        Self { max_mb: Some(mb), ..self }
    }

    const fn seconds(self, seconds: u32) -> Self {
        Self { max_seconds: Some(seconds), ..self }
    }

    /// The codec every one of them plays as it is sent.
    pub const fn codec(&self) -> Family {
        Family::H264
    }
}

/// Every network, the most used to share a moment of a game first, and each one's plans from the free one up.
///
/// Discord: 20 MB free (since August 2026), 50 MB with Nitro Basic, 1 GB with Nitro (since October 2026); it keeps the
/// file as it is: 720p, at 30 FPS on the free plan so that more of the moment fits. X: 2 min 20 s without Premium,
/// where it plays 720p; with Premium, 1080p under two hours. Bluesky: 10 minutes, 300 MB, played at 720p at most.
/// Reddit: 15 minutes, 1 GB. Instagram: Reels of 3 minutes reach everyone. Threads: 5 minutes, 1 GB. TikTok: 10
/// minutes. Telegram: 2 GB.
pub const TARGETS: [Target; 12] = [
    Target::new("discord", Network::Discord, "Discord", 720, 30).mb(20),
    Target::new("discord_nitro_basic", Network::Discord, "Discord Nitro Basic", 720, 60).mb(50),
    Target::new("discord_nitro", Network::Discord, "Discord Nitro", 1080, 60).mb(1_000),
    Target::new("x", Network::X, "X", 720, 60).mb(512).seconds(140),
    Target::new("x_premium", Network::X, "X Premium", 1080, 60).mb(16_000).seconds(7_200),
    Target::new("youtube", Network::YouTube, "YouTube", 1080, 60),
    Target::new("tiktok", Network::TikTok, "TikTok", 1080, 60).seconds(600),
    Target::new("bluesky", Network::Bluesky, "Bluesky", 720, 60).mb(300).seconds(600),
    Target::new("reddit", Network::Reddit, "Reddit", 1080, 60).mb(1_000).seconds(900),
    Target::new("instagram", Network::Instagram, "Instagram", 1080, 30).seconds(180),
    Target::new("threads", Network::Threads, "Threads", 1080, 30).mb(1_000).seconds(300),
    Target::new("telegram", Network::Telegram, "Telegram", 1080, 60).mb(2_000),
];

pub fn target(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.id == id)
}

/// The sound of a copy: AAC, every track in one (what a web player plays).
pub const AUDIO_KBPS: u32 = 160;

/// The share of a size limit the streams may take: the container and a rate control that overshoots a little take the
/// rest.
const SIZE_SHARE: f64 = 0.97;

/// Seconds of the encoder's buffer at its ceiling a file may hold beyond its average (the buffer of
/// [`Encoder::cap_rate`](crate::registry::Encoder::cap_rate)).
const BUFFER_SECONDS: f64 = 2.0;

/// The bitrate (kbit/s) under which a game's picture breaks into blocks at `size` and `fps`: what streaming platforms
/// advise for H.264 (October 2026: 1080p60 6000, 720p60 and 1080p30 4500, 720p30 3000), as a power of the pixels a
/// second. Kept for HEVC and AV1, which then look better.
pub fn floor_kbps((width, height): (u32, u32), fps: u32) -> u32 {
    let rate = f64::from(width) * f64::from(height) * f64::from(fps.max(1));
    let kbps = 3_000.0 * (rate / (1280.0 * 720.0 * 30.0)).powf(0.46);
    ((kbps / 100.0).round() * 100.0).max(500.0) as u32
}

fn budget_kbit(mb: u32) -> f64 {
    f64::from(mb) * 8_000.0 * SIZE_SHARE
}

/// The longest part `target` takes at `size` and `fps`, in seconds: its own limit, and the one its size limit leaves at
/// [`floor_kbps`]. `None`: no limit.
pub fn max_seconds(target: &Target, size: (u32, u32), fps: u32) -> Option<f64> {
    let by_size = target.max_mb.map(|mb| {
        let floor = f64::from(floor_kbps(size, fps));
        ((budget_kbit(mb) - BUFFER_SECONDS * floor) / (floor + f64::from(AUDIO_KBPS))).max(1.0)
    });
    let own = target.max_seconds.map(f64::from);
    match (by_size, own) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The video's ceiling (kbit/s) for a part of `seconds` to fit `target`'s size with its sound, its buffer included.
/// `None`: no size limit, constant quality alone.
pub fn video_kbps(target: &Target, seconds: f64) -> Option<u32> {
    let mb = target.max_mb?;
    let seconds = seconds.max(0.1);
    let kbps = (budget_kbit(mb) - f64::from(AUDIO_KBPS) * seconds) / (seconds + BUFFER_SECONDS);
    Some(kbps.max(100.0) as u32)
}

/// The size of a copy: `height` at most (never more than the source), the width that keeps the picture's proportions,
/// both even.
pub fn scaled((width, height): (u32, u32), to_height: u32) -> (u32, u32) {
    if to_height >= height || height == 0 {
        return (width & !1, height & !1);
    }
    let w = (f64::from(width) * f64::from(to_height) / f64::from(height)).round() as u32;
    (w.max(2) & !1, to_height.max(2) & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_follows_what_streaming_advises() {
        assert_eq!(floor_kbps((1920, 1080), 60), 6_000);
        assert_eq!(floor_kbps((1280, 720), 30), 3_000);
        let near = |a: u32, b: u32| a.abs_diff(b) <= 400;
        assert!(near(floor_kbps((1280, 720), 60), 4_500));
        assert!(near(floor_kbps((1920, 1080), 30), 4_500));
    }

    #[test]
    fn a_size_limit_holds_the_length_and_fills_the_file() {
        let discord = target("discord").unwrap();
        // 720p30 in 20 MB: about three quarters of a minute.
        let longest = max_seconds(discord, (1280, 720), 30).unwrap();
        assert!((40.0..55.0).contains(&longest), "{longest}");
        // At that length the bitrate is the floor; shorter, more.
        let at = video_kbps(discord, longest).unwrap();
        assert!(at.abs_diff(3_000) <= 10, "{at}");
        assert!(video_kbps(discord, 10.0).unwrap() > 10_000);
        // The file, its buffer spent at once, stays under 20 MB.
        for seconds in [1.0, 10.0, longest] {
            let kbit = f64::from(video_kbps(discord, seconds).unwrap()) * (seconds + 2.0) + 160.0 * seconds;
            assert!(kbit / 8_000.0 < 20.0, "{seconds}");
        }
        // X: its own limit; YouTube: none.
        assert_eq!(max_seconds(target("x").unwrap(), (1280, 720), 60), Some(140.0));
        assert_eq!(max_seconds(target("youtube").unwrap(), (1920, 1080), 60), None);
        assert_eq!(video_kbps(target("youtube").unwrap(), 30.0), None);
    }

    #[test]
    fn the_presets_never_go_above_1080p_and_are_found_by_id() {
        assert!(TARGETS.iter().all(|t| t.height <= 1080 && matches!(t.fps, 30 | 60)));
        let mut ids: Vec<_> = TARGETS.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TARGETS.len());
        assert_eq!(target("discord_nitro").map(|t| t.max_mb), Some(Some(1_000)));
        assert_eq!(scaled((2560, 1440), 720), (1280, 720));
        assert_eq!(scaled((3440, 1440), 1080), (2580, 1080));
        assert_eq!(scaled((1280, 720), 1080), (1280, 720));
    }
}
