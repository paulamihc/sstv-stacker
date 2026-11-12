//! `sstv-stacker`: combine repeat copies of SSTV pictures in a folder.

use std::path::PathBuf;
use std::process::ExitCode;

use sstv_stacker::batch::{ColourFix, Settings};
use sstv_stacker::folder;

const HELP: &str = "\
SSTV Stacker: combine repeat copies of an SSTV picture into one better copy.
Nothing made up.

Usage: sstv-stacker <folder> [options]

Reads every PNG, JPEG and BMP picture in <folder> (and the folders inside
it), finds which are copies of the same picture, and files one best copy of
each, with a record of how it was made. The originals are never changed.

Options:
  --out <folder>        Where the best copies go (default: <folder>/Best copies)
  --colour-fix <when>   Put swapped Robot 36 colours back: auto (Robot 36
                        pictures only; the default), on or off
  -h, --help            Show this
  -V, --version         Show the version

Run it again after more pictures come in: only pictures with new copies are
rebuilt, and a file is replaced only by a better one (the old one moves to
Replaced/).";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut source: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut settings = Settings::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("sstv-stacker {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--out" => match args.next() {
                Some(o) => out = Some(o.into()),
                None => return fail("--out needs a folder"),
            },
            "--colour-fix" => {
                settings.colour_fix = match args.next().as_deref() {
                    Some("auto") => ColourFix::Auto,
                    Some("on") => ColourFix::On,
                    Some("off") => ColourFix::Off,
                    _ => return fail("--colour-fix is auto, on or off"),
                }
            }
            s if s.starts_with('-') => return fail(&format!("unknown option {s}")),
            s if source.is_none() => source = Some(s.into()),
            _ => return fail("give one folder"),
        }
    }
    let Some(source) = source else {
        println!("{HELP}");
        return ExitCode::from(2);
    };
    if !source.is_dir() {
        return fail(&format!("{} isn't a folder", source.display()));
    }
    let out = out.unwrap_or_else(|| source.join(folder::DEFAULT_OUT));
    match folder::update(&source, &out, &settings, &|| false) {
        Ok(summary) => {
            println!("{}", summary.text());
            for (name, why) in &summary.unreadable {
                eprintln!("  could not read {name}: {why}");
            }
            println!("Best copies: {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("sstv-stacker: {msg}");
    ExitCode::from(2)
}
