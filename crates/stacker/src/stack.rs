//! Finding and combining repeated copies of one SSTV picture.
//!
//! An SSTV event (on the ISS, for example) sends the same set of pictures
//! for days, so a station collects several copies of each, every one
//! damaged in different places: noisy
//! lines, lines never received (black), small shifts, and now and then a
//! Robot 36 picture with its colours swapped. This module finds the copies
//! of each picture and combines them.
//!
//! The rules, so nothing is invented:
//!
//! - Every pixel of the result comes from the copies at that place: a
//!   weighted average of the copies that agree there, so each value lies
//!   between the lowest and highest the copies hold.
//! - A line no copy received stays missing, drawn as a grey hatched band.
//! - The only corrections are undoing decoding errors: lining copies up
//!   (a shift) and putting swapped Robot 36 colours back.
//!
//! How:
//!
//! 1. [`group`]: copies are matched on what differs from the frame all the
//!    pictures share (the logos and callsigns), at a quarter of the size
//!    so speckle averages out.
//! 2. [`combine`]: each copy is lined up with the best one, swapped colours
//!    are found by a vote, and each line is weighted by how noisy that
//!    copy's line is (measured against the other copies where there are
//!    three or more, else by its own roughness). At each pixel the most
//!    central copy is found, copies far from it are left out, and the rest
//!    are averaged by weight.

// Pixel loops index several planes and per-row tables at once.
#![allow(clippy::needless_range_loop)]

use crate::{rgb_to_ycc, ycc_to_rgb, Image};

/// A row whose pixels are all at most this (each of R, G and B) was never
/// received: the decoder leaves such rows black.
const MISSING_MAX: u8 = 2;
/// Grouping works on blocks of this many pixels square.
const BLOCK: usize = 4;
/// Copies are grouped when their average likeness is above this (1 is
/// identical, 0 unrelated; noise lowers it). High, so two similar pictures
/// are never joined; noisy copies left out join afterwards (below).
const GROUP_MIN: f64 = 0.6;
/// A copy left on its own joins the group whose average it is most like,
/// if at least ATTACH_MIN and by at least ATTACH_MARGIN more than any
/// other group's.
const ATTACH_MIN: f64 = 0.35;
const ATTACH_MARGIN: f64 = 0.15;
/// A copy's block is part of a shared frame when it is within FRAME_AGREE
/// levels of the same block in more than this share of all the copies.
const FRAME_SHARE: f64 = 0.3;
const FRAME_AGREE: f32 = 12.0;
/// A frame is looked for only among at least this many copies, and is
/// believed only if it covers at most this share of a copy.
const FRAME_MIN_COPIES: usize = 8;
const FRAME_MOST: f64 = 0.6;
/// The least share of the picture's height two copies must both have to
/// be compared at all.
const GROUP_OVERLAP: f64 = 1.0 / 3.0;
/// Shifts searched when lining copies up, pixels: sideways, and the least
/// searched up and down (a fifth of the picture's height is searched where
/// that is more).
const ALIGN_DX: i32 = 10;
const ALIGN_DY: i32 = 6;
/// A match is clear when its cost is at most this share of the typical
/// cost over all the shifts searched (on real ISS copies, 0.2 to 0.6 for a
/// good copy, up to about 0.9 for a very noisy one).
const ALIGN_CLEAR: f64 = 0.8;
/// Lines two copies must share to be lined up with each other.
const ALIGN_ROWS: usize = 16;
/// Colour swaps are judged on pixels whose Cr and Cb differ by at least
/// this in the copy lined up with, and only with this many of them.
const COLOUR_STRONG: f32 = 24.0;
const COLOUR_PIXELS: usize = 500;
/// Noise floor in the line weights (levels, 0 to 255): a clean line's
/// weight is about 1 / NOISE_FLOOR².
const NOISE_FLOOR: f64 = 5.0;
/// A copy whose pixel is further than this from the most central copy's
/// (Y, Cr and Cb differences added) is left out at that pixel.
const CLIP: f32 = 60.0;
/// Leave one out is measured on every this-many rows and columns.
const LOO_STEP: usize = 4;
/// Noise counted for a line no copy received, when scoring a result.
const MISSING_NOISE: f64 = 100.0;

/// A picture as brightness and colour planes, with the rows received.
#[derive(Clone)]
struct Planes {
    w: usize,
    h: usize,
    /// Y, Cr, Cb, row by row.
    p: [Vec<f32>; 3],
    row_ok: Vec<bool>,
}

impl Planes {
    fn new(img: &Image) -> Self {
        let (w, h) = (img.width, img.height);
        let mut p = [vec![0f32; w * h], vec![0f32; w * h], vec![0f32; w * h]];
        let mut row_ok = vec![false; h];
        for y in 0..h {
            let row = img.row(y);
            row_ok[y] = row.iter().any(|&v| v > MISSING_MAX);
            for x in 0..w {
                let (yy, cr, cb) = rgb_to_ycc([row[x * 3], row[x * 3 + 1], row[x * 3 + 2]]);
                p[0][y * w + x] = yy as f32;
                p[1][y * w + x] = cr as f32;
                p[2][y * w + x] = cb as f32;
            }
        }
        Self { w, h, p, row_ok }
    }

    fn swap_colours(&mut self) {
        self.p.swap(1, 2);
    }

    /// How rough row `y` is: the mean size of the second difference along
    /// it, over the three planes. Noise makes it large; within one group
    /// the picture is the same, so it compares the copies' noise.
    fn roughness(&self, y: usize) -> f64 {
        let w = self.w;
        if w < 3 {
            return 0.0;
        }
        let mut sum = 0.0f64;
        for plane in &self.p {
            let r = &plane[y * w..(y + 1) * w];
            for x in 1..w - 1 {
                sum += (2.0 * r[x] - r[x - 1] - r[x + 1]).abs() as f64;
            }
        }
        sum / (3 * (w - 2)) as f64
    }
}

fn weight_for(noise: f64) -> f64 {
    1.0 / (noise * noise + NOISE_FLOOR * NOISE_FLOOR)
}

// ------------------------------------------------------------ grouping

/// Groups copies of the same picture: each group lists indices into
/// `images`, in order. Pictures of different sizes are never grouped. A
/// picture with no other copy is a group of one.
pub fn group(images: &[&Image]) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut sizes: Vec<(usize, usize)> = images.iter().map(|i| (i.width, i.height)).collect();
    sizes.sort();
    sizes.dedup();
    for size in sizes {
        let members: Vec<usize> = (0..images.len())
            .filter(|&i| (images[i].width, images[i].height) == size)
            .collect();
        for g in group_same_size(images, &members) {
            out.push(g);
        }
    }
    out.sort_by_key(|g| g[0]);
    out
}

