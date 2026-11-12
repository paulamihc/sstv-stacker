//! Reading PNG, JPEG and BMP pictures with their details, and writing the
//! result as PNG with the strongest copy's details.
//!
//! Details kept:
//! - PNG text fields: international, then plain, then compressed, each in
//!   file order (all written back as international text);
//! - an EXIF block, from a JPEG (its APP1 segment) or a PNG (its `eXIf`
//!   chunk), written into the result's `eXIf` chunk;
//! - a JPEG's first comment, as the text field "Comment".
//!
//! Other parts of a file (colour profiles, XMP) aren't carried over.

use crate::Image;

/// The kinds of file read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Bmp,
}

impl Format {
    /// From the file's first bytes.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"BM") {
            Some(Self::Bmp)
        } else {
            None
        }
    }

    /// Whether a file name looks like one of these.
    pub fn by_name(name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".bmp"]
            .iter()
            .any(|e| n.ends_with(e))
    }
}

/// A picture's details.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    /// Text fields (keyword, value), in file order.
    pub text: Vec<(String, String)>,
    /// An EXIF block (TIFF header onwards), if it had one.
    pub exif: Option<Vec<u8>>,
}

impl Details {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.text
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// A picture read from a file.
#[derive(Debug, Clone)]
pub struct Picture {
    pub image: Image,
    pub details: Details,
    pub format: Format,
}

/// The largest picture read, in pixels: SSTV's largest is 800 by 616, so
/// a stray photo is refused rather than fill the memory.
pub const MAX_PIXELS: usize = 2048 * 2048;

/// Reads a picture and its details.
pub fn read(bytes: &[u8]) -> Result<Picture, String> {
    let format = Format::sniff(bytes).ok_or("not a PNG, JPEG or BMP picture")?;
    let (image, details) = match format {
        Format::Png => read_png(bytes)?,
        Format::Jpeg => read_jpeg(bytes)?,
        Format::Bmp => (read_bmp(bytes)?, Details::default()),
    };
    Ok(Picture {
        image,
        details,
        format,
    })
}

fn too_big(w: usize, h: usize) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    if w.saturating_mul(h) > MAX_PIXELS {
        return Err(format!("{w} by {h} is too large for an SSTV picture"));
    }
    Ok(())
}

fn read_png(bytes: &[u8]) -> Result<(Image, Details), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    {
        let info = reader.info();
        too_big(info.width as usize, info.height as usize)?;
    }
    let mut buf = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (frame.width as usize, frame.height as usize);
    let data = &buf[..frame.buffer_size()];
    let rgb: Vec<u8> = match frame.color_type {
        png::ColorType::Rgb => data.to_vec(),
        png::ColorType::Rgba => data.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&v| [v, v, v]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks(2).flat_map(|p| [p[0], p[0], p[0]]).collect(),
        other => return Err(format!("unexpected colour type {other:?}")),
    };
    let image = Image::from_rgb(w, h, rgb).ok_or("unexpected picture size")?;
    // The chunks after the picture can hold text too: a damaged one fails
    // the file, rather than details meant to be kept being dropped.
    reader.finish().map_err(|e| e.to_string())?;
    let info = reader.info();
    let mut text: Vec<(String, String)> = Vec::new();
    // A field that can't be read fails the whole file, for the same
    // reason. They are kept in this order: international (iTXt), then
    // plain (tEXt), then compressed (zTXt).
    for t in &info.utf8_text {
        let v = t
            .get_text()
            .map_err(|e| format!("text {}: {e}", t.keyword))?;
        text.push((t.keyword.clone(), v));
    }
    for t in &info.uncompressed_latin1_text {
        text.push((t.keyword.clone(), t.text.clone()));
    }
    for t in &info.compressed_latin1_text {
        let v = t
            .get_text()
            .map_err(|e| format!("text {}: {e}", t.keyword))?;
        text.push((t.keyword.clone(), v));
    }
    let exif = png_chunk(bytes, b"eXIf");
    Ok((image, Details { text, exif }))
}

