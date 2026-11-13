//! Best copies on disk: reads every picture in a folder (and the folders
//! inside it) and files one best copy of each in an output folder, with
//! records. What the command line uses.
//!
//! - The originals are never changed.
//! - Run again after more pictures come in, a picture is rebuilt from all
//!   its copies and replaces its file only if it scores better; the file
//!   replaced moves to `Replaced/`. Nothing is deleted.
//! - `index.json` remembers which copies (by fingerprint, so moving or
//!   renaming them doesn't matter) made each best copy. It is saved after
//!   every picture, whole or not at all.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::batch::{self, free_name, stem, Input, Settings, Summary, RECORDS};

/// The output folder's usual name, inside the folder read.
pub const DEFAULT_OUT: &str = "Best copies";
const INDEX: &str = "index.json";
const REPLACED: &str = "Replaced";
/// How deep folders are read.
const DEPTH: usize = 8;

/// One entry of `index.json`: a best copy and what it was made from.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    file: String,
    /// Where its copies were, for people reading the index.
    sources: Vec<String>,
    /// Its copies' fingerprints.
    ids: Vec<String>,
    /// The estimated noise (lower is better).
    noise: f64,
    /// Its record's file name in `Records/`, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record: Option<String>,
    /// The combining method it was made by ([`METHOD`]; 0 in an index
    /// from before methods were numbered).
    #[serde(default)]
    method: u32,
}

/// The combining method, numbered so that pictures made by an older one
/// are made again once: 1 lines up copies decoded many lines out of place,
/// and leaves out those it can't.
const METHOD: u32 = 1;

/// Whether two picture files hold the same pixels.
fn same_pixels(a: &[u8], b: &[u8]) -> bool {
    match (crate::formats::read(a), crate::formats::read(b)) {
        (Ok(a), Ok(b)) => {
            a.image.width == b.image.width
                && a.image.height == b.image.height
                && a.image.rgb == b.image.rgb
        }
        _ => false,
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    pictures: Vec<Entry>,
}

/// Whether a folder is an output folder: named "Best copies" (or "Best
/// copies 2"…, as the web page saves them), or holding an `index.json`.
fn is_output(name: &str, path: &Path) -> bool {
    let n = name.to_lowercase();
    let named = n == "best copies"
        || n.strip_prefix("best copies ")
            .is_some_and(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_digit()));
    named || path.join(INDEX).is_file()
}

/// Whether a name from `index.json` is a plain file name in the output
/// folder (not a path that could point anywhere else).
fn plain_name(n: &str) -> bool {
    let mut parts = Path::new(n).components();
    matches!(parts.next(), Some(std::path::Component::Normal(_)))
        && parts.next().is_none()
        && !n.contains(['/', '\\'])
}

/// Every picture file under `dir`, as (path with '/', full path), leaving
/// out `skip` (the output folder), other output folders, hidden folders
/// and folders reached through a link.
pub fn find_pictures(dir: &Path, skip: &Path) -> Vec<(String, PathBuf)> {
    let skip = skip.canonicalize().unwrap_or_else(|_| skip.to_path_buf());
    let mut out = Vec::new();
    let mut todo = vec![(dir.to_path_buf(), String::new(), 0)];
    while let Some((d, rel, depth)) = todo.pop() {
        if d.canonicalize().map(|c| c == skip).unwrap_or(false) {
            continue;
        }
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let path = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if depth < DEPTH && !is_output(&name, &path) {
                    todo.push((path, r, depth + 1));
                }
            } else if (kind.is_file() || kind.is_symlink() && path.is_file())
                && crate::formats::Format::by_name(&name)
            {
                out.push((r, path));
            }
        }
    }
    out.sort();
    out
}

/// Writes a file whole or not at all: to a temporary name, then renamed.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn save_index(path: &Path, index: &Index) -> Result<(), String> {
    let json = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
    write_whole(path, json.as_bytes())
}

fn mkdir(d: &Path) -> Result<(), String> {
    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))
}