/// Block brightness, with which blocks were received.
struct Blocks {
    bw: usize,
    bh: usize,
    v: Vec<f32>,
    ok: Vec<bool>,
}

fn blocks(img: &Image) -> Blocks {
    let pl = Planes::new(img);
    let (bw, bh) = (pl.w / BLOCK, pl.h / BLOCK);
    let mut v = vec![0f32; bw * bh];
    let mut ok = vec![false; bw * bh];
    for by in 0..bh {
        let rows_ok = (0..BLOCK).all(|d| pl.row_ok[by * BLOCK + d]);
        for bx in 0..bw {
            let mut s = 0f32;
            for dy in 0..BLOCK {
                for dx in 0..BLOCK {
                    s += pl.p[0][(by * BLOCK + dy) * pl.w + bx * BLOCK + dx];
                }
            }
            v[by * bw + bx] = s / (BLOCK * BLOCK) as f32;
            ok[by * bw + bx] = rows_ok;
        }
    }
    Blocks { bw, bh, v, ok }
}

fn group_same_size(images: &[&Image], members: &[usize]) -> Vec<Vec<usize>> {
    let n = members.len();
    // Too small to compare (the margins `likeness` skips need 7 blocks
    // across): each on its own.
    let (w, h) = (images[members[0]].width, images[members[0]].height);
    if n < 2 || w / BLOCK < 7 || h / BLOCK < 2 {
        return members.iter().map(|&i| vec![i]).collect();
    }
    let b: Vec<Blocks> = members.iter().map(|&i| blocks(images[i])).collect();
    let (bw, bh) = (b[0].bw, b[0].bh);
    // The frame a series of pictures shares (the logos and callsigns):
    // a copy's block is "common" when it matches the same block in a good
    // share of all the copies, more than any one picture's copies would.
    // Common blocks say nothing about which picture it is, so they are left
    // out of the comparison. Worked out per copy, so two events with
    // different frames in one folder are each handled.
    let mut resid: Vec<Vec<f32>> = b.iter().map(|x| x.v.clone()).collect();
    if n >= FRAME_MIN_COPIES {
        let mut col: Vec<f32> = Vec::with_capacity(n);
        for k in 0..bw * bh {
            col.clear();
            col.extend(b.iter().filter(|x| x.ok[k]).map(|x| x.v[k]));
            let need = (FRAME_SHARE * n as f64).ceil() as usize;
            if col.len() < need {
                continue;
            }
            for (i, x) in b.iter().enumerate() {
                if !x.ok[k] {
                    continue;
                }
                let agree = col
                    .iter()
                    .filter(|&&v| (v - x.v[k]).abs() <= FRAME_AGREE)
                    .count();
                if agree > need {
                    resid[i][k] = f32::NAN;
                }
            }
        }
        // A copy that is nearly all "common" is not a frame and a picture:
        // most copies are that picture. Then nothing of it is left out.
        for r in resid.iter_mut().zip(&b) {
            let ok = r.1.ok.iter().filter(|&&o| o).count();
            let common = r.0.iter().filter(|v| v.is_nan()).count();
            if ok > 0 && common as f64 > FRAME_MOST * ok as f64 {
                *r.0 = r.1.v.clone();
            }
        }
    }
    let need_rows = ((bh as f64 * GROUP_OVERLAP).ceil() as usize).max(2);
    let mut sim: Vec<Vec<Option<f64>>> = vec![vec![None; n]; n];
    for i in 0..n {
        for j in i + 1..n {
            let s = likeness(&b[i], &b[j], &resid[i], &resid[j], need_rows);
            sim[i][j] = s;
            sim[j][i] = s;
        }
    }
    // Average linkage: join the two groups most alike on average, while
    // that is above the limits. Pairs that share too little to compare
    // (one has the top of the picture, the other the bottom) are left out
    // of the average; groups with no pair compared are not joined.
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    loop {
        let mut best: Option<(f64, usize, usize)> = None;
        for x in 0..groups.len() {
            for y in x + 1..groups.len() {
                let (mut s, mut m) = (0.0, 0usize);
                for &i in &groups[x] {
                    for &j in &groups[y] {
                        if let Some(v) = sim[i][j] {
                            s += v;
                            m += 1;
                        }
                    }
                }
                if m == 0 {
                    continue;
                }
                let l = s / m as f64;
                if l > GROUP_MIN && best.is_none_or(|b| l > b.0) {
                    best = Some((l, x, y));
                }
            }
        }
        let Some((_, x, y)) = best else { break };
        let moved = groups.remove(y);
        groups[x].extend(moved);
    }
    // A noisy copy matches each of its picture's copies poorly, but their
    // average better: copies left on their own are compared with each
    // group's average, and join only a clear winner.
    let firm: Vec<usize> = (0..groups.len()).filter(|&g| groups[g].len() > 1).collect();
    if !firm.is_empty() {
        let avg: Vec<(Blocks, Vec<f32>)> = firm
            .iter()
            .map(|&g| average(&b, &resid, &groups[g]))
            .collect();
        let mut moves: Vec<(usize, usize)> = Vec::new();
        for (gi, g) in groups.iter().enumerate() {
            if g.len() != 1 {
                continue;
            }
            let i = g[0];
            let mut scores: Vec<(f64, usize)> = firm
                .iter()
                .zip(&avg)
                .filter_map(|(&f, (ab, ar))| {
                    likeness(&b[i], ab, &resid[i], ar, need_rows).map(|l| (l, f))
                })
                .collect();
            scores.sort_by(|a, b| b.0.total_cmp(&a.0));
            let Some(&(top, to)) = scores.first() else {
                continue;
            };
            let second = scores.get(1).map_or(0.0, |s| s.0.max(0.0));
            if top > ATTACH_MIN && top - second > ATTACH_MARGIN {
                moves.push((gi, to));
            }
        }
        for &(from, to) in &moves {
            let i = groups[from][0];
            groups[to].push(i);
        }
        let gone: Vec<usize> = moves.iter().map(|m| m.0).collect();
        groups = groups
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !gone.contains(i))
            .map(|(_, g)| g)
            .collect();
    }
    groups
        .into_iter()
        .map(|g| {
            let mut g: Vec<usize> = g.into_iter().map(|k| members[k]).collect();
            g.sort();
            g
        })
        .collect()
}

