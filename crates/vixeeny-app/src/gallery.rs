// SPDX-License-Identifier: GPL-3.0-or-later
//! The files of the gallery (plan 5.13, section 1): the latest captures in the images, videos and
//! replays folders, with their thumbnails. Pure file handling, no UI.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use vixeeny_platform::LocalTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
    pub modified: SystemTime,
    pub size: u64,
    /// The application the capture is named after (its sub-folder, or the start of its name).
    pub app: String,
}

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "avif", "jxl"];
const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "mkv", "webm"];
/// Folders are walked this deep (sub-folder per application, then maybe one more).
const MAX_DEPTH: usize = 3;

fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(Kind::Image)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// `Game_2026-10-02_14-03-11` → `Game`: the part before the date of the default template.
fn app_from_stem(stem: &str) -> String {
    let bytes = stem.as_bytes();
    for (i, _) in stem.match_indices('_') {
        let rest = &bytes[i + 1..];
        let is_date = rest.len() >= 10
            && rest[..10].iter().enumerate().all(|(k, b)| match k {
                4 | 7 => *b == b'-',
                _ => b.is_ascii_digit(),
            });
        if is_date && i > 0 {
            return stem[..i].to_owned();
        }
    }
    String::new()
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Item>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, depth + 1, out);
            }
            continue;
        }
        let Some(kind) = kind_of(&path) else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let sub = path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.components().next())
            .filter(|_| path.parent() != Some(root))
            .map(|c| c.as_os_str().to_string_lossy().into_owned());
        out.push(Item {
            app: sub.unwrap_or_else(|| app_from_stem(stem)),
            name,
            kind,
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: meta.len(),
            path,
        });
    }
}

/// The `limit` newest captures of `roots`, newest first. A folder that does not exist is skipped.
pub fn scan(roots: &[PathBuf], limit: usize) -> Vec<Item> {
    let mut items = Vec::new();
    for root in roots {
        walk(root, root, 1, &mut items);
    }
    items.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    items.dedup_by(|a, b| a.path == b.path);
    items.truncate(limit);
    items
}

/// Indexes of the items that pass the filters: `kind` 0 all, 1 images, 2 videos; `app` is a
/// case-insensitive part of the application name (empty = any).
pub fn filter(items: &[Item], kind: i32, app: &str) -> Vec<usize> {
    let app = app.trim().to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, i)| match kind {
            1 => i.kind == Kind::Image,
            2 => i.kind == Kind::Video,
            _ => true,
        })
        .filter(|(_, i)| app.is_empty() || i.app.to_lowercase().contains(&app))
        .map(|(n, _)| n)
        .collect()
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Seconds since the epoch of a civil date-time read as UTC.
fn unix_of(t: &LocalTime) -> i64 {
    let (y, m, d) = (i64::from(t.year), i64::from(t.month), i64::from(t.day));
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.minute) * 60 + i64::from(t.second)
}

/// How far the local time is from UTC right now, in seconds (rounded to 15 minutes).
pub fn local_offset_secs() -> i64 {
    let now_utc = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let diff = unix_of(&vixeeny_platform::local_time()) - now_utc;
    (diff as f64 / 900.0).round() as i64 * 900
}

/// `2026-10-02 14:03 · 2.4 MB`, the date in the local time zone.
pub fn detail(item: &Item, offset_secs: i64) -> String {
    let secs = item
        .modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let t = LocalTime::from_unix_utc((secs + offset_secs).max(0) as u64, 0);
    format!(
        "{} {:02}:{:02} · {}",
        t.date(),
        t.hour,
        t.minute,
        human_size(item.size)
    )
}

#[cfg(feature = "ffmpeg")]
fn video_thumbnail(path: &Path, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    vixeeny_encode::thumbnail::video_thumbnail(path, max_side)
}

/// Without FFmpeg a video has no picture: the gallery shows its placeholder.
#[cfg(not(feature = "ffmpeg"))]
fn video_thumbnail(_: &Path, _: u32) -> Option<(u32, u32, Vec<u8>)> {
    None
}