/// Makes or brings up to date the best copies of the pictures in `source`
/// (and the folders inside it), in `out`. `stop` is asked before each
/// picture: when it says so, the run ends there, with what was done kept.
pub fn update(
    source: &Path,
    out: &Path,
    settings: &Settings,
    stop: &dyn Fn() -> bool,
) -> Result<Summary, String> {
    let mut inputs = Vec::new();
    let mut unreadable = Vec::new();
    for (rel, path) in find_pictures(source, out) {
        match std::fs::read(&path) {
            Ok(bytes) => inputs.push(Input { name: rel, bytes }),
            Err(e) => unreadable.push((rel, e.to_string())),
        }
    }
    let loaded = batch::load(inputs);
    let copies = loaded.copies;
    unreadable.extend(loaded.unreadable);
    let mut summary = Summary {
        copies: copies.len(),
        unreadable,
        earlier: loaded.earlier.len(),
        duplicates: loaded.duplicates.len(),
        ..Default::default()
    };
    if copies.is_empty() {
        return Ok(summary);
    }
    mkdir(out)?;
    let index_path = out.join(INDEX);
    // A damaged index stops the run: carrying on would file everything
    // again beside the old files.
    let mut index: Index = match std::fs::read_to_string(&index_path) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| {
            format!(
                "{} can't be read ({e}); nothing was changed",
                index_path.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Index::default(),
        Err(e) => return Err(format!("{}: {e}", index_path.display())),
    };
    // Names in the index are only ever plain file names here: an edited
    // one mustn't move a file from anywhere else.
    if index
        .pictures
        .iter()
        .any(|e| !plain_name(&e.file) || e.record.as_deref().is_some_and(|r| !plain_name(r)))
    {
        return Err(format!(
            "{} names a file outside its folder; nothing was changed",
            index_path.display()
        ));
    }
    let mut records_taken: BTreeSet<String> = index
        .pictures
        .iter()
        .filter_map(|e| e.record.as_ref().map(|r| r.to_lowercase()))
        .collect();
    let mut claimed = vec![false; index.pictures.len()];
    let mut taken: BTreeSet<String> = index
        .pictures
        .iter()
        .map(|e| e.file.to_lowercase())
        .collect();
    for g in batch::groups(&copies) {
        if stop() {
            summary.stopped = true;
            break;
        }
        let ids: BTreeSet<String> = g.iter().map(|&i| copies[i].id.clone()).collect();
        let sources: Vec<String> = g.iter().map(|&i| copies[i].name.clone()).collect();
        // The entry this picture already has: the one most of whose copies
        // are in this group (so two groups can never share one).
        let existing = index
            .pictures
            .iter()
            .enumerate()
            .filter(|(i, _)| !claimed[*i])
            .map(|(i, e)| {
                (
                    i,
                    e.ids.iter().filter(|s| ids.contains(*s)).count(),
                    e.ids.len(),
                )
            })
            .filter(|&(_, shared, of)| shared * 2 > of)
            .max_by_key(|&(_, shared, _)| shared)
            .map(|(i, _, _)| i);
        if let Some(i) = existing {
            claimed[i] = true;
            let e = &index.pictures[i];
            let same: BTreeSet<String> = e.ids.iter().cloned().collect();
            if same == ids && out.join(&e.file).exists() && e.method >= METHOD {
                summary.pictures += 1;
                summary.combined += usize::from(g.len() > 1);
                summary.kept += 1;
                continue;
            }
        }
        let b = batch::build(&copies, &g, settings);
        if existing.is_none() && b.is_scrap() {
            summary.scraps += 1;
            continue;
        }
        summary.pictures += 1;
        summary.combined += usize::from(g.len() > 1);
        if let Some(i) = existing {
            let e = &mut index.pictures[i];
            let path = out.join(&e.file);
            let older = e.method < METHOD;
            // Made by an older method: the same pixels again are kept as
            // they are; different ones replace it, whatever the noise
            // estimates say (an older method could misjudge them).
            let unchanged =
                older && std::fs::read(&path).is_ok_and(|old| same_pixels(&old, &b.bytes));
            if unchanged || (!older && path.exists() && b.combined.noise >= e.noise - 0.05) {
                // No better: kept, with the copies looked at remembered so
                // the same set isn't worked through again.
                e.ids = ids.into_iter().collect();
                e.sources = sources;
                if unchanged {
                    e.method = METHOD;
                    e.noise = b.combined.noise;
                }
                summary.kept += 1;
                save_index(&index_path, &index)?;
                continue;
            }
        }
        // A replaced file (and its record) moves out first, so its name is
        // free again.
        if let Some(i) = existing {
            let old = index.pictures[i].file.clone();
            let old_record = index.pictures[i].record.clone();
            let from = out.join(&old);
            if from.exists() {
                let replaced = out.join(REPLACED);
                mkdir(&replaced)?;
                // A name free for both the picture and its record.
                let to = free_name(&old, &|c| {
                    replaced.join(c).exists() || replaced.join(format!("{}.txt", stem(c))).exists()
                });
                std::fs::rename(&from, replaced.join(&to))
                    .map_err(|e| format!("{}: {e}", from.display()))?;
                if let Some(r) = &old_record {
                    let old_rec = out.join(RECORDS).join(r);
                    if old_rec.exists() {
                        std::fs::rename(&old_rec, replaced.join(format!("{}.txt", stem(&to))))
                            .map_err(|e| format!("{}: {e}", old_rec.display()))?;
                    }
                    records_taken.remove(&r.to_lowercase());
                }
                summary.replaced += 1;
            } else {
                summary.added += 1;
            }
            taken.remove(&old.to_lowercase());
        } else {
            summary.added += 1;
        }
        let file = free_name(&b.name, &|c| {
            taken.contains(&c.to_lowercase()) || out.join(c).exists()
        });
        taken.insert(file.to_lowercase());
        write_whole(&out.join(&file), &b.bytes)?;
        let mut record = None;
        if let Some(rec) = &b.record {
            let records = out.join(RECORDS);
            mkdir(&records)?;
            let name = free_name(&format!("{}.txt", stem(&file)), &|c| {
                records_taken.contains(&c.to_lowercase()) || records.join(c).exists()
            });
            records_taken.insert(name.to_lowercase());
            write_whole(&records.join(&name), rec.as_bytes())?;
            record = Some(name);
        }
        let entry = Entry {
            file,
            sources,
            ids: ids.into_iter().collect(),
            noise: b.combined.noise,
            record,
            method: METHOD,
        };
        match existing {
            Some(i) => index.pictures[i] = entry,
            None => {
                index.pictures.push(entry);
                claimed.push(true);
            }
        }
        // Saved after every picture, so a stop or a failure part way
        // leaves the index matching the files.
        save_index(&index_path, &index)?;
    }
    save_index(&index_path, &index)?;
    Ok(summary)
}
