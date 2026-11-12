//! The web page's WebAssembly: SSTV Stacker's combining, run in the
//! browser so no picture leaves the computer.
//!
//! A plain C interface, so the page needs no generated glue:
//!
//! - `stacker_alloc(len)` gives a buffer for the input; the page fills it.
//! - `stacker_run(ptr, len, colour_fix, dos_time, dos_date)` reads it
//!   (and frees it), makes the best copies, and returns a pointer to the
//!   output; `stacker_output_len()` gives its length. The page reads it,
//!   then calls `stacker_free(ptr, len)`.
//! - While running it calls the page's `env.progress(done, of)`.
//!
//! Input: for each file, its name's length (u32, little-endian), the name
//! (UTF-8), its length (u32), its bytes.
//!
//! Output: the summary as JSON (u32 length then UTF-8), then the zip of
//! everything (u32 length then bytes), then the files themselves, each as
//! the input is (name, then bytes).

use std::cell::Cell;

use sstv_stacker::batch::{self, ColourFix, Input, Output, Settings};

pub mod zip;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn progress(done: u32, of: u32);
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn progress(_done: u32, _of: u32) {}

thread_local! {
    static OUTPUT_LEN: Cell<usize> = const { Cell::new(0) };
}

/// A buffer of `len` bytes for the page to fill.
#[no_mangle]
pub extern "C" fn stacker_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Frees the output of [`stacker_run`].
///
/// # Safety
/// `ptr` and `len` must be an output given by [`stacker_run`] (and
/// [`stacker_output_len`]), freed once.
#[no_mangle]
pub unsafe extern "C" fn stacker_free(ptr: *mut u8, len: usize) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

/// The length of the last output.
#[no_mangle]
pub extern "C" fn stacker_output_len() -> usize {
    OUTPUT_LEN.with(|l| l.get())
}

/// Makes the best copies. `colour_fix`: 0 auto, 1 on, 2 off. `dos_time`
/// and `dos_date`: now, for the zip's file times.
///
/// # Safety
/// `ptr` and `len` must be a buffer from [`stacker_alloc`], filled; it is
/// freed here.
#[no_mangle]
pub unsafe extern "C" fn stacker_run(
    ptr: *mut u8,
    len: usize,
    colour_fix: u32,
    dos_time: u32,
    dos_date: u32,
) -> *mut u8 {
    let input = Vec::from_raw_parts(ptr, len, len.max(1));
    let out = run(&input, colour_fix, (dos_time as u16, dos_date as u16));
    drop(input);
    // A boxed slice, so freeing it needs only its length.
    let out = out.into_boxed_slice();
    OUTPUT_LEN.with(|l| l.set(out.len()));
    Box::into_raw(out) as *mut u8
}

/// The whole run, from input to output bytes (see the module's notes).
pub fn run(input: &[u8], colour_fix: u32, when: (u16, u16)) -> Vec<u8> {
    let inputs = match parse(input) {
        Ok(i) => i,
        Err(e) => {
            return output(
                &format!("{{\"error\":{}}}", quote(&e)),
                &[],
                &Output::default(),
            )
        }
    };
    let settings = Settings {
        colour_fix: match colour_fix {
            1 => ColourFix::On,
            2 => ColourFix::Off,
            _ => ColourFix::Auto,
        },
        ..Settings::default()
    };
    let result = batch::run(inputs, &settings, &mut |done, of| unsafe {
        progress(done as u32, of as u32)
    });
    let zipped = zip::store(
        result
            .files
            .iter()
            .map(|(name, bytes)| (format!("Best copies/{name}"), bytes.as_slice())),
        when,
    );
    output(&summary_json(&result), &zipped, &result)
}

fn output(json: &str, zipped: &[u8], result: &Output) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, json.as_bytes());
    put(&mut out, zipped);
    for (name, bytes) in &result.files {
        put(&mut out, name.as_bytes());
        put(&mut out, bytes);
    }
    out
}

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

/// The input's files.
pub fn parse(mut b: &[u8]) -> Result<Vec<Input>, String> {
    let take = |b: &mut &[u8]| -> Result<Vec<u8>, String> {
        if b.len() < 4 {
            return Err("the input is cut short".into());
        }
        let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        if b.len() - 4 < n {
            return Err("the input is cut short".into());
        }
        let v = b[4..4 + n].to_vec();
        *b = &b[4 + n..];
        Ok(v)
    };
    let mut files = Vec::new();
    while !b.is_empty() {
        let name = String::from_utf8(take(&mut b)?).map_err(|_| "a file name isn't UTF-8")?;
        let bytes = take(&mut b)?;
        files.push(Input { name, bytes });
    }
    Ok(files)
}

