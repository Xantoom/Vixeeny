// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::Mutex;

use super::*;

fn write_png(path: &Path, seed: u8) {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 16, 8);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    let data: Vec<u8> = (0..16 * 8 * 3)
        .map(|i| (i as u8).wrapping_mul(seed))
        .collect();
    w.write_image_data(&data).unwrap();
    w.finish().unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, out).unwrap();
}

fn job(dir: Option<PathBuf>, on_existing: OnExisting) -> Job {
    Job {
        inputs: vec![],
        format: ImageFormat::WebP,
        settings: Settings::default(),
        output_dir: dir,
        on_existing,
        threads: 3,
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vixeeny-convert-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn collects_files_and_folders_recursively() {
    let dir = scratch("collect");
    write_png(&dir.join("a.png"), 1);
    write_png(&dir.join("sub/b.PNG"), 2);
    fs::write(dir.join("notes.txt"), "x").unwrap();
    let files = collect(&[dir.clone(), dir.join("a.png")]);
    assert_eq!(files.len(), 3); // a.png appears via the folder and directly
    assert!(files.iter().all(|f| is_supported(f)));
    assert!(!is_supported(&dir.join("notes.txt")));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn converts_in_parallel_and_reports_progress() {
    let dir = scratch("run");
    let inputs: Vec<PathBuf> = (0..12u8).map(|i| dir.join(format!("{i}.png"))).collect();
    for (i, p) in inputs.iter().enumerate() {
        write_png(p, i as u8 + 1);
    }
    let events = Mutex::new(Vec::new());
    let summary = run(
        &inputs,
        &job(None, OnExisting::Rename),
        &AtomicBool::new(false),
        &|e| {
            events.lock().unwrap().push((e.done, e.total));
        },
    );
    assert_eq!(
        summary,
        Summary {
            converted: 12,
            ..Summary::default()
        }
    );
    let mut seen = events.into_inner().unwrap();
    seen.sort_unstable();
    assert_eq!(seen, (1..=12).map(|d| (d, 12)).collect::<Vec<_>>());
    for i in 0..12 {
        let out = fs::read(dir.join(format!("{i}.webp"))).unwrap();
        assert_eq!(&out[..4], b"RIFF");
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn existing_files_follow_the_policy() {
    let dir = scratch("exist");
    write_png(&dir.join("a.png"), 1);
    let files = vec![dir.join("a.png")];
    let cancel = AtomicBool::new(false);
    let ignore: &(dyn Fn(Event) + Sync) = &|_| {};
    run(&files, &job(None, OnExisting::Rename), &cancel, ignore);
    assert!(dir.join("a.webp").exists());
    // Rename: a second run makes "a (2).webp", then "a (3).webp".
    run(&files, &job(None, OnExisting::Rename), &cancel, ignore);
    run(&files, &job(None, OnExisting::Rename), &cancel, ignore);
    assert!(dir.join("a (2).webp").exists() && dir.join("a (3).webp").exists());
    // Skip leaves everything as is.
    let s = run(&files, &job(None, OnExisting::Skip), &cancel, ignore);
    assert_eq!(s.skipped, 1);
    assert!(!dir.join("a (4).webp").exists());
    // Overwrite replaces in place.
    fs::write(dir.join("a.webp"), b"old").unwrap();
    let s = run(&files, &job(None, OnExisting::Overwrite), &cancel, ignore);
    assert_eq!(s.converted, 1);
    assert_eq!(&fs::read(dir.join("a.webp")).unwrap()[..4], b"RIFF");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_source_is_never_overwritten() {
    let dir = scratch("self");
    write_png(&dir.join("a.png"), 1);
    let before = fs::read(dir.join("a.png")).unwrap();
    let mut j = job(None, OnExisting::Overwrite);
    j.format = ImageFormat::Png;
    let s = run(&[dir.join("a.png")], &j, &AtomicBool::new(false), &|_| {});
    assert_eq!(s.converted, 1);
    assert_eq!(fs::read(dir.join("a.png")).unwrap(), before);
    assert!(dir.join("a (2).png").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn same_names_from_different_inputs_do_not_collide() {
    let dir = scratch("collide");
    write_png(&dir.join("one/x.png"), 1);
    write_png(&dir.join("two/x.png"), 2);
    let out = dir.join("out");
    let files = vec![dir.join("one/x.png"), dir.join("two/x.png")];
    let s = run(
        &files,
        &job(Some(out.clone()), OnExisting::Overwrite),
        &AtomicBool::new(false),
        &|_| {},
    );
    assert_eq!(s.converted, 2);
    assert!(out.join("x.webp").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bad_files_fail_alone_and_cancel_stops() {
    let dir = scratch("bad");
    write_png(&dir.join("ok.png"), 1);
    fs::write(dir.join("broken.png"), b"not a png").unwrap();
    let files = vec![dir.join("broken.png"), dir.join("ok.png")];
    let s = run(
        &files,
        &job(None, OnExisting::Rename),
        &AtomicBool::new(false),
        &|_| {},
    );
    assert_eq!((s.converted, s.failed), (1, 1));
    let cancel = AtomicBool::new(true);
    let s = run(&files, &job(None, OnExisting::Rename), &cancel, &|_| {});
    assert_eq!(s.cancelled, 2);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn choices_map_to_settings() {
    let base = Settings::default();
    let c = Choices {
        format: ImageFormat::Jpeg,
        quality: 70,
        lossless: false,
    };
    assert_eq!(c.apply(base).jpeg.quality, 70);
    assert!(c.has_quality() && !c.has_lossless());
    let c = Choices::from_settings(ImageFormat::WebP, &base);
    assert!(c.lossless && !c.has_quality() && c.has_lossless());
    let lossy = Choices {
        lossless: false,
        quality: 60,
        ..c
    };
    let s = lossy.apply(base);
    assert!(!s.webp.lossless && s.webp.quality == 60.0);
    let jxl = Choices {
        format: ImageFormat::Jxl,
        quality: 90,
        lossless: false,
    };
    assert!((jxl.apply(base).jxl.distance - 1.0).abs() < 1e-5);
    assert!(!Choices::from_settings(ImageFormat::Png, &base).has_quality());
}

/// CA-CONV-1 (scaled down by default): many 4K PNGs → AVIF without error. Run the full 100 with
/// `VIXEENY_CONV_COUNT=100 cargo test -p vixeeny-convert --features native-codecs -- --ignored`.
#[cfg(feature = "native-codecs")]
#[test]
#[ignore = "slow: encodes 4K AVIF images"]
fn many_4k_pngs_to_avif() {
    let count: usize = std::env::var("VIXEENY_CONV_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let dir = scratch("4k");
    let mut inputs = Vec::new();
    for i in 0..count {
        let path = dir.join(format!("{i}.png"));
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 3840, 2160);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().unwrap();
        let row: Vec<u8> = (0..3840 * 3).map(|x| (x / 7 + i * 13) as u8).collect();
        let data: Vec<u8> = (0..2160)
            .flat_map(|y| row.iter().map(move |v| v.wrapping_add((y / 5) as u8)))
            .collect();
        w.write_image_data(&data).unwrap();
        w.finish().unwrap();
        fs::write(&path, out).unwrap();
        inputs.push(path);
    }
    let mut j = job(None, OnExisting::Rename);
    j.format = ImageFormat::Avif;
    let started = std::time::Instant::now();
    let s = run(&inputs, &j, &AtomicBool::new(false), &|_| {});
    eprintln!("{count} files in {:?}", started.elapsed());
    assert_eq!(
        s,
        Summary {
            converted: count,
            ..Summary::default()
        }
    );
    let _ = fs::remove_dir_all(dir);
}
