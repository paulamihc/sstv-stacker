//! Making best copies from a set of picture files, in memory: what the
//! web page uses, and what [`crate::folder`] builds on.
//!
//! - One best copy per picture. A picture received once is passed on as
//!   it is, byte for byte.
//! - A combined picture is a PNG named after its strongest copy, carrying
//!   the strongest copy's details unchanged, with four fields added:
//!   "Combined from", "Strongest copy", "Coverage" and "Combined by".
//! - Each combined picture has a record (`Records/<name>.txt`).
//! - Scraps (less than a third of their lines received, matching nothing)
//!   are left out: there's no telling which picture they are.

use std::collections::BTreeSet;

use crate::formats::{self, Details, Picture};
use crate::record::{self, CopyInfo};
use crate::{sizes, stack, Combined, Image, Options};

/// A picture with less than this share of its lines received, on any
/// copy, is a scrap and isn't filed.
pub const MIN_COVERAGE: f64 = 1.0 / 3.0;
/// The folder records go in.
pub const RECORDS: &str = "Records";

/// Whether to put swapped Robot 36 colours back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColourFix {
    /// For Robot 36 pictures only (see [`crate::colour_fix_suits`]).
    #[default]
    Auto,
    On,
    Off,
}

/// How to make best copies.
#[derive(Debug, Clone)]
pub struct Settings {
    pub colour_fix: ColourFix,
    /// What made them, for the "Combined by" field, such as
    /// "SSTV Stacker 0.1.0".
    pub made_by: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            colour_fix: ColourFix::Auto,
            made_by: format!("SSTV Stacker {}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// A file given to read.
#[derive(Debug, Clone)]
pub struct Input {
    /// Its name: a file name, or a path inside the folder, with '/'.
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A picture read, ready to combine.
#[derive(Debug, Clone)]
pub struct Copy {
    pub name: String,
    /// The file as it was.
    pub bytes: Vec<u8>,
    pub picture: Picture,
    /// Its fingerprint ([`Image::fingerprint`]).
    pub id: String,
    /// The picture at the size it is combined at.
    pub image: Image,
    /// Anything done to it before combining.
    pub note: Option<String>,
}

impl Copy {
    /// A satellite's elevation when it was received, degrees, if the
    /// details say: settles a tie for the strongest copy.
    fn elevation(&self) -> Option<f64> {
        self.picture
            .details
            .field("Elevation")?
            .split_whitespace()
            .next()?
            .trim_end_matches('°')
            .parse()
            .ok()
    }
}

/// What [`load`] found.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    /// The copies, at the size each is combined at.
    pub copies: Vec<Copy>,
    /// Files that couldn't be read, with why.
    pub unreadable: Vec<(String, String)>,
    /// Best copies made earlier (by this tool or another that marks its
    /// results the same way), which aren't read again: their hatched
    /// missing lines would count as received, and their copies twice.
    pub earlier: Vec<String>,
    /// Exact duplicates of a copy already read (the same picture in two
    /// places), counted once: (the duplicate, the copy it repeats).
    pub duplicates: Vec<(String, String)>,
}

/// A best copy made earlier: its details say what combined it.
pub fn is_earlier_result(details: &Details) -> bool {
    details.field("Combined by").is_some()
}

/// Reads every file it can. Earlier best copies and exact duplicates are
/// left out (see [`Loaded`]).
pub fn load(inputs: Vec<Input>) -> Loaded {
    let mut out = Loaded::default();
    let mut read: Vec<(Input, Picture)> = Vec::new();
    let mut seen: std::collections::HashMap<String, String> = Default::default();
    for i in inputs {
        // A reader that panics on a strange file loses only that file.
        let r = std::panic::catch_unwind(|| formats::read(&i.bytes))
            .unwrap_or_else(|_| Err("the file couldn't be read (the reader failed)".into()));
        match r {
            Ok(p) if is_earlier_result(&p.details) => out.earlier.push(i.name),
            Ok(p) => {
                let id = p.image.fingerprint();
                if let Some(first) = seen.get(&id) {
                    out.duplicates.push((i.name, first.clone()));
                } else {
                    seen.insert(id, i.name.clone());
                    read.push((i, p));
                }
            }
            Err(e) => out.unreadable.push((i.name, e)),
        }
    }
    let dims: Vec<(usize, usize)> = read
        .iter()
        .map(|(_, p)| (p.image.width, p.image.height))
        .collect();
    let targets = sizes::targets(&dims);
    out.copies = read
        .into_iter()
        .zip(targets)
        .map(|((input, picture), (_, factor))| {
            let image = sizes::shrink(&picture.image, factor);
            let note = (factor > 1).then(|| {
                format!(
                    "made {factor} times smaller (saved at {} by {})",
                    picture.image.width, picture.image.height
                )
            });
            Copy {
                id: picture.image.fingerprint(),
                name: input.name,
                bytes: input.bytes,
                picture,
                image,
                note,
            }
        })
        .collect();
    out
}

/// The copies of each picture: lists of indices into `copies`.
pub fn groups(copies: &[Copy]) -> Vec<Vec<usize>> {
    let images: Vec<&Image> = copies.iter().map(|c| &c.image).collect();
    stack::group(&images)
}

/// A best copy made.
#[derive(Debug, Clone)]
pub struct Built {
    /// The name it wants (not yet checked against others).
    pub name: String,
    /// The file.
    pub bytes: Vec<u8>,
    /// The copies it was made from (indices into the copies given).
    pub members: Vec<usize>,
    /// The strongest copy (an index into the copies given).
    pub strongest: usize,
    pub combined: Combined,
    /// The record, for a picture made from more than one copy.
    pub record: Option<String>,
    pub record_json: Option<String>,
}

impl Built {
    /// Whether it is a scrap: too little received to tell which picture
    /// it is.
    pub fn is_scrap(&self) -> bool {
        self.combined.coverage < MIN_COVERAGE
    }
}

/// Makes the best copy of one picture from its copies (`members`, as
/// [`groups`] found them).
pub fn build(copies: &[Copy], members: &[usize], settings: &Settings) -> Built {
    let ms: Vec<&Copy> = members.iter().map(|&i| &copies[i]).collect();
    let imgs: Vec<&Image> = ms.iter().map(|c| &c.image).collect();
    let tie: Vec<Option<f64>> = ms.iter().map(|c| c.elevation()).collect();
    let colour_fix = match settings.colour_fix {
        ColourFix::On => true,
        ColourFix::Off => false,
        ColourFix::Auto => ms.iter().any(|c| {
            crate::colour_fix_suits(c.image.width, c.image.height, &c.picture.details.text)
        }),
    };
    let combined = stack::combine_with(&imgs, &tie, Options { colour_fix });
    let strongest = ms[combined.strongest];
    if ms.len() == 1 {
        return Built {
            name: base_name(&strongest.name),
            bytes: strongest.bytes.clone(),
            members: members.to_vec(),
            strongest: members[combined.strongest],
            combined,
            record: None,
            record_json: None,
        };
    }
    let name = format!("{}.png", stem(&strongest.name));
    let details = details(&ms, &combined, &settings.made_by);
    let bytes = formats::write_png(&combined.image, &details);
    let info: Vec<CopyInfo> = ms
        .iter()
        .map(|c| CopyInfo {
            name: c.name.clone(),
            note: c.note.clone(),
        })
        .collect();
    Built {
        record: Some(record::text(&info, &combined, &name)),
        record_json: Some(record::json(&info, &combined, &name)),
        name,
        bytes,
        members: members.to_vec(),
        strongest: members[combined.strongest],
        combined,
    }
}

/// The details for a combined picture: the strongest copy's, unchanged
/// (but for "Complete", which says "yes" only if every line came in on
/// at least one copy), then what says it is combined.
fn details(copies: &[&Copy], c: &Combined, made_by: &str) -> Details {
    let strongest = copies[c.strongest];
    let complete = if c.missing_rows.is_empty() {
        "yes"
    } else {
        "no"
    };
    let mut text: Vec<(String, String)> = strongest
        .picture
        .details
        .text
        .iter()
        .map(|(k, v)| {
            if k == "Complete" {
                (k.clone(), complete.to_string())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect();
    let used: Vec<&str> = c.used.iter().map(|&i| copies[i].name.as_str()).collect();
    text.push((
        "Combined from".into(),
        format!("{} copies: {}", used.len(), used.join("; ")),
    ));
    text.push(("Strongest copy".into(), strongest.name.clone()));
    text.push((
        "Coverage".into(),
        format!(
            "{:.0}% of lines received on at least one copy",
            c.coverage * 100.0
        ),
    ));
    text.push((
        "Combined by".into(),
        format!("{made_by}: copies lined up and averaged where they agree; nothing added"),
    ));
    Details {
        text,
        exif: strongest.picture.details.exif.clone(),
    }
}

/// The last part of a name with '/'.
pub fn base_name(name: &str) -> String {
    let b = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if b.is_empty() {
        "picture".into()
    } else {
        b.to_string()
    }
}

/// A name without its folder and extension.
pub fn stem(name: &str) -> String {
    let b = base_name(name);
    match b.rfind('.') {
        Some(i) if i > 0 => b[..i].to_string(),
        _ => b,
    }
}

/// `name`, or "name 2.png", "name 3.png"… if taken.
pub fn free_name(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let (st, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut n = 1;
    let mut candidate = name.to_string();
    while taken(&candidate) {
        n += 1;
        candidate = format!("{st} {n}{ext}");
    }
    candidate
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    /// Pictures read.
    pub copies: usize,
    /// Distinct pictures found (scraps not counted).
    pub pictures: usize,
    /// Of those, received more than once.
    pub combined: usize,
    /// Files written for the first time.
    pub added: usize,
    /// Files replaced by a better one.
    pub replaced: usize,
    /// Files left as they were (nothing new, or no better).
    pub kept: usize,
    /// Copies with too little received to tell which picture they are.
    pub scraps: usize,
    /// Files that couldn't be read, with why.
    pub unreadable: Vec<(String, String)>,
    /// Earlier best copies found among the pictures, not read again.
    pub earlier: usize,
    /// Exact duplicates, counted once.
    pub duplicates: usize,
    /// Stopped part way; what was done is kept.
    pub stopped: bool,
}

impl Summary {
    pub fn text(&self) -> String {
        let mut s = format!(
            "{} from {} ({} received more than once): {} added, {} replaced, {} unchanged",
            plural(self.pictures, "picture"),
            plural(self.copies, "copy"),
            self.combined,
            self.added,
            self.replaced,
            self.kept
        );
        if self.scraps > 0 {
            s += &format!(
                "; {} left out, with too few lines to tell which picture {}",
                self.scraps,
                if self.scraps == 1 {
                    "it is"
                } else {
                    "they are"
                }
            );
        }
        if !self.unreadable.is_empty() {
            s += &format!("; {} could not be read", self.unreadable.len());
        }
        if self.earlier > 0 {
            s += &format!(
                "; {} from an earlier run {} not read again",
                plural(self.earlier, "best copy"),
                if self.earlier == 1 { "was" } else { "were" }
            );
        }
        if self.duplicates > 0 {
            s += &format!(
                "; {} counted once",
                plural(self.duplicates, "exact duplicate")
            );
        }
        if self.stopped {
            s += "; stopped part way, the rest is done next time";
        }
        s
    }
}

/// "1 picture", "2 pictures"; "1 copy", "2 copies".
fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else if let Some(stem) = one.strip_suffix('y') {
        format!("{n} {stem}ies")
    } else {
        format!("{n} {one}s")
    }
}

/// One picture of a run, for showing.
#[derive(Debug, Clone)]
pub struct Made {
    /// Its file, in the output.
    pub file: String,
    /// The strongest copy's name.
    pub strongest: String,
    /// How many copies it was made from.
    pub copies: usize,
    pub coverage: f64,
    pub noise: f64,
    pub best_single_noise: f64,
    pub missing_rows: usize,
    /// The record as JSON, for a picture made from more than one copy.
    pub record_json: Option<String>,
}

/// What [`run`] gives: the files to save (path, bytes), with '/' between
/// folders, and what was done.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub files: Vec<(String, Vec<u8>)>,
    pub made: Vec<Made>,
    pub summary: Summary,
}

/// Makes best copies of everything in `inputs`, from scratch. `progress`
/// is told (done, of) after each picture.
pub fn run(
    inputs: Vec<Input>,
    settings: &Settings,
    progress: &mut dyn FnMut(usize, usize),
) -> Output {
    let loaded = load(inputs);
    let copies = loaded.copies;
    let mut out = Output {
        summary: Summary {
            copies: copies.len(),
            unreadable: loaded.unreadable,
            earlier: loaded.earlier.len(),
            duplicates: loaded.duplicates.len(),
            ..Default::default()
        },
        ..Default::default()
    };
    let gs = groups(&copies);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let n = gs.len();
    for (k, g) in gs.into_iter().enumerate() {
        let b = build(&copies, &g, settings);
        progress(k + 1, n);
        if b.is_scrap() {
            out.summary.scraps += 1;
            continue;
        }
        out.summary.pictures += 1;
        out.summary.added += 1;
        if g.len() > 1 {
            out.summary.combined += 1;
        }
        let lower: BTreeSet<String> = taken.iter().map(|t| t.to_lowercase()).collect();
        let file = free_name(&b.name, &|c| lower.contains(&c.to_lowercase()));
        taken.insert(file.clone());
        if let Some(rec) = &b.record {
            out.files.push((
                format!("{RECORDS}/{}.txt", stem(&file)),
                rec.clone().into_bytes(),
            ));
        }
        out.made.push(Made {
            file: file.clone(),
            strongest: copies[b.strongest].name.clone(),
            copies: g.len(),
            coverage: b.combined.coverage,
            noise: b.combined.noise,
            best_single_noise: record::best_single(&b.combined),
            missing_rows: b.combined.missing_rows.len(),
            record_json: b.record_json.clone(),
        });
        out.files.push((file, b.bytes));
    }
    out
}