fn summary_json(r: &Output) -> String {
    let s = &r.summary;
    let made: Vec<String> = r
        .made
        .iter()
        .map(|m| {
            format!(
                "{{\"file\":{},\"strongest\":{},\"copies\":{},\"coverage\":{:.4},\"noise\":{:.2},\"best_single_noise\":{:.2},\"missing_rows\":{},\"record\":{}}}",
                quote(&m.file),
                quote(&m.strongest),
                m.copies,
                m.coverage,
                fin(m.noise),
                fin(m.best_single_noise),
                m.missing_rows,
                m.record_json.as_deref().unwrap_or("null"),
            )
        })
        .collect();
    let bad: Vec<String> = s
        .unreadable
        .iter()
        .map(|(n, why)| format!("{{\"name\":{},\"why\":{}}}", quote(n), quote(why)))
        .collect();
    format!(
        "{{\"text\":{},\"copies\":{},\"pictures\":{},\"combined\":{},\"scraps\":{},\"unreadable\":[{}],\"made\":[{}]}}",
        quote(&s.text()),
        s.copies,
        s.pictures,
        s.combined,
        s.scraps,
        bad.join(","),
        made.join(",")
    )
}

fn fin(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        999.0
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sstv_stacker::formats::{self, Details};
    use sstv_stacker::Image;

    /// The made-up test picture, and a chequerboard.
    fn scene() -> Vec<u8> {
        include_bytes!("../../stacker/tests/fixtures/scene.png").to_vec()
    }

    fn chequers() -> Vec<u8> {
        let rgb = (0..240)
            .flat_map(|y| {
                (0..320).flat_map(move |x| {
                    let v = if (x / 40 + y / 30) % 2 == 0 { 220 } else { 30 };
                    [v, 255 - v, 128]
                })
            })
            .collect();
        formats::write_png(
            &Image::from_rgb(320, 240, rgb).unwrap(),
            &Details::default(),
        )
    }

    fn pack(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut b = Vec::new();
        for (n, bytes) in files {
            put(&mut b, n.as_bytes());
            put(&mut b, bytes);
        }
        b
    }

    fn take<'a>(b: &mut &'a [u8]) -> &'a [u8] {
        let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
        let v = &b[4..4 + n];
        *b = &b[4 + n..];
        v
    }

    /// The names and contents in a stored zip, checking each CRC.
    fn unzip(z: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while z[i..].starts_with(&[0x50, 0x4b, 3, 4]) {
            let u16_at = |k: usize| u16::from_le_bytes([z[i + k], z[i + k + 1]]) as usize;
            let crc = u32::from_le_bytes(z[i + 14..i + 18].try_into().unwrap());
            let size = u32::from_le_bytes(z[i + 18..i + 22].try_into().unwrap()) as usize;
            let (nl, el) = (u16_at(26), u16_at(28));
            let name = String::from_utf8(z[i + 30..i + 30 + nl].to_vec()).unwrap();
            let start = i + 30 + nl + el;
            let data = z[start..start + size].to_vec();
            assert_eq!(zip::crc32(&data), crc, "{name}");
            out.push((name, data));
            i = start + size;
        }
        assert!(z[i..].starts_with(&[0x50, 0x4b, 1, 2]), "central directory");
        out
    }

    #[test]
    fn the_page_gets_its_summary_zip_and_files() {
        // Two different copies of the scene (one with a line lost), and
        // another picture.
        let a = scene();
        let mut damaged = formats::read(&a).unwrap().image;
        let w = damaged.width;
        damaged.rgb[10 * w * 3..11 * w * 3].fill(0);
        let b = formats::write_png(&damaged, &Details::default());
        let c = chequers();
        let input = pack(&[
            ("pass 1/a.png", a),
            ("pass 2/b.png", b),
            ("pass 2/c.png", c.clone()),
            ("notes.png", b"not really".to_vec()),
        ]);
        let out = run(&input, 0, (0, 0x5b6c));
        let mut rest = out.as_slice();
        let json = String::from_utf8(take(&mut rest).to_vec()).unwrap();
        let zipped = take(&mut rest).to_vec();
        let mut files = Vec::new();
        while !rest.is_empty() {
            let n = String::from_utf8(take(&mut rest).to_vec()).unwrap();
            files.push((n, take(&mut rest).to_vec()));
        }
        assert!(json.contains("\"pictures\":2"), "{json}");
        assert!(json.contains("\"combined\":1"), "{json}");
        assert!(json.contains("\"name\":\"notes.png\""), "{json}");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"c.png"), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("Records/")), "{names:?}");
        // The zip holds the same files, in a "Best copies" folder.
        let z = unzip(&zipped);
        assert_eq!(z.len(), files.len());
        for ((zn, zd), (n, d)) in z.iter().zip(&files) {
            assert_eq!(zn, &format!("Best copies/{n}"));
            assert_eq!(zd, d);
        }
        // A picture received once comes back as it was.
        assert_eq!(files.iter().find(|(n, _)| n == "c.png").unwrap().1, c);
    }

    #[test]
    fn a_damaged_input_is_reported_not_crashed_on() {
        let out = run(&[5, 0, 0, 0, b'a'], 0, (0, 0));
        let mut rest = out.as_slice();
        let json = String::from_utf8(take(&mut rest).to_vec()).unwrap();
        assert!(json.contains("\"error\""), "{json}");
    }
}