/// A group's average blocks: each block the mean of the copies that
/// have it, received where any copy has it; outside any shared frame in
/// the second part (NaN where every copy's block is frame).
fn average(b: &[Blocks], resid: &[Vec<f32>], g: &[usize]) -> (Blocks, Vec<f32>) {
    let (bw, bh) = (b[0].bw, b[0].bh);
    let mut v = vec![0f32; bw * bh];
    let mut ok = vec![false; bw * bh];
    let mut r = vec![f32::NAN; bw * bh];
    for k in 0..bw * bh {
        let (mut s, mut n, mut rs, mut rn) = (0f32, 0usize, 0f32, 0usize);
        for &i in g {
            if b[i].ok[k] {
                s += b[i].v[k];
                n += 1;
                if !resid[i][k].is_nan() {
                    rs += resid[i][k];
                    rn += 1;
                }
            }
        }
        if n > 0 {
            v[k] = s / n as f32;
            ok[k] = true;
        }
        if rn > 0 {
            r[k] = rs / rn as f32;
        }
    }
    (Blocks { bw, bh, v, ok }, r)
}

/// How alike two pictures are, from -1 to 1: the correlation of the
/// blocks outside the shared frame (NaN in `ra` and `rb`), at the best
/// small shift, over the blocks both received. None when they share too
/// few rows to say.
fn likeness(a: &Blocks, b: &Blocks, ra: &[f32], rb: &[f32], need_rows: usize) -> Option<f64> {
    let (bw, bh) = (a.bw, a.bh);
    let mut best: Option<f64> = None;
    for dy in -1i32..=1 {
        for dx in -2i32..=2 {
            let (mut sx, mut sy, mut sxx, mut syy, mut sxy, mut n) =
                (0f64, 0f64, 0f64, 0f64, 0f64, 0usize);
            let mut rows = 0;
            for y in 0..bh as i32 {
                let yb = y - dy;
                if yb < 0 || yb >= bh as i32 {
                    continue;
                }
                let (ya, yb) = (y as usize, yb as usize);
                if !(a.ok[ya * bw] && b.ok[yb * bw]) {
                    continue;
                }
                rows += 1;
                for x in 3..bw as i32 - 3 {
                    let xb = x - dx;
                    let (p, q) = (
                        ra[ya * bw + x as usize] as f64,
                        rb[yb * bw + xb as usize] as f64,
                    );
                    if p.is_nan() || q.is_nan() {
                        continue;
                    }
                    sx += p;
                    sy += q;
                    sxx += p * p;
                    syy += q * q;
                    sxy += p * q;
                    n += 1;
                }
            }
            if rows < need_rows || n < 2 {
                continue;
            }
            let nf = n as f64;
            let cov = sxy - sx * sy / nf;
            let va = sxx - sx * sx / nf;
            let vb = syy - sy * sy / nf;
            if va <= 0.0 || vb <= 0.0 {
                continue;
            }
            let c = cov / (va * vb).sqrt();
            best = Some(best.map_or(c, |b| b.max(c)));
        }
    }
    best
}

// ------------------------------------------------------------ combining

/// One output row's sources: (copy, share of the row's weight).
pub type RowSources = Vec<(usize, f32)>;

/// The combined picture and how it was made. Copies are numbered as in
/// the slice given to [`combine`].
#[derive(Debug, Clone)]
pub struct Combined {
    pub image: Image,
    /// The copy received best: the most weight over all its lines.
    pub strongest: usize,
    /// The copies that gave at least part of a line.
    pub used: Vec<usize>,
    /// Each copy's shift to line it up, (right, down) in pixels.
    pub shifts: Vec<(i32, i32)>,
    /// Copies too far out of place to line up, so left out: (0, 0) in
    /// `shifts`, and no lines from them.
    pub left_out: Vec<usize>,
    /// Copies whose colours were swapped and put back.
    pub swapped: Vec<usize>,
    /// The share of lines received by at least one copy, 0 to 1.
    pub coverage: f64,
    /// Lines no copy received.
    pub missing_rows: Vec<usize>,
    /// The result's estimated noise, in levels (0 to 255): lower is
    /// better. Lines no copy received count as very noisy.
    pub noise: f64,
    /// Each copy's estimated noise, the same way: to compare the result
    /// with the best single copy.
    pub copy_noise: Vec<f64>,
    /// Leave one out: for each copy (with three or more), the mean
    /// difference between it and the combination of the others, in levels.
    pub leave_one_out: Vec<Option<f64>>,
    /// For each line, which copies it came from.
    pub rows: Vec<RowSources>,
}

/// A copy lined up: planes, its shift, and each source row's weight.
struct Placed {
    pl: Planes,
    dx: i32,
    dy: i32,
    w: Vec<f64>,
}

impl Placed {
    /// The copy's pixel at output (x, y), with its weight, if it has one.
    fn at(&self, x: usize, y: usize) -> Option<([f32; 3], f64)> {
        let sx = x as i32 - self.dx;
        let sy = y as i32 - self.dy;
        if sx < 0 || sy < 0 || sx >= self.pl.w as i32 || sy >= self.pl.h as i32 {
            return None;
        }
        let (sx, sy) = (sx as usize, sy as usize);
        let wt = self.w[sy];
        if wt <= 0.0 {
            return None;
        }
        let k = sy * self.pl.w + sx;
        Some(([self.pl.p[0][k], self.pl.p[1][k], self.pl.p[2][k]], wt))
    }
}

/// How to combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Look for copies with their colours swapped and put them back. Only
    /// Robot 36 (and other modes sending the two colour differences on
    /// alternate lines) can swap like this; see
    /// [`crate::colour_fix_suits`].
    pub colour_fix: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { colour_fix: true }
    }
}

/// Combines copies of one picture (as [`group`] found them; all the same
/// size). `tie_break` (higher is better: for a satellite, its elevation in
/// degrees, if known) settles a tie for the strongest copy.
pub fn combine(copies: &[&Image], tie_break: &[Option<f64>]) -> Combined {
    combine_with(copies, tie_break, Options::default())
}