/// The first chunk of a kind in a PNG file.
fn png_chunk(bytes: &[u8], kind: &[u8; 4]) -> Option<Vec<u8>> {
    let mut i = 8;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().ok()?) as usize;
        let k = &bytes[i + 4..i + 8];
        let start = i + 8;
        let end = start.checked_add(len)?;
        if end > bytes.len() {
            return None;
        }
        if k == kind {
            return Some(bytes[start..end].to_vec());
        }
        if k == b"IEND" {
            return None;
        }
        i = end + 4;
    }
    None
}

fn read_jpeg(bytes: &[u8]) -> Result<(Image, Details), String> {
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    dec.read_info().map_err(|e| e.to_string())?;
    let info = dec.info().ok_or("no picture in the JPEG")?;
    let (w, h) = (info.width as usize, info.height as usize);
    too_big(w, h)?;
    let data = dec.decode().map_err(|e| e.to_string())?;
    use jpeg_decoder::PixelFormat;
    let rgb: Vec<u8> = match info.pixel_format {
        PixelFormat::RGB24 => data,
        PixelFormat::L8 => data.iter().flat_map(|&v| [v, v, v]).collect(),
        PixelFormat::L16 => {
            return Err("16-bit JPEGs aren't supported".into());
        }
        PixelFormat::CMYK32 => data
            .chunks(4)
            .flat_map(|p| {
                let k = 255 - p[3] as u32;
                let f = |c: u8| ((255 - c as u32) * k / 255) as u8;
                [f(p[0]), f(p[1]), f(p[2])]
            })
            .collect(),
    };
    let image = Image::from_rgb(w, h, rgb).ok_or("unexpected picture size")?;
    let (exif, comment) = jpeg_segments(bytes);
    let mut text = Vec::new();
    if let Some(c) = comment {
        text.push(("Comment".to_string(), c));
    }
    Ok((image, Details { text, exif }))
}

/// A JPEG's EXIF block (from its APP1 segment, without the "Exif" mark)
/// and its comment, read from the segments before the picture data.
fn jpeg_segments(bytes: &[u8]) -> (Option<Vec<u8>>, Option<String>) {
    let (mut exif, mut comment) = (None, None);
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            break;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // Start of scan or end of image: no more details.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            break;
        }
        let body = &bytes[i + 4..i + 2 + len];
        if marker == 0xE1 && exif.is_none() && body.starts_with(b"Exif\0\0") {
            exif = Some(body[6..].to_vec());
        }
        if marker == 0xFE && comment.is_none() {
            let text = String::from_utf8_lossy(body)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            if !text.is_empty() {
                comment = Some(text);
            }
        }
        i += 2 + len;
    }
    (exif, comment)
}

