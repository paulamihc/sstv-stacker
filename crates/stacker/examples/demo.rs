//! Makes `docs/demo.png` for the README: four damaged copies of the
//! made-up test picture, the best of them, and what SSTV Stacker makes of
//! them all.
//!
//!     cargo run -p sstv-stacker --example demo

use sstv_stacker::formats::{self, Details};
use sstv_stacker::{combine, record, Image};

fn main() {
    let scene = formats::read(include_bytes!("../tests/fixtures/scene.png"))
        .unwrap()
        .image;
    let mut seed = 0x5eed_u64;
    let mut rnd = move || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    // Four passes: each starts late or stops early, and has noisy lines.
    let lost = [0..50, 190..240, 90..130, 0..0];
    let copies: Vec<Image> = lost
        .iter()
        .map(|l| {
            let mut c = scene.clone();
            let w = c.width;
            for y in 0..c.height {
                let row = &mut c.rgb[y * w * 3..(y + 1) * w * 3];
                if l.contains(&y) {
                    row.fill(0);
                } else if rnd() < 0.45 {
                    for v in row.iter_mut() {
                        if rnd() < 0.5 {
                            *v = (*v as f64 + (rnd() - 0.5) * 220.0).clamp(3.0, 255.0) as u8;
                        }
                    }
                }
            }
            c
        })
        .collect();
    let refs: Vec<&Image> = copies.iter().collect();
    let c = combine(&refs, &[]);
    let best = &copies[c.strongest];
    println!(
        "noise {:.1} (best single copy) to {:.1} (combined)",
        record::best_single(&c),
        c.noise
    );
    // Side by side, with a gap.
    let (w, h, gap) = (scene.width, scene.height, 12);
    let mut out = Image::new(w * 2 + gap, h);
    out.rgb.fill(255);
    for y in 0..h {
        for (k, img) in [best, &c.image].iter().enumerate() {
            let dst = (y * out.width + k * (w + gap)) * 3;
            out.rgb[dst..dst + w * 3].copy_from_slice(img.row(y));
        }
    }
    std::fs::write(
        "docs/demo.png",
        formats::write_png(&out, &Details::default()),
    )
    .unwrap();
}