/// [`combine`], with [`Options`].
pub fn combine_with(copies: &[&Image], elevations: &[Option<f64>], opts: Options) -> Combined {
    assert!(!copies.is_empty(), "nothing to combine");
    let (w, h) = (copies[0].width, copies[0].height);
    let planes: Vec<Planes> = copies.iter().map(|c| Planes::new(c)).collect();
    let k = copies.len();

    // 1. Line weights from each copy's own roughness.
    let rough: Vec<Vec<f64>> = planes
        .iter()
        .map(|pl| {
            (0..h)
                .map(|y| {
                    if pl.row_ok[y] {
                        pl.roughness(y)
                    } else {
                        f64::NAN
                    }
                })
                .collect()
        })
        .collect();
    let rough_w: Vec<Vec<f64>> = rough
        .iter()
        .map(|r| {
            r.iter()
                .map(|&n| if n.is_nan() { 0.0 } else { weight_for(n) })
                .collect()
        })
        .collect();
    let total = |w: &[f64]| w.iter().sum::<f64>();
    let pick = |ws: &[Vec<f64>]| -> usize {
        (0..k)
            .max_by(|&a, &b| {
                total(&ws[a])
                    .total_cmp(&total(&ws[b]))
                    .then(
                        elevations
                            .get(a)
                            .copied()
                            .flatten()
                            .unwrap_or(-90.0)
                            .total_cmp(&elevations.get(b).copied().flatten().unwrap_or(-90.0)),
                    )
                    .then(b.cmp(&a))
            })
            .unwrap_or(0)
    };
    let reference = pick(&rough_w);

    // 2. Line everything up with the reference, best copies first, each
    //    against what is lined up so far.
    let mut order: Vec<usize> = (0..k).filter(|&i| i != reference).collect();
    order.sort_by(|&a, &b| total(&rough_w[b]).total_cmp(&total(&rough_w[a])));
    let mut shifts = vec![(0i32, 0i32); k];
    let mut mosaic: Vec<Option<f32>> = vec![None; w * h];
    let paint = |mosaic: &mut Vec<Option<f32>>, pl: &Planes, dx: i32, dy: i32| {
        for sy in 0..h {
            if !pl.row_ok[sy] {
                continue;
            }
            let y = sy as i32 + dy;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for sx in 0..w {
                let x = sx as i32 + dx;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let m = &mut mosaic[y as usize * w + x as usize];
                if m.is_none() {
                    *m = Some(pl.p[0][sy * w + sx]);
                }
            }
        }
    };
    paint(&mut mosaic, &planes[reference], 0, 0);
    let mut left_out = Vec::new();
    for &i in &order {
        match align(&mosaic, &planes[i], w, h) {
            Some((dx, dy)) => {
                shifts[i] = (dx, dy);
                paint(&mut mosaic, &planes[i], dx, dy);
            }
            // Too far out to line up: left out, so it gives no lines.
            None => left_out.push(i),
        }
    }
    left_out.sort();

    // 3. Swapped colours: each copy against the reference, then a vote on
    //    which way is right. With two copies there is no majority, so the
    //    reference's colours are kept.
    let mut planes = planes;
    let rel: Vec<Option<bool>> = (0..k)
        .map(|i| {
            if left_out.contains(&i) {
                None
            } else if i == reference || !opts.colour_fix {
                Some(false)
            } else {
                swapped_against(&planes[reference], &planes[i], shifts[i])
            }
        })
        .collect();
    let judged = rel.iter().filter(|r| r.is_some()).count();
    let against = rel.iter().filter(|r| **r == Some(true)).count();
    let reference_wrong = judged >= 3 && against * 2 > judged;
    let mut swapped = Vec::new();
    for i in 0..k {
        let flip = match rel[i] {
            Some(r) => r != reference_wrong,
            None => false,
        };
        if flip {
            planes[i].swap_colours();
            swapped.push(i);
        }
    }

    // 4. Line weights: against the other copies where there are three or
    //    more of them at that line, else from roughness.
    let mut placed: Vec<Placed> = planes
        .into_iter()
        .enumerate()
        .map(|(i, pl)| Placed {
            dx: shifts[i].0,
            dy: shifts[i].1,
            w: if left_out.contains(&i) {
                vec![0.0; h]
            } else {
                rough_w[i].clone()
            },
            pl,
        })
        .collect();
    let first = merge(&placed, w, h, None).0;
    let mut noise: Vec<Vec<f64>> = rough.clone();
    for (i, c) in placed.iter().enumerate() {
        for sy in 0..h {
            if !c.pl.row_ok[sy] {
                continue;
            }
            let y = sy as i32 + c.dy;
            if y < 0 || y >= h as i32 {
                continue;
            }
            let y = y as usize;
            // How many copies have this line.
            let others = placed
                .iter()
                .enumerate()
                .filter(|(j, _)| !left_out.contains(j))
                .filter(|(_, o)| {
                    let oy = y as i32 - o.dy;
                    oy >= 0 && (oy as usize) < h && o.pl.row_ok[oy as usize]
                })
                .count();
            if others < 3 {
                continue;
            }
            let mut s = 0.0f64;
            let mut n = 0usize;
            for sx in 0..w {
                let x = sx as i32 + c.dx;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                if let Some(v) = first[y * w + x as usize] {
                    let kk = sy * w + sx;
                    s += (0..3)
                        .map(|p| (c.pl.p[p][kk] - v[p]).abs() as f64)
                        .sum::<f64>()
                        / 3.0;
                    n += 1;
                }
            }
            if n > 0 {
                noise[i][sy] = s / n as f64;
            }
        }
    }
    for (i, c) in placed.iter_mut().enumerate() {
        for sy in 0..h {
            c.w[sy] = if c.pl.row_ok[sy] && !left_out.contains(&i) {
                weight_for(noise[i][sy])
            } else {
                0.0
            };
        }
    }
    let final_w: Vec<Vec<f64>> = placed.iter().map(|c| c.w.clone()).collect();
    let strongest = pick(&final_w);

    // 5. Combine.
    let (out, row_weight, rows) = merge(&placed, w, h, None);
    let mut image = Image::new(w, h);
    let mut missing_rows = Vec::new();
    for y in 0..h {
        let any = (0..w).any(|x| out[y * w + x].is_some());
        if !any {
            missing_rows.push(y);
        }
        for x in 0..w {
            let i = (y * w + x) * 3;
            let px = match out[y * w + x] {
                Some(v) => ycc_to_rgb(v[0] as f64, v[1] as f64, v[2] as f64),
                // Not received: a grey hatch, plainly not picture.
                None => {
                    if (x + y) % 8 < 4 {
                        [96, 96, 96]
                    } else {
                        [128, 128, 128]
                    }
                }
            };
            image.rgb[i..i + 3].copy_from_slice(&px);
        }
    }
    let coverage = 1.0 - missing_rows.len() as f64 / h as f64;
    let score = |weights: &[f64]| -> f64 {
        weights
            .iter()
            .map(|&wt| {
                if wt > 0.0 {
                    (1.0 / wt).sqrt()
                } else {
                    MISSING_NOISE
                }
            })
            .sum::<f64>()
            / h as f64
    };
    let noise_out = score(&row_weight);
    let copy_noise: Vec<f64> = (0..k)
        .map(|i| {
            let p = &placed[i];
            let per_row: Vec<f64> = (0..h)
                .map(|y| {
                    let sy = y as i32 - p.dy;
                    if sy >= 0 && (sy as usize) < h {
                        p.w[sy as usize]
                    } else {
                        0.0
                    }
                })
                .collect();
            score(&per_row)
        })
        .collect();

    // 6. Leave one out.
    let leave_one_out: Vec<Option<f64>> = (0..k)
        .map(|i| {
            if k < 3 {
                return None;
            }
            // On every LOO_STEP-th row and column: a fair sample, and with
            // many copies the full picture each time would be slow.
            let (others, _, _) = merge_sampled(&placed, w, h, Some(i), LOO_STEP);
            let c = &placed[i];
            let (mut s, mut n) = (0.0f64, 0usize);
            for y in (0..h).step_by(LOO_STEP) {
                for x in (0..w).step_by(LOO_STEP) {
                    if let (Some((v, _)), Some(o)) = (c.at(x, y), others[y * w + x]) {
                        s += (0..3).map(|p| (v[p] - o[p]).abs() as f64).sum::<f64>() / 3.0;
                        n += 1;
                    }
                }
            }
            (n > 0).then(|| s / n as f64)
        })
        .collect();

    let mut used: Vec<usize> = rows.iter().flatten().map(|&(c, _)| c).collect();
    used.sort();
    used.dedup();
    Combined {
        image,
        strongest,
        used,
        shifts,
        left_out,
        swapped,
        coverage,
        missing_rows,
        noise: noise_out,
        copy_noise,
        leave_one_out,
        rows,
    }
}

