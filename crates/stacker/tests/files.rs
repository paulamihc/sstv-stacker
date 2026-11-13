//! Reading files, keeping details, sizes, the colour fix and the folder
//! runs, on made-up pictures (no real pictures in this project).
//!
//! `fixtures/scene.png` is a made-up 320 by 240 picture of random colour
//! blocks; `fixtures/scene.jpg` is the same, saved as a JPEG.

use std::collections::BTreeMap;
use std::path::Path;

use sstv_stacker::batch::{self, ColourFix, Input, Settings};
use sstv_stacker::folder;
use sstv_stacker::formats::{self, Details, Format};
use sstv_stacker::Image;

const SCENE_PNG: &[u8] = include_bytes!("fixtures/scene.png");
const SCENE_JPG: &[u8] = include_bytes!("fixtures/scene.jpg");

fn scene() -> Image {
    formats::read(SCENE_PNG).unwrap().image
}

/// A repeatable random number, 0 to 1.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A copy of `img` with speckle on some lines and a band of lines lost.
fn damaged(img: &Image, seed: u64, noisy_lines: f64, lost: std::ops::Range<usize>) -> Image {
    let mut r = Rng(seed);
    let mut out = img.clone();
    let w = img.width;
    for y in 0..img.height {
        if lost.contains(&y) {
            for v in &mut out.rgb[y * w * 3..(y + 1) * w * 3] {
                *v = 0;
            }
            continue;
        }
        if r.next() < noisy_lines {
            for x in 0..w {
                for c in 0..3 {
                    let i = (y * w + x) * 3 + c;
                    let n = (r.next() - 0.5) * 160.0;
                    out.rgb[i] = (out.rgb[i] as f64 + n).clamp(3.0, 255.0) as u8;
                }
            }
        }
    }
    out
}

fn png(img: &Image, text: &[(&str, &str)]) -> Vec<u8> {
    formats::write_png(
        img,
        &Details {
            text: text
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            exif: None,
        },
    )
}

/// The same picture with Cr and Cb swapped, as a confused Robot 36
/// decoder gives.
fn colours_swapped(img: &Image) -> Image {
    let mut out = img.clone();
    for p in out.rgb.chunks_mut(3) {
        let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let cr = 128.0 + 0.713 * (r - y);
        let cb = 128.0 + 0.564 * (b - y);
        let (cr, cb) = (cb, cr);
        let nr = y + 1.402 * (cr - 128.0);
        let ng = y - 0.344_136 * (cb - 128.0) - 0.714_136 * (cr - 128.0);
        let nb = y + 1.772 * (cb - 128.0);
        for (v, n) in p.iter_mut().zip([nr, ng, nb]) {
            *v = n.round().clamp(3.0, 255.0) as u8;
        }
    }
    out
}

fn input(name: &str, bytes: Vec<u8>) -> Input {
    Input {
        name: name.into(),
        bytes,
    }
}

fn quiet(_: usize, _: usize) {}

#[test]
fn png_jpeg_and_bmp_copies_are_read_and_grouped_together() {
    let s = scene();
    let jpg = formats::read(SCENE_JPG).unwrap();
    assert_eq!(jpg.format, Format::Jpeg);
    assert_eq!((jpg.image.width, jpg.image.height), (320, 240));
    let inputs = vec![
        input("a.png", png(&damaged(&s, 1, 0.3, 0..20), &[])),
        input("b.jpg", SCENE_JPG.to_vec()),
        input("c.bmp", bmp(&damaged(&s, 2, 0.3, 200..240))),
    ];
    let out = batch::run(inputs, &Settings::default(), &mut quiet);
    assert_eq!(out.summary.pictures, 1, "{:?}", out.summary);
    assert_eq!(out.summary.combined, 1);
    assert_eq!(out.made[0].copies, 3);
    assert!(out.summary.unreadable.is_empty());
}

