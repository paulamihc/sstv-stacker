//! # SSTV Stacker
//!
//! Combine repeat copies of an SSTV picture into one better copy. Nothing
//! made up.
//!
//! When the same picture is received several times (an SSTV event on the
//! ISS sends the same set for days), every copy is damaged in different
//! places. This crate finds which copies are of the same picture
//! ([`group`]) and combines them ([`combine`]), keeping to three rules:
//!
//! 1. Every pixel of the result comes from the copies at that place: a
//!    weighted average of the copies that agree there, so each value lies
//!    between the lowest and highest the copies hold. No AI, no
//!    inpainting, no upscaling, no guessing between lines.
//! 2. A line no copy received stays missing, drawn as a grey hatched band.
//! 3. The only corrections undo decoding errors: lining copies up (a
//!    shift), and putting swapped Robot 36 colours back.
//!
//! With the `files` feature (on by default) it also reads PNG, JPEG and
//! BMP files ([`formats`]), keeps the strongest copy's details, and files
//! one best copy of each picture with a record of how it was made
//! ([`batch`] in memory, [`folder`] on disk).

pub mod record;
pub mod sizes;
pub mod stack;

#[cfg(feature = "files")]
pub mod batch;
#[cfg(feature = "files")]
pub mod folder;
#[cfg(feature = "files")]
pub mod formats;

pub use stack::{combine, combine_with, group, Combined, Options, RowSources};

/// An RGB picture, 8 bits a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// Row by row, 3 bytes per pixel.
    pub rgb: Vec<u8>,
}

impl Image {
    /// A black picture.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            rgb: vec![0; width * height * 3],
        }
    }

    /// A picture from its pixels, row by row, 3 bytes each.
    pub fn from_rgb(width: usize, height: usize, rgb: Vec<u8>) -> Option<Self> {
        (rgb.len() == width.checked_mul(height)?.checked_mul(3)?).then_some(Self {
            width,
            height,
            rgb,
        })
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    pub fn row(&self, y: usize) -> &[u8] {
        &self.rgb[y * self.width * 3..(y + 1) * self.width * 3]
    }

    /// A fingerprint of the picture (its size and pixels; FNV-1a, 64 bits,
    /// as 16 hex digits): the same picture has the same fingerprint
    /// wherever it is moved or whatever it is renamed to.
    pub fn fingerprint(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in (self.width as u32)
            .to_le_bytes()
            .iter()
            .chain(&(self.height as u32).to_le_bytes())
            .chain(&self.rgb)
        {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// RGB to Y, Cr (R-Y) and Cb (B-Y), full range, as SSTV carries them.
pub(crate) fn rgb_to_ycc([r, g, b]: [u8; 3]) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64, g as f64, b as f64);
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cr = 128.0 + 0.713 * (r - y);
    let cb = 128.0 + 0.564 * (b - y);
    (y, cr, cb)
}

pub(crate) fn ycc_to_rgb(y: f64, cr: f64, cb: f64) -> [u8; 3] {
    let r = y + 1.402 * (cr - 128.0);
    let g = y - 0.344_136 * (cb - 128.0) - 0.714_136 * (cr - 128.0);
    let b = y + 1.772 * (cb - 128.0);
    [r, g, b].map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// Whether the Robot 36 colour fix suits pictures of this size with these
/// details: yes for Robot 36's own size, 320 by 240, or when the details
/// name Robot 36 (or Robot 24, which swaps the same way). Other modes send
/// all their colour on every line, so their colours can't swap like that.
pub fn colour_fix_suits(width: usize, height: usize, details: &[(String, String)]) -> bool {
    let named = details.iter().any(|(_, v)| {
        let v: String = v
            .to_ascii_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        v.contains("robot36") || v.contains("robot24")
    });
    named || (width, height) == (320, 240)
}