/// A thumbnail of an image file, at most `max_side` pixels on its long side: RGBA.
/// `None` for files that cannot be decoded (and for videos in a build without FFmpeg).
pub fn thumbnail(path: &Path, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    if kind_of(path)? == Kind::Video {
        return video_thumbnail(path, max_side);
    }
    let bytes = std::fs::read(path).ok()?;
    let img = vixeeny_image::decode(&bytes).ok()?;
    let scale = (max_side as f32 / img.width.max(img.height) as f32).min(1.0);
    let (w, h) = (
        ((img.width as f32 * scale).round() as u32).max(1),
        ((img.height as f32 * scale).round() as u32).max(1),
    );
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    // Box filter: the mean of the source pixels each thumbnail pixel covers.
    for y in 0..h {
        let y0 = (u64::from(y) * u64::from(img.height) / u64::from(h)) as usize;
        let y1 = ((u64::from(y + 1) * u64::from(img.height) / u64::from(h)) as usize).max(y0 + 1);
        for x in 0..w {
            let x0 = (u64::from(x) * u64::from(img.width) / u64::from(w)) as usize;
            let x1 =
                ((u64::from(x + 1) * u64::from(img.width) / u64::from(w)) as usize).max(x0 + 1);
            let (mut b, mut g, mut r, mut n) = (0_u32, 0_u32, 0_u32, 0_u32);
            for sy in y0..y1.min(img.height as usize) {
                for sx in x0..x1.min(img.width as usize) {
                    let o = (sy * img.width as usize + sx) * 4;
                    b += u32::from(img.bgra[o]);
                    g += u32::from(img.bgra[o + 1]);
                    r += u32::from(img.bgra[o + 2]);
                    n += 1;
                }
            }
            let n = n.max(1);
            out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
        }
    }
    Some((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vixeeny-gallery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, age_secs: u64, size: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0_u8; size]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn the_newest_captures_come_first_across_folders_and_other_files_are_ignored() {
        let root = scratch("scan");
        let (images, videos) = (root.join("Images"), root.join("Videos"));
        touch(&images.join("Game_2026-10-02_14-03-11.png"), 300, 10);
        touch(
            &images.join("Minecraft/Minecraft_2026-10-01_09-00-00.jpg"),
            100,
            10,
        );
        touch(&videos.join("clip.mp4"), 200, 10);
        touch(&videos.join("notes.txt"), 1, 10);
        let items = scan(&[images, videos, root.join("missing")], 10);
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Minecraft_2026-10-01_09-00-00.jpg",
                "clip.mp4",
                "Game_2026-10-02_14-03-11.png"
            ]
        );
        assert_eq!(scan(&[root.join("Images")], 1).len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_application_comes_from_the_sub_folder_or_the_file_name() {
        let root = scratch("apps");
        touch(&root.join("Game_2026-10-02_14-03-11.png"), 1, 1);
        touch(&root.join("Firefox/shot.png"), 2, 1);
        touch(&root.join("plain.png"), 3, 1);
        let items = scan(std::slice::from_ref(&root), 10);
        let app = |name: &str| items.iter().find(|i| i.name == name).unwrap().app.clone();
        assert_eq!(app("Game_2026-10-02_14-03-11.png"), "Game");
        assert_eq!(app("shot.png"), "Firefox");
        assert_eq!(app("plain.png"), "");
        assert_eq!(app_from_stem("My_Game_2026-10-02_14-03-11"), "My_Game");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn filters_by_kind_and_application() {
        let root = scratch("filter");
        touch(&root.join("Game_2026-10-02_14-03-11.png"), 1, 1);
        touch(&root.join("Game_2026-10-02_14-05-00.mp4"), 2, 1);
        touch(&root.join("Browser_2026-10-02_14-06-00.png"), 3, 1);
        let items = scan(std::slice::from_ref(&root), 10);
        assert_eq!(filter(&items, 0, "").len(), 3);
        assert_eq!(filter(&items, 1, "").len(), 2);
        assert_eq!(filter(&items, 2, "").len(), 1);
        assert_eq!(filter(&items, 0, " GAM ").len(), 2);
        assert_eq!(filter(&items, 1, "game").len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sizes_and_dates_read_well() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2_516_582), "2.4 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        let item = Item {
            path: PathBuf::new(),
            name: String::new(),
            kind: Kind::Image,
            // 2026-10-02 12:03:00 UTC
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_942_580),
            size: 2_516_582,
            app: String::new(),
        };
        assert_eq!(detail(&item, 0), "2026-10-02 12:03 · 2.4 MB");
        assert_eq!(detail(&item, 2 * 3600), "2026-10-02 14:03 · 2.4 MB");
    }

    #[test]
    fn the_local_offset_is_a_plausible_whole_quarter_hour() {
        let offset = local_offset_secs();
        assert_eq!(offset % 900, 0);
        assert!(offset.abs() <= 14 * 3600);
        // The conversion and its inverse agree.
        let t = LocalTime::from_unix_utc(1_790_942_580, 0);
        assert_eq!(unix_of(&t), 1_790_942_580);
    }

    #[test]
    fn thumbnails_are_scaled_down_with_the_aspect_ratio_and_videos_have_none() {
        let root = scratch("thumb");
        let path = root.join("wide.png");
        let mut data = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut data, 400, 100);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            // Left half red, right half blue.
            let row: Vec<u8> = (0..400)
                .flat_map(|x| {
                    if x < 200 {
                        [255, 0, 0, 255]
                    } else {
                        [0, 0, 255, 255]
                    }
                })
                .collect();
            w.write_image_data(&row.repeat(100)).unwrap();
        }
        std::fs::write(&path, data).unwrap();
        let (w, h, rgba) = thumbnail(&path, 100).unwrap();
        assert_eq!((w, h), (100, 25));
        assert_eq!(rgba.len(), 100 * 25 * 4);
        assert_eq!(&rgba[..4], [255, 0, 0, 255]);
        assert_eq!(&rgba[(99) * 4..(99) * 4 + 4], [0, 0, 255, 255]);
        // A small picture is not enlarged.
        assert_eq!(thumbnail(&path, 1000).map(|t| (t.0, t.1)), Some((400, 100)));
        assert!(thumbnail(&root.join("a.mp4"), 100).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