/// The shift (right, down) that best lines `pl` up with the mosaic: the
/// least mean brightness difference over what both have. (0, 0) when they
/// share too little to say. None when the copy can't be lined up: its best
/// match is at the edge of the search (it is further out than that), or is
/// unclear and would move it more than a little.
///
/// Up and down the search reaches a fifth of the picture: a decoder that
/// starts a picture from its line sync pulses (having missed the header)
/// can guess wrongly which line it joined at, by tens of lines. To keep
/// that fast, a coarse search (every other sideways shift, a sample of the
/// pixels) is followed by a fine one around its best.
fn align(mosaic: &[Option<f32>], pl: &Planes, w: usize, h: usize) -> Option<(i32, i32)> {
    let max_dy = ((h / 5) as i32).max(ALIGN_DY);
    let mut per_row: Vec<f64> = Vec::with_capacity(h);
    // The cost of one shift, on every `step`-th pixel of each line.
    let mut cost = |dx: i32, dy: i32, step: usize| -> Option<f64> {
        per_row.clear();
        for sy in 0..h {
            if !pl.row_ok[sy] {
                continue;
            }
            let y = sy as i32 + dy;
            if y < 0 || y >= h as i32 {
                continue;
            }
            let (mut s, mut n) = (0.0f64, 0usize);
            for sx in (0..w).step_by(step) {
                let x = sx as i32 + dx;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                if let Some(m) = mosaic[y as usize * w + x as usize] {
                    s += (m - pl.p[0][sy * w + sx]).abs() as f64;
                    n += 1;
                }
            }
            if n > w / (2 * step) {
                per_row.push(s / n as f64);
            }
        }
        if per_row.len() < ALIGN_ROWS {
            return None;
        }
        // The better half of the rows: noisy lines in either picture
        // say nothing about the shift.
        per_row.sort_by(|a, b| a.total_cmp(b));
        let half = &per_row[..per_row.len() / 2];
        let diff = half.iter().sum::<f64>() / half.len() as f64;
        // A small preference for no shift, so a flat picture stays put.
        Some(diff + 0.05 * (dx.abs() + dy.abs()) as f64)
    };
    let mut coarse: Option<(f64, i32, i32)> = None;
    let mut all: Vec<f64> = Vec::new();
    for dy in -max_dy..=max_dy {
        for dx in (-ALIGN_DX..=ALIGN_DX).step_by(2) {
            if let Some(c) = cost(dx, dy, 4) {
                all.push(c);
                if coarse.is_none_or(|b| c < b.0) {
                    coarse = Some((c, dx, dy));
                }
            }
        }
    }
    let Some((_, cx, cy)) = coarse else {
        return Some((0, 0));
    };
    let mut best: Option<(f64, i32, i32)> = None;
    for dy in (cy - 2).max(-max_dy)..=(cy + 2).min(max_dy) {
        for dx in (cx - 2).max(-ALIGN_DX)..=(cx + 2).min(ALIGN_DX) {
            if let Some(c) = cost(dx, dy, 2) {
                if best.is_none_or(|b| c < b.0) {
                    best = Some((c, dx, dy));
                }
            }
        }
    }
    let (_, dx, dy) = best?;
    if dy.abs() == max_dy || dx.abs() == ALIGN_DX {
        return None;
    }
    // How clear the match is: its cost against the typical (median) cost
    // over all the shifts searched. A copy that matches nowhere clearly is
    // trusted only where it needs no real move, as a noisy copy in its
    // right place does; moved further on an unclear match it may be
    // anywhere, so it is left out.
    all.sort_by(|a, b| a.total_cmp(b));
    let typical = all[all.len() / 2];
    let clear = typical > 0.0 && coarse.map_or(0.0, |c| c.0) / typical <= ALIGN_CLEAR;
    if !clear && (dx.abs() > 2 || dy.abs() > 1) {
        return None;
    }
    Some((dx, dy))
}