fn read_bmp(bytes: &[u8]) -> Result<Image, String> {
    const SHORT: &str = "the BMP is cut short";
    // All the sums are checked: on 32-bit WebAssembly a hostile header
    // could otherwise wrap round to somewhere else in the file.
    let at = |i: usize, n: usize| -> Result<&[u8], String> {
        i.checked_add(n)
            .and_then(|end| bytes.get(i..end))
            .ok_or_else(|| SHORT.to_string())
    };
    let u16_at = |i: usize| at(i, 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |i: usize| at(i, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let offset = u32_at(10)? as usize;
    let header = u32_at(14)? as usize;
    if header < 40 {
        return Err("this kind of BMP isn't supported (an old OS/2 one)".into());
    }
    let w = u32_at(18)? as i32;
    let h = u32_at(22)? as i32;
    let bpp = u16_at(28)?;
    let compression = u32_at(30)?;
    if w <= 0 || h == 0 || h == i32::MIN {
        return Err("the BMP has no picture".into());
    }
    let top_down = h < 0;
    let (w, h) = (w as usize, h.unsigned_abs() as usize);
    too_big(w, h)?;
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(format!("{bpp}-bit BMPs aren't supported"));
    }
    // Uncompressed, or bit fields: only the usual layouts, read from the
    // masks (just after the 40-byte header, in every later header too).
    let masks_16_565 = match compression {
        0 => false,
        3 => {
            let m = (u32_at(54)?, u32_at(58)?, u32_at(62)?);
            match (bpp, m) {
                (16, (0x7C00, 0x03E0, 0x001F)) => false,
                (16, (0xF800, 0x07E0, 0x001F)) => true,
                (32, (0x00FF_0000, 0x0000_FF00, 0x0000_00FF)) => false,
                _ => return Err("this BMP's colour layout isn't supported".into()),
            }
        }
        _ => return Err("compressed BMPs aren't supported".into()),
    };
    let pal: Vec<[u8; 3]> = if bpp <= 8 {
        let start = 14usize.checked_add(header).ok_or(SHORT)?;
        let count = match u32_at(46)? as usize {
            0 => 1usize << bpp,
            c => c.min(256),
        };
        let mut p = Vec::with_capacity(count);
        for k in 0..count {
            let e = at(start.checked_add(k * 4).ok_or(SHORT)?, 3)
                .map_err(|_| "the BMP's colours are cut short".to_string())?;
            p.push([e[2], e[1], e[0]]);
        }
        p
    } else {
        Vec::new()
    };
    let stride = w
        .checked_mul(bpp as usize)
        .ok_or(SHORT)?
        .div_ceil(32)
        .checked_mul(4)
        .ok_or(SHORT)?;
    let mut rgb = vec![0u8; w * h * 3];
    for row in 0..h {
        let src_row = if top_down { row } else { h - 1 - row };
        let start = src_row
            .checked_mul(stride)
            .and_then(|o| o.checked_add(offset))
            .ok_or(SHORT)?;
        let line = at(start, stride).map_err(|_| "the BMP's picture is cut short".to_string())?;
        for x in 0..w {
            let px = match bpp {
                24 => [line[x * 3 + 2], line[x * 3 + 1], line[x * 3]],
                32 => [line[x * 4 + 2], line[x * 4 + 1], line[x * 4]],
                16 => {
                    let v = u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]) as u32;
                    if masks_16_565 {
                        [
                            ((v >> 11) & 31) * 255 / 31,
                            ((v >> 5) & 63) * 255 / 63,
                            (v & 31) * 255 / 31,
                        ]
                        .map(|c| c as u8)
                    } else {
                        [
                            ((v >> 10) & 31) * 255 / 31,
                            ((v >> 5) & 31) * 255 / 31,
                            (v & 31) * 255 / 31,
                        ]
                        .map(|c| c as u8)
                    }
                }
                _ => {
                    let bits = bpp as usize;
                    let bit = x * bits;
                    let byte = line[bit / 8];
                    let shift = 8 - bits - (bit % 8);
                    let idx = ((byte >> shift) as usize) & ((1 << bits) - 1);
                    *pal.get(idx).ok_or("a BMP colour is missing")?
                }
            };
            let i = (row * w + x) * 3;
            rgb[i..i + 3].copy_from_slice(&px);
        }
    }
    Image::from_rgb(w, h, rgb).ok_or_else(|| "unexpected picture size".into())
}

/// A PNG of `image`, carrying `details`: the text fields as international
/// text chunks, and the EXIF block as an `eXIf` chunk.
pub fn write_png(image: &Image, details: &Details) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, image.width as u32, image.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        for (k, v) in &details.text {
            // Keywords are 1 to 79 printable Latin-1 characters, with no
            // space at either end: a bad one is skipped rather than lose
            // the picture.
            if keyword_ok(k) {
                let _ = enc.add_itxt_chunk(k.clone(), v.clone());
            }
        }
        let mut w = enc.write_header().expect("PNG header");
        if let Some(exif) = &details.exif {
            let _ = w.write_chunk(png::chunk::eXIf, exif);
        }
        w.write_image_data(&image.rgb).expect("PNG data");
    }
    out
}