#[test]
fn the_strongest_copys_details_and_exif_are_kept() {
    let s = scene();
    let exif = b"MM\0*\0\0\0\x08\0\0".to_vec();
    let jpg = with_jpeg_details(SCENE_JPG, &exif, "Robot 36 from M7XYZ");
    let inputs = vec![
        input(
            "pass 1/noisy.png",
            png(
                &damaged(&s, 3, 0.6, 0..60),
                &[("Callsign", "M7ABC"), ("Elevation", "20 degrees")],
            ),
        ),
        input("pass 2/clean.jpg", jpg),
        input(
            "pass 3/noisy too.png",
            png(&damaged(&s, 4, 0.6, 180..240), &[]),
        ),
    ];
    let out = batch::run(inputs, &Settings::default(), &mut quiet);
    assert_eq!(out.made.len(), 1);
    let m = &out.made[0];
    assert_eq!(m.strongest, "pass 2/clean.jpg");
    assert_eq!(m.file, "clean.png");
    let files: BTreeMap<_, _> = out.files.iter().cloned().collect();
    let made = formats::read(&files["clean.png"]).unwrap();
    assert_eq!(made.format, Format::Png);
    assert_eq!(made.details.exif.as_deref(), Some(&exif[..]));
    let t = &made.details;
    assert_eq!(t.field("Comment"), Some("Robot 36 from M7XYZ"));
    assert!(t.field("Combined from").unwrap().starts_with("3 copies: "));
    assert_eq!(t.field("Strongest copy"), Some("pass 2/clean.jpg"));
    assert!(t.field("Coverage").unwrap().starts_with("100%"));
    assert!(t.field("Combined by").unwrap().starts_with("SSTV Stacker "));
    // The other copies' details aren't mixed in.
    assert_eq!(t.field("Callsign"), None);
    assert!(files.contains_key("Records/clean.txt"));
}

#[test]
fn a_double_size_copy_joins_its_group_and_says_so() {
    let s = scene();
    let mut big = Image::new(640, 480);
    for y in 0..480 {
        for x in 0..640 {
            let p = s.pixel(x / 2, y / 2);
            let i = (y * 640 + x) * 3;
            big.rgb[i..i + 3].copy_from_slice(&p);
        }
    }
    let inputs = vec![
        input("small.png", png(&damaged(&s, 5, 0.3, 0..30), &[])),
        input("big.png", png(&damaged(&big, 6, 0.3, 400..480), &[])),
    ];
    let out = batch::run(inputs, &Settings::default(), &mut quiet);
    assert_eq!(out.made.len(), 1, "{:?}", out.summary);
    let m = &out.made[0];
    assert_eq!(m.copies, 2);
    assert!(m
        .record_json
        .as_deref()
        .unwrap()
        .contains("made 2 times smaller (saved at 640 by 480)"));
    let files: BTreeMap<_, _> = out.files.iter().cloned().collect();
    let made = formats::read(&files[&m.file]).unwrap();
    assert_eq!((made.image.width, made.image.height), (320, 240));
}

#[test]
fn a_copy_of_another_size_stays_apart() {
    let s = scene();
    let odd = Image::from_rgb(
        300,
        240,
        (0..240)
            .flat_map(|y| (0..300).flat_map(move |x| [(x % 256) as u8, y as u8, 99]))
            .collect(),
    )
    .unwrap();
    let inputs = vec![
        input("a.png", png(&s, &[])),
        input("b.png", png(&damaged(&s, 7, 0.3, 0..10), &[])),
        input("other.png", png(&odd, &[])),
    ];
    let out = batch::run(inputs, &Settings::default(), &mut quiet);
    assert_eq!(out.summary.pictures, 2);
    assert_eq!(out.summary.combined, 1);
}

/// Three copies the right way and one with its colours swapped, at a
/// size: whether the swap was put back.
fn swap_fixed(img: &Image, settings: &Settings) -> bool {
    let inputs = vec![
        input("1.png", png(&damaged(img, 11, 0.2, 0..0), &[])),
        input("2.png", png(&damaged(img, 12, 0.2, 0..0), &[])),
        input("3.png", png(&damaged(img, 13, 0.2, 0..0), &[])),
        input(
            "4.png",
            png(&damaged(&colours_swapped(img), 14, 0.2, 0..0), &[]),
        ),
    ];
    let out = batch::run(inputs, settings, &mut quiet);
    let json = out
        .made
        .iter()
        .find(|m| m.copies > 1)
        .and_then(|m| m.record_json.clone())
        .unwrap_or_default();
    json.contains("\"swapped\":true")
}