/// Whether `c` (shifted by `shift`) has its colours swapped against `r`:
/// Some(true) if swapping them makes the colour differences clearly
/// smaller, Some(false) if clearly larger, None if it can't tell (too
/// little overlap, or too little strong colour).
fn swapped_against(r: &Planes, c: &Planes, shift: (i32, i32)) -> Option<bool> {
    let (w, h) = (r.w, r.h);
    let (mut same, mut cross, mut n) = (0.0f64, 0.0f64, 0usize);
    for sy in 0..h {
        if !c.row_ok[sy] {
            continue;
        }
        let y = sy as i32 + shift.1;
        if y < 0 || y >= h as i32 || !r.row_ok[y as usize] {
            continue;
        }
        let y = y as usize;
        for sx in 0..w {
            let x = sx as i32 + shift.0;
            if x < 0 || x >= w as i32 {
                continue;
            }
            let (a, b) = (y * w + x as usize, sy * w + sx);
            let (rcr, rcb, ccr, ccb) = (r.p[1][a], r.p[2][a], c.p[1][b], c.p[2][b]);
            // Only where the colour is strong: on greys a swap changes
            // nothing, and noise there would hide it.
            if (rcr - rcb).abs() < COLOUR_STRONG {
                continue;
            }
            same += ((rcr - ccr).abs() + (rcb - ccb).abs()) as f64;
            cross += ((rcr - ccb).abs() + (rcb - ccr).abs()) as f64;
            n += 1;
        }
    }
    if n < COLOUR_PIXELS {
        return None;
    }
    if cross < 0.7 * same {
        Some(true)
    } else if same < 0.7 * cross {
        Some(false)
    } else {
        None
    }
}

/// Combines lined-up copies, leaving out copy `skip` if given. Returns
/// the picture (None where no copy has the pixel), each output row's total
/// weight (the most at any pixel of the row), and each row's sources.
#[allow(clippy::type_complexity)]
fn merge(
    placed: &[Placed],
    w: usize,
    h: usize,
    skip: Option<usize>,
) -> (Vec<Option<[f32; 3]>>, Vec<f64>, Vec<RowSources>) {
    merge_sampled(placed, w, h, skip, 1)
}

