//! Copies saved at different sizes.
//!
//! Some programs save a picture at twice (or more) its size. Such a copy
//! is combined with the others by averaging it down, which adds nothing.
//! Copies of sizes that aren't an exact multiple of each other are never
//! combined.

use crate::Image;

/// A row whose pixels are all at most this was never received (the same
/// rule as the combining).
const MISSING_MAX: u8 = 2;

/// For each size in `sizes` (width, height), the size it is combined at
/// and the whole number it is divided by: the smallest other size present
/// that it is an exact multiple of (the same multiple across and down),
/// else its own size and 1.
pub fn targets(sizes: &[(usize, usize)]) -> Vec<((usize, usize), usize)> {
    sizes
        .iter()
        .map(|&(w, h)| {
            sizes
                .iter()
                .filter(|&&(sw, sh)| sw > 0 && sh > 0 && sw < w)
                .filter(|&&(sw, sh)| w % sw == 0 && h % sh == 0 && w / sw == h / sh)
                .min_by_key(|&&(sw, _)| sw)
                .map(|&(sw, sh)| ((sw, sh), w / sw))
                .unwrap_or(((w, h), 1))
        })
        .collect()
}

/// `img` made `factor` times smaller, each pixel the average of a square
/// of `factor` by `factor`. Rows never received are left out of the
/// average, so a line received on any of its rows stays received, and a
/// line received on none stays missing (black).
pub fn shrink(img: &Image, factor: usize) -> Image {
    if factor <= 1 {
        return img.clone();
    }
    let (w, h) = (img.width / factor, img.height / factor);
    let mut out = Image::new(w, h);
    for y in 0..h {
        let rows: Vec<usize> = (y * factor..(y + 1) * factor)
            .filter(|&sy| img.row(sy).iter().any(|&v| v > MISSING_MAX))
            .collect();
        if rows.is_empty() {
            continue;
        }
        for x in 0..w {
            let mut sum = [0u32; 3];
            for &sy in &rows {
                for sx in x * factor..(x + 1) * factor {
                    for (s, v) in sum.iter_mut().zip(img.pixel(sx, sy)) {
                        *s += v as u32;
                    }
                }
            }
            let n = (rows.len() * factor) as u32;
            let i = (y * w + x) * 3;
            for (o, s) in out.rgb[i..i + 3].iter_mut().zip(sum) {
                *o = ((s + n / 2) / n) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_size_copy_is_combined_at_the_smaller_size() {
        let t = targets(&[(320, 240), (640, 480), (640, 496), (960, 720)]);
        assert_eq!(t[0], ((320, 240), 1));
        assert_eq!(t[1], ((320, 240), 2));
        // Not a multiple of anything here: on its own.
        assert_eq!(t[2], ((640, 496), 1));
        assert_eq!(t[3], ((320, 240), 3));
    }

    #[test]
    fn shrinking_averages_and_keeps_missing_lines_missing() {
        let mut img = Image::new(4, 4);
        // Rows 0 and 1 received (10 and 30); rows 2 and 3 missing.
        for x in 0..4 {
            for c in 0..3 {
                img.rgb[(x) * 3 + c] = 10;
                img.rgb[(4 + x) * 3 + c] = 30;
            }
        }
        let s = shrink(&img, 2);
        assert_eq!((s.width, s.height), (2, 2));
        assert_eq!(s.pixel(0, 0), [20, 20, 20]);
        assert_eq!(s.pixel(1, 1), [0, 0, 0]);
    }

    #[test]
    fn a_line_received_on_one_of_its_rows_stays_received() {
        let mut img = Image::new(2, 2);
        for c in 0..3 {
            img.rgb[c] = 100;
            img.rgb[3 + c] = 100;
        }
        // Row 1 missing: the average is of row 0 alone, not darkened.
        assert_eq!(shrink(&img, 2).pixel(0, 0), [100, 100, 100]);
    }
}