/// Whether `k` can be a PNG text keyword.
fn keyword_ok(k: &str) -> bool {
    let n = k.chars().count();
    (1..=79).contains(&n)
        && !k.starts_with(' ')
        && !k.ends_with(' ')
        && !k.contains("  ")
        && k.chars()
            .all(|c| (' '..='~').contains(&c) || ('\u{a1}'..='\u{ff}').contains(&c))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small made-up picture.
    pub fn picture(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                img.rgb[i] = (x * 255 / w.max(1)) as u8;
                img.rgb[i + 1] = (y * 255 / h.max(1)) as u8;
                img.rgb[i + 2] = ((x + y) % 256) as u8;
            }
        }
        img
    }

    /// A 24-bit BMP of `img`, bottom-up.
    pub fn bmp(img: &Image) -> Vec<u8> {
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

    /// A tiny JPEG with an EXIF block and a comment: a baseline JPEG
    /// isn't written by any dependency, so the details are spliced into
    /// the start of one made by hand below.
    pub fn with_jpeg_details(jpeg: &[u8], exif: &[u8], comment: &str) -> Vec<u8> {
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
    fn a_png_comes_back_with_its_details() {
        let img = picture(40, 30);
        let d = Details {
            text: vec![
                ("Title".into(), "M7XYZ test".into()),
                ("Frequency".into(), "145.800 MHz".into()),
            ],
            exif: Some(b"MM\0*\0\0\0\x08\0\0".to_vec()),
        };
        let p = read(&write_png(&img, &d)).unwrap();
        assert_eq!(p.format, Format::Png);
        assert_eq!(p.image, img);
        assert_eq!(p.details, d);
    }

    #[test]
    fn a_bmp_is_read_the_right_way_up() {
        let img = picture(13, 7);
        let p = read(&bmp(&img)).unwrap();
        assert_eq!(p.format, Format::Bmp);
        assert_eq!(p.image, img);
    }

    #[test]
    fn jpeg_details_are_found_before_the_picture() {
        // Only the segments are read here; decoding is tested with a real
        // JPEG in `tests/`.
        let fake = [0xFF, 0xD8, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9];
        let j = with_jpeg_details(&fake, b"II*\0", "Robot 36 from M7XYZ");
        let (exif, comment) = jpeg_segments(&j);
        assert_eq!(exif.as_deref(), Some(&b"II*\0"[..]));
        assert_eq!(comment.as_deref(), Some("Robot 36 from M7XYZ"));
    }

    /// A 16-bit bit-field BMP of one row, with the given masks.
    fn bmp16(px: &[u16], masks: [u32; 3]) -> Vec<u8> {
        let w = px.len();
        let stride = (w * 2).div_ceil(4) * 4;
        let offset = 54 + 12;
        let mut b = b"BM".to_vec();
        b.extend_from_slice(&((offset + stride) as u32).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&(offset as u32).to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&(w as i32).to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[0; 20]);
        for m in masks {
            b.extend_from_slice(&m.to_le_bytes());
        }
        let mut line: Vec<u8> = px.iter().flat_map(|v| v.to_le_bytes()).collect();
        line.resize(stride, 0);
        b.extend_from_slice(&line);
        b
    }

    #[test]
    fn a_565_bmp_reads_its_colours_right() {
        let b = bmp16(&[0xF800, 0x07E0, 0x001F], [0xF800, 0x07E0, 0x001F]);
        let p = read(&b).unwrap();
        assert_eq!(p.image.pixel(0, 0), [255, 0, 0]);
        assert_eq!(p.image.pixel(1, 0), [0, 255, 0]);
        assert_eq!(p.image.pixel(2, 0), [0, 0, 255]);
        // An unusual layout is refused, not read with the wrong colours.
        let odd = bmp16(&[1], [0x000F, 0x00F0, 0x0F00]);
        assert!(read(&odd).unwrap_err().contains("colour layout"));
    }

    #[test]
    fn a_wrapping_bmp_header_is_refused() {
        let mut b = bmp(&picture(4, 4));
        b[14..18].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        b[28..30].copy_from_slice(&8u16.to_le_bytes());
        assert!(read(&b).is_err());
    }

    #[test]
    fn bad_keywords_are_skipped_not_fatal() {
        let d = Details {
            text: vec![
                ("".into(), "x".into()),
                (" Padded".into(), "x".into()),
                ("Good".into(), "kept".into()),
                ("é".repeat(80), "x".into()),
            ],
            exif: None,
        };
        let p = read(&write_png(&picture(4, 4), &d)).unwrap();
        assert_eq!(
            p.details.text,
            vec![("Good".to_string(), "kept".to_string())]
        );
    }

    #[test]
    fn other_files_are_refused() {
        assert!(read(b"GIF89a....").is_err());
        assert!(Format::by_name("a/B.JPG"));
        assert!(!Format::by_name("pass.wav"));
    }

    #[test]
    fn a_huge_bmp_is_refused() {
        let mut b = bmp(&picture(2, 2));
        b[18..22].copy_from_slice(&5000i32.to_le_bytes());
        b[22..26].copy_from_slice(&5000i32.to_le_bytes());
        assert!(read(&b).unwrap_err().contains("too large"));
    }
}