/// As [`merge`], working out only every `step`-th row and column (the
/// rest stay None, their rows empty).
#[allow(clippy::type_complexity)]
fn merge_sampled(
    placed: &[Placed],
    w: usize,
    h: usize,
    skip: Option<usize>,
    step: usize,
) -> (Vec<Option<[f32; 3]>>, Vec<f64>, Vec<RowSources>) {
    let mut out = vec![None; w * h];
    let mut row_weight = vec![0.0f64; h];
    let mut rows = Vec::with_capacity(h);
    let mut samples: Vec<(usize, [f32; 3], f64)> = Vec::with_capacity(placed.len());
    for y in 0..h {
        let mut share = vec![0.0f64; placed.len()];
        let mut best_row = 0.0f64;
        if y % step != 0 {
            rows.push(Vec::new());
            continue;
        }
        for x in (0..w).step_by(step) {
            samples.clear();
            for (i, c) in placed.iter().enumerate() {
                if Some(i) == skip {
                    continue;
                }
                if let Some((v, wt)) = c.at(x, y) {
                    samples.push((i, v, wt));
                }
            }
            if samples.is_empty() {
                continue;
            }
            // The most central copy: least weighted distance to the rest.
            let dist = |a: &[f32; 3], b: &[f32; 3]| {
                (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
            };
            let centre = samples
                .iter()
                .map(|s| {
                    let d: f64 = samples.iter().map(|t| t.2 * dist(&s.1, &t.1) as f64).sum();
                    (d, s.1)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, v)| v)
                .unwrap_or(samples[0].1);
            let mut acc = [0.0f64; 3];
            let mut tw = 0.0f64;
            for s in samples.iter().filter(|s| dist(&s.1, &centre) <= CLIP) {
                for p in 0..3 {
                    acc[p] += s.2 * s.1[p] as f64;
                }
                tw += s.2;
                share[s.0] += s.2;
            }
            if tw > 0.0 {
                out[y * w + x] = Some(acc.map(|v| (v / tw) as f32));
                best_row = best_row.max(tw);
            }
        }
        row_weight[y] = best_row;
        let sum: f64 = share.iter().sum();
        let mut src: RowSources = share
            .iter()
            .enumerate()
            .filter(|(_, &s)| s > 0.0)
            .map(|(i, &s)| (i, (s / sum) as f32))
            .collect();
        src.sort_by(|a, b| b.1.total_cmp(&a.1));
        rows.push(src);
    }
    (out, row_weight, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test picture with detail everywhere and colour, different for each
    /// seed and not repeating, so shifts and swaps are measurable: blocks
    /// of random colour (8 by 6 pixels) over a gentle gradient.
    fn scene(w: usize, h: usize, seed: u32) -> Image {
        let mut img = Image::new(w, h);
        // splitmix64, so neighbouring cells and seeds are unrelated.
        let cell = |cx: usize, cy: usize, k: u64| -> f64 {
            let mut z = (seed as u64) << 40 ^ (cx as u64) << 20 ^ (cy as u64) << 4 ^ k;
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        for y in 0..h {
            for x in 0..w {
                let (cx, cy) = (x / 8, y / 6);
                let r = 128.0 + 80.0 * cell(cx, cy, 1);
                let g = 128.0 + 80.0 * cell(cx, cy, 2);
                let b = 128.0 + 80.0 * cell(cx, cy, 3);
                let i = (y * w + x) * 3;
                img.rgb[i] = r.clamp(0.0, 255.0) as u8;
                img.rgb[i + 1] = g.clamp(0.0, 255.0) as u8;
                img.rgb[i + 2] = b.clamp(0.0, 255.0) as u8;
            }
        }
        img
    }

    /// The shared frame: the same border on every picture, as on the ISS
    /// slides.
    fn framed(mut img: Image) -> Image {
        let (w, h) = (img.width, img.height);
        let mut r = Lcg(99);
        let pattern: Vec<u8> = (0..(w / 5 + 1) * (h / 5 + 1))
            .map(|_| (128.0 + 100.0 * r.next()) as u8)
            .collect();
        for y in 0..h {
            for x in 0..w {
                if y < 12 || y >= h - 40 || x < 8 || x >= w - 60 {
                    let i = (y * w + x) * 3;
                    let v = pattern[(y / 5) * (w / 5 + 1) + x / 5];
                    img.rgb[i..i + 3].copy_from_slice(&[v, v / 2, 255 - v]);
                }
            }
        }
        img
    }

    /// Cheap repeatable noise.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
        }
    }

    /// A damaged copy: noise on some rows (stronger on `bad` rows), rows
    /// lost, a shift and maybe swapped colours.
    fn damaged(
        src: &Image,
        seed: u64,
        noise: f64,
        bad: std::ops::Range<usize>,
        lost: std::ops::Range<usize>,
        shift: (i32, i32),
        swap: bool,
    ) -> Image {
        let (w, h) = (src.width, src.height);
        let mut out = Image::new(w, h);
        let mut rng = Lcg(seed);
        for y in 0..h {
            for x in 0..w {
                let sx = x as i32 - shift.0;
                let sy = y as i32 - shift.1;
                let p = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    src.pixel(sx as usize, sy as usize)
                } else {
                    [20, 20, 20]
                };
                let (yy, mut cr, mut cb) = rgb_to_ycc(p);
                if swap {
                    std::mem::swap(&mut cr, &mut cb);
                }
                let n = if bad.contains(&y) { noise * 6.0 } else { noise };
                let q = ycc_to_rgb(
                    yy + n * rng.next(),
                    cr + n * rng.next(),
                    cb + n * rng.next(),
                );
                let i = (y * w + x) * 3;
                out.rgb[i..i + 3].copy_from_slice(&q);
            }
        }
        for y in lost {
            let w3 = w * 3;
            out.rgb[y * w3..(y + 1) * w3].fill(0);
        }
        out
    }

    /// Mean difference from the original over `rows`, levels, with `b`
    /// shifted by `off` against `a` (b at (x, y) is a at (x - dx, y - dy)).
    fn error(a: &Image, b: &Image, rows: &[usize], off: (i32, i32)) -> f64 {
        let w = a.width;
        let mut s = 0.0;
        let mut n = 0;
        for &y in rows {
            for x in 12..w - 12 {
                let (ax, ay) = ((x as i32 - off.0) as usize, (y as i32 - off.1) as usize);
                if ay >= a.height {
                    continue;
                }
                let (p, q) = (a.pixel(ax, ay), b.pixel(x, y));
                s += (0..3)
                    .map(|c| (p[c] as f64 - q[c] as f64).abs())
                    .sum::<f64>()
                    / 3.0;
                n += 1;
            }
        }
        s / n as f64
    }

    #[test]
    fn copies_combine_better_than_any_one() {
        let (w, h) = (320, 240);
        let truth = framed(scene(w, h, 1));
        let copies = [
            damaged(&truth, 1, 18.0, 0..60, 200..240, (0, 0), false),
            damaged(&truth, 2, 18.0, 60..120, 0..30, (3, 0), false),
            damaged(&truth, 3, 18.0, 120..180, 0..0, (-2, 2), true),
            damaged(&truth, 4, 18.0, 180..240, 100..140, (1, -2), false),
        ];
        let refs: Vec<&Image> = copies.iter().collect();
        let c = combine(&refs, &[None; 4]);
        assert!(c.missing_rows.is_empty(), "every line came in somewhere");
        assert_eq!(c.coverage, 1.0);
        assert_eq!(c.swapped, vec![2], "the swapped copy is found");
        // Shifts are relative to whichever copy the others were lined up
        // with: each pair's difference undoes the damage's.
        let made = [(0, 0), (3, 0), (-2, 2), (1, -2)];
        for i in 0..4 {
            for j in 0..4 {
                let got = (c.shifts[i].0 - c.shifts[j].0, c.shifts[i].1 - c.shifts[j].1);
                assert_eq!(
                    got,
                    (made[j].0 - made[i].0, made[j].1 - made[i].1),
                    "copies {i} and {j}"
                );
            }
        }
        // Away from the edges the shifts leave empty.
        let rows: Vec<usize> = (12..h - 12).collect();
        // The result is drawn where the copy lined up with sits.
        let combined = error(&truth, &c.image, &rows, c.shifts[0]);
        for (i, copy) in copies.iter().enumerate() {
            let own: Vec<usize> = rows
                .iter()
                .copied()
                .filter(|&y| copy.row(y).iter().any(|&v| v > MISSING_MAX))
                .collect();
            let single = if i == 0 {
                error(&truth, copy, &own, (0, 0))
            } else {
                f64::INFINITY
            };
            if i == 0 {
                assert!(
                    combined < single * 0.6,
                    "combined {combined:.1} against copy 0's {single:.1}"
                );
            }
        }
        assert!(combined < 10.0, "combined error {combined:.1}");
        assert!(c.noise < c.copy_noise.iter().cloned().fold(f64::INFINITY, f64::min));
        assert!(c.leave_one_out.iter().all(|v| v.is_some()));
    }

    #[test]
    fn a_copy_decoded_many_lines_out_is_lined_up() {
        // As when a decoder missed the header, started from the line sync
        // pulses and misjudged which line it had joined at.
        let (w, h) = (320, 240);
        let truth = framed(scene(w, h, 5));
        let copies = [
            damaged(&truth, 1, 14.0, 0..40, 0..0, (0, 0), false),
            damaged(&truth, 2, 14.0, 100..140, 0..0, (1, 0), false),
            damaged(&truth, 3, 14.0, 180..220, 0..0, (0, 0), false),
            damaged(&truth, 4, 20.0, 0..0, 0..15, (0, 15), false),
        ];
        let refs: Vec<&Image> = copies.iter().collect();
        let c = combine(&refs, &[None; 4]);
        assert!(c.left_out.is_empty(), "left out {:?}", c.left_out);
        let made = [(0, 0), (1, 0), (0, 0), (0, 15)];
        for i in 0..4 {
            let got = (c.shifts[i].0 - c.shifts[0].0, c.shifts[i].1 - c.shifts[0].1);
            assert_eq!(
                got,
                (made[0].0 - made[i].0, made[0].1 - made[i].1),
                "copy {i}"
            );
        }
        assert!(c.used.contains(&3));
        let rows: Vec<usize> = (20..h - 20).collect();
        let combined = error(&truth, &c.image, &rows, c.shifts[0]);
        assert!(combined < 10.0, "combined error {combined:.1}");
    }

    #[test]
    fn a_copy_too_far_out_is_left_out() {
        let (w, h) = (320, 240);
        let truth = framed(scene(w, h, 6));
        let copies = [
            damaged(&truth, 1, 14.0, 0..40, 0..0, (0, 0), false),
            damaged(&truth, 2, 14.0, 100..140, 0..0, (0, 0), false),
            damaged(&truth, 3, 14.0, 180..220, 0..0, (0, 0), false),
            damaged(&truth, 4, 20.0, 0..0, 0..70, (0, 70), false),
        ];
        let refs: Vec<&Image> = copies.iter().collect();
        let c = combine(&refs, &[None; 4]);
        assert_eq!(c.left_out, vec![3]);
        assert!(!c.used.contains(&3));
        assert_eq!(c.shifts[3], (0, 0));
        // The same picture as without it.
        let without = combine(&refs[..3], &[None; 3]);
        assert!(without.left_out.is_empty());
        assert_eq!(c.image.rgb, without.image.rgb);
    }

    #[test]
    fn a_line_nobody_received_stays_missing() {
        let truth = framed(scene(320, 240, 2));
        let a = damaged(&truth, 5, 10.0, 0..0, 50..70, (0, 0), false);
        let b = damaged(&truth, 6, 10.0, 0..0, 60..80, (0, 0), false);
        let c = combine(&[&a, &b], &[Some(30.0), Some(60.0)]);
        assert_eq!(c.missing_rows, (60..70).collect::<Vec<_>>());
        assert!((c.coverage - 230.0 / 240.0).abs() < 1e-9);
        // Drawn as the grey hatch, nothing taken from elsewhere.
        for x in 0..320 {
            let p = c.image.pixel(x, 65);
            assert!(p == [96, 96, 96] || p == [128, 128, 128]);
        }
        // Rows 50 to 59 come only from b, 70 to 79 only from a.
        assert_eq!(c.rows[55], vec![(1, 1.0)]);
        assert_eq!(c.rows[75], vec![(0, 1.0)]);
    }

    #[test]
    fn every_value_lies_within_the_copies() {
        let truth = framed(scene(160, 120, 3));
        let copies: Vec<Image> = (0..5)
            .map(|i| damaged(&truth, 10 + i, 25.0, 0..0, 0..0, (0, 0), false))
            .collect();
        let refs: Vec<&Image> = copies.iter().collect();
        let c = combine(&refs, &[None; 5]);
        assert!(c.shifts.iter().all(|&s| s == (0, 0)));
        // In Y, Cr and Cb (what is averaged), each result lies between the
        // least and most of the copies at that pixel (allowing for the
        // rounding to RGB and back).
        for y in 0..120 {
            for x in 0..160 {
                let v = rgb_to_ycc(c.image.pixel(x, y));
                let cs: Vec<(f64, f64, f64)> =
                    copies.iter().map(|i| rgb_to_ycc(i.pixel(x, y))).collect();
                for (p, get) in [
                    (
                        v.0,
                        (|t: &(f64, f64, f64)| t.0) as fn(&(f64, f64, f64)) -> f64,
                    ),
                    (v.1, |t| t.1),
                    (v.2, |t| t.2),
                ] {
                    let lo = cs.iter().map(get).fold(f64::INFINITY, f64::min);
                    let hi = cs.iter().map(get).fold(f64::NEG_INFINITY, f64::max);
                    assert!(
                        p >= lo - 3.0 && p <= hi + 3.0,
                        "({x}, {y}): {p} outside {lo}..{hi}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_single_copy_comes_back_as_it_was() {
        let truth = framed(scene(320, 240, 4));
        let a = damaged(&truth, 7, 8.0, 0..0, 200..240, (0, 0), false);
        let c = combine(&[&a], &[None]);
        assert_eq!(c.strongest, 0);
        assert_eq!(c.missing_rows.len(), 40);
        for y in 0..200 {
            for x in 0..320 {
                let (p, q) = (a.pixel(x, y), c.image.pixel(x, y));
                for k in 0..3 {
                    assert!((p[k] as i32 - q[k] as i32).abs() <= 2, "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn copies_are_grouped_by_picture_not_by_frame() {
        // A series of pictures in the same frame, as an ISS event sends.
        let scenes: Vec<Image> = [1, 5, 9, 13]
            .iter()
            .map(|&s| framed(scene(320, 240, s)))
            .collect();
        let copies = [
            damaged(&scenes[0], 1, 15.0, 0..40, 200..240, (0, 0), false),
            damaged(&scenes[1], 2, 15.0, 0..0, 0..0, (0, 0), false),
            damaged(&scenes[0], 3, 15.0, 0..0, 100..140, (2, 0), false),
            damaged(&scenes[2], 4, 15.0, 0..0, 0..100, (0, 0), false),
            damaged(&scenes[1], 5, 30.0, 0..0, 0..60, (-3, 1), false),
            damaged(&scenes[0], 6, 30.0, 0..0, 0..0, (0, 0), true),
            // Almost nothing received: compared with nothing.
            damaged(&scenes[0], 7, 15.0, 0..0, 0..225, (0, 0), false),
            damaged(&scenes[3], 8, 15.0, 0..0, 0..0, (0, 0), false),
            damaged(&scenes[3], 9, 20.0, 0..60, 180..240, (1, 0), false),
            damaged(&scenes[2], 10, 15.0, 0..0, 200..240, (0, 0), false),
        ];
        let refs: Vec<&Image> = copies.iter().collect();
        let g = group(&refs);
        assert_eq!(
            g,
            vec![vec![0, 2, 5], vec![1, 4], vec![3, 9], vec![6], vec![7, 8]]
        );
    }

    #[test]
    fn with_few_copies_the_frame_is_not_guessed() {
        // Three copies of one picture: nothing is taken as a frame.
        let a = framed(scene(320, 240, 2));
        let copies = [
            damaged(&a, 1, 15.0, 0..0, 0..80, (0, 0), false),
            damaged(&a, 2, 15.0, 0..0, 150..240, (1, 0), false),
            damaged(&a, 3, 25.0, 0..0, 60..120, (0, 1), false),
        ];
        let refs: Vec<&Image> = copies.iter().collect();
        assert_eq!(group(&refs), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn a_strongest_copy_is_the_best_received() {
        let truth = framed(scene(320, 240, 6));
        let poor = damaged(&truth, 1, 30.0, 0..120, 0..0, (0, 0), false);
        let good = damaged(&truth, 2, 6.0, 0..0, 0..0, (0, 0), false);
        let fair = damaged(&truth, 3, 15.0, 0..0, 0..0, (0, 0), false);
        let c = combine(
            &[&poor, &good, &fair],
            &[Some(80.0), Some(10.0), Some(40.0)],
        );
        assert_eq!(c.strongest, 1);
    }
}