#[test]
fn swapped_colours_are_put_back_on_robot_36_only() {
    let s = scene();
    assert!(swap_fixed(&s, &Settings::default()));
    // A PD120-sized picture (640 by 496) can't swap like that: left alone
    // unless asked.
    let mut pd = Image::new(640, 496);
    for y in 0..496 {
        for x in 0..640 {
            let p = s.pixel(x / 2, (y / 2).min(239));
            let i = (y * 640 + x) * 3;
            pd.rgb[i..i + 3].copy_from_slice(&p);
        }
    }
    assert!(!swap_fixed(&pd, &Settings::default()));
    let on = Settings {
        colour_fix: ColourFix::On,
        ..Settings::default()
    };
    assert!(swap_fixed(&pd, &on));
    let off = Settings {
        colour_fix: ColourFix::Off,
        ..Settings::default()
    };
    assert!(!swap_fixed(&s, &off));
}

fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                let rel = p
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

fn temp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sstv-stacker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_mixed_folder_gives_one_file_per_picture_and_leaves_the_originals() {
    let dir = temp("mixed");
    let s = scene();
    let photo = Image::from_rgb(
        200,
        150,
        (0..150)
            .flat_map(|y| (0..200).flat_map(move |x| [(x + y) as u8, (x * 2) as u8, y as u8]))
            .collect(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("pass 1")).unwrap();
    std::fs::create_dir_all(dir.join("pass 2")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(
        dir.join("pass 1/a.png"),
        png(&damaged(&s, 21, 0.4, 0..40), &[]),
    )
    .unwrap();
    std::fs::write(
        dir.join("pass 2/a.png"),
        png(&damaged(&s, 22, 0.4, 150..240), &[]),
    )
    .unwrap();
    std::fs::write(
        dir.join("pass 2/b.bmp"),
        bmp(&damaged(&s, 23, 0.4, 60..120)),
    )
    .unwrap();
    // A scrap: 20 lines of something.
    std::fs::write(
        dir.join("pass 2/scrap.png"),
        png(&damaged(&photo_320(), 24, 0.0, 20..240), &[]),
    )
    .unwrap();
    std::fs::write(dir.join("photo.png"), png(&photo, &[])).unwrap();
    std::fs::write(dir.join("notes.txt"), b"not a picture").unwrap();
    std::fs::write(dir.join(".hidden/x.png"), png(&s, &[])).unwrap();
    std::fs::write(dir.join("broken.png"), b"\x89PNG\r\n\x1a\nnope").unwrap();
    let before = tree(&dir);

    let out = dir.join(folder::DEFAULT_OUT);
    let s1 = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(s1.copies, 5, "{s1:?}");
    assert_eq!(s1.pictures, 2, "{s1:?}");
    assert_eq!(s1.combined, 1);
    assert_eq!(s1.added, 2);
    assert_eq!(s1.scraps, 1);
    assert_eq!(s1.unreadable.len(), 1);
    let made = tree(&out);
    assert!(made.contains_key("photo.png"), "{:?}", made.keys());
    assert_eq!(
        made["photo.png"], before["photo.png"],
        "a single copy is passed on as it is"
    );
    assert!(made.contains_key("index.json"));
    assert_eq!(made.keys().filter(|k| k.starts_with("Records/")).count(), 1);

    // Again: nothing changes, and the originals never did.
    let s2 = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!((s2.added, s2.replaced, s2.kept), (0, 0, 2), "{s2:?}");
    let again = tree(&out);
    assert_eq!(
        again.keys().collect::<Vec<_>>(),
        made.keys().collect::<Vec<_>>()
    );
    for (k, v) in &made {
        assert!(again[k] == *v, "{k} changed");
    }
    let after = tree(&dir);
    for (k, v) in &before {
        assert_eq!(after.get(k), Some(v), "{k} changed");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_better_rebuild_replaces_and_keeps_the_old_one() {
    let dir = temp("better");
    let s = scene();
    std::fs::write(dir.join("1.png"), png(&damaged(&s, 31, 0.5, 0..60), &[])).unwrap();
    std::fs::write(dir.join("2.png"), png(&damaged(&s, 32, 0.5, 180..240), &[])).unwrap();
    let out = dir.join("out");
    let s1 = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(s1.added, 1);
    std::fs::write(dir.join("3.png"), png(&damaged(&s, 33, 0.05, 0..0), &[])).unwrap();
    std::fs::write(dir.join("4.png"), png(&damaged(&s, 34, 0.05, 0..0), &[])).unwrap();
    let s2 = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(s2.replaced, 1, "{s2:?}");
    let t = tree(&out);
    assert_eq!(
        t.keys().filter(|k| k.starts_with("Replaced/")).count(),
        2,
        "{:?}",
        t.keys()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Takes the method numbers out of `index.json`, as an index written
/// before they were numbered.
fn as_older_index(out: &Path) {
    let p = out.join("index.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    for e in v["pictures"].as_array_mut().unwrap() {
        e.as_object_mut().unwrap().remove("method");
    }
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[test]
fn a_picture_made_by_an_older_method_is_made_again_once() {
    let dir = temp("method");
    let s = scene();
    std::fs::write(dir.join("1.png"), png(&damaged(&s, 41, 0.5, 0..60), &[])).unwrap();
    std::fs::write(dir.join("2.png"), png(&damaged(&s, 42, 0.5, 180..240), &[])).unwrap();
    let out = dir.join("out");
    let first = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(first.added, 1);
    let made = tree(&out);

    // The same pixels again: nothing is touched, and it isn't made again
    // after that.
    as_older_index(&out);
    let again = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(
        (again.added, again.replaced, again.kept),
        (0, 0, 1),
        "{again:?}"
    );
    let t = tree(&out);
    for (k, v) in &made {
        if k != "index.json" {
            assert_eq!(t.get(k), Some(v), "{k} changed");
        }
    }
    let v: serde_json::Value = serde_json::from_slice(&t["index.json"]).unwrap();
    assert_eq!(v["pictures"][0]["method"], 1);

    // Different pixels (as a copy lined up wrongly made): replaced, even
    // though the old file's noise estimate looks better.
    let file = made
        .keys()
        .find(|k| k.ends_with(".png") && !k.contains('/'))
        .unwrap()
        .clone();
    std::fs::write(out.join(&file), png(&damaged(&s, 43, 0.5, 0..0), &[])).unwrap();
    as_older_index(&out);
    let idx = out.join("index.json");
    let text = String::from_utf8(std::fs::read(&idx).unwrap()).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["pictures"][0]["noise"] = serde_json::json!(0.0);
    std::fs::write(&idx, serde_json::to_vec(&v).unwrap()).unwrap();
    let fixed = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(fixed.replaced, 1, "{fixed:?}");
    assert_eq!(tree(&out).get(&file), made.get(&file));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stop_part_way_keeps_what_was_done() {
    let dir = temp("stop");
    let s = scene();
    std::fs::write(dir.join("1.png"), png(&s, &[])).unwrap();
    std::fs::write(dir.join("photo.png"), png(&photo_320(), &[])).unwrap();
    let out = dir.join("out");
    let asked = std::cell::Cell::new(0);
    let r = folder::update(&dir, &out, &Settings::default(), &|| {
        asked.set(asked.get() + 1);
        asked.get() > 1
    })
    .unwrap();
    assert!(r.stopped);
    assert_eq!(r.added, 1);
    let again = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!((again.added, again.kept), (1, 1), "{again:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_index_changes_nothing() {
    let dir = temp("index");
    std::fs::write(dir.join("1.png"), png(&scene(), &[])).unwrap();
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("index.json"), b"{ not json").unwrap();
    let err = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap_err();
    assert!(err.contains("nothing was changed"), "{err}");
    assert_eq!(tree(&out).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A different 320 by 240 picture.
fn photo_320() -> Image {
    Image::from_rgb(
        320,
        240,
        (0..240)
            .flat_map(|y| {
                (0..320).flat_map(move |x| {
                    let v = if (x / 40 + y / 30) % 2 == 0 { 220 } else { 30 };
                    [v, 255 - v, 128]
                })
            })
            .collect(),
    )
    .unwrap()
}

/// A 24-bit BMP of `img`, bottom-up.
fn bmp(img: &Image) -> Vec<u8> {
    let stride = (img.width * 3).div_ceil(4) * 4;
    let size = 54 + stride * img.height;
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(img.width as i32).to_le_bytes());
    b.extend_from_slice(&(img.height as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    for y in (0..img.height).rev() {
        let mut line = Vec::with_capacity(stride);
        for x in 0..img.width {
            let [r, g, bb] = img.pixel(x, y);
            line.extend_from_slice(&[bb, g, r]);
        }
        line.resize(stride, 0);
        b.extend_from_slice(&line);
    }
    b
}

/// A JPEG with an EXIF block and a comment spliced in after its start.
fn with_jpeg_details(jpeg: &[u8], exif: &[u8], comment: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(exif);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&[0xFF, 0xFE]);
    out.extend_from_slice(&((comment.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(comment.as_bytes());
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[test]
fn earlier_best_copies_are_not_read_again() {
    let s = scene();
    let first = vec![
        input("1.png", png(&damaged(&s, 41, 0.4, 0..60), &[])),
        input("2.png", png(&damaged(&s, 42, 0.4, 0..60), &[])),
    ];
    let out = batch::run(first.clone(), &Settings::default(), &mut quiet);
    let (name, made) = out
        .files
        .iter()
        .find(|(n, _)| n.ends_with(".png"))
        .cloned()
        .unwrap();
    assert_eq!(out.made[0].missing_rows, 60);
    // The result put back among the copies: left out, so the hatched
    // lines never count as received.
    let mut again = first;
    again.push(input(&format!("Best copies 2/{name}"), made));
    let out2 = batch::run(again, &Settings::default(), &mut quiet);
    assert_eq!(out2.summary.earlier, 1);
    assert_eq!(out2.summary.copies, 2);
    assert_eq!(out2.made[0].missing_rows, 60);
}

#[test]
fn an_exact_duplicate_is_counted_once_and_passed_on_as_it_was() {
    let b = bmp(&damaged(&scene(), 43, 0.3, 0..0));
    let out = batch::run(
        vec![input("x/r1.bmp", b.clone()), input("y/r1.bmp", b.clone())],
        &Settings::default(),
        &mut quiet,
    );
    assert_eq!(out.summary.duplicates, 1);
    assert_eq!(out.summary.combined, 0);
    assert_eq!(out.files, vec![("r1.bmp".to_string(), b)]);
}

#[test]
fn replacing_one_picture_leaves_anothers_record() {
    let dir = temp("records");
    let s = scene();
    let q = photo_320();
    for d in ["s1", "s2", "s3", "s4"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    // P received once, as a.bmp; Q twice, combined as a.png with a record.
    std::fs::write(dir.join("s1/a.bmp"), bmp(&damaged(&s, 51, 0.5, 0..0))).unwrap();
    std::fs::write(dir.join("s2/a.bmp"), bmp(&damaged(&q, 52, 0.3, 0..0))).unwrap();
    std::fs::write(dir.join("s3/a.bmp"), bmp(&damaged(&q, 53, 0.3, 0..0))).unwrap();
    let out = dir.join("out");
    folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    let before = tree(&out);
    assert!(
        before.contains_key("a.bmp") && before.contains_key("a.png"),
        "{:?}",
        before.keys()
    );
    let q_record = before["Records/a.txt"].clone();
    // A second, cleaner copy of P: P is rebuilt, Q's record stays put.
    std::fs::write(dir.join("s4/b.png"), png(&damaged(&s, 54, 0.05, 0..0), &[])).unwrap();
    std::fs::write(dir.join("s4/c.png"), png(&damaged(&s, 55, 0.05, 0..0), &[])).unwrap();
    let r = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap();
    assert_eq!(r.replaced, 1, "{r:?}");
    let after = tree(&out);
    assert_eq!(
        after.get("Records/a.txt"),
        Some(&q_record),
        "{:?}",
        after.keys()
    );
    assert!(after.contains_key("Replaced/a.bmp"), "{:?}", after.keys());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_index_naming_a_file_elsewhere_changes_nothing() {
    let dir = temp("escape");
    std::fs::write(dir.join("1.png"), png(&scene(), &[])).unwrap();
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("index.json"),
        br#"{"pictures":[{"file":"../1.png","sources":[],"ids":[],"noise":99}]}"#,
    )
    .unwrap();
    let err = folder::update(&dir, &out, &Settings::default(), &|| false).unwrap_err();
    assert!(err.contains("outside"), "{err}");
    assert!(dir.join("1.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn output_folders_and_links_are_not_read() {
    let dir = temp("skip");
    let s = scene();
    std::fs::write(dir.join("1.png"), png(&s, &[])).unwrap();
    std::fs::create_dir_all(dir.join("Best copies 2")).unwrap();
    std::fs::write(dir.join("Best copies 2/x.png"), png(&photo_320(), &[])).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
    let found = folder::find_pictures(&dir, &dir.join("out"));
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["1.png"]);
    let _ = std::fs::remove_dir_all(&dir);
}
