//! The record of how a best copy was made: which copies were used, how
//! they were lined up and corrected, and where each line came from. As
//! text for people, and as JSON for programs (the web page).

use crate::Combined;

/// What the record says about each copy, beside what [`Combined`] holds.
#[derive(Debug, Clone, Default)]
pub struct CopyInfo {
    /// Its name (a file name, or a path inside the folder).
    pub name: String,
    /// Anything done to it before combining, such as "made 2 times
    /// smaller (saved at 640 by 480)".
    pub note: Option<String>,
}

/// The record as text.
pub fn text(copies: &[CopyInfo], c: &Combined, file: &str) -> String {
    let mut s = format!("Best copy: {file}\n\n");
    s += &format!("Strongest copy: {}\n", copies[c.strongest].name);
    s += &format!(
        "Coverage: {:.1}% of lines ({} never received, shown hatched grey)\n",
        c.coverage * 100.0,
        c.missing_rows.len()
    );
    s += &format!(
        "Estimated noise: {:.1} levels, against {:.1} for the best single copy (0 to 255; lower is better)\n\n",
        c.noise,
        best_single(c)
    );
    s += "Copies (shift to line up, right and down; estimated noise; leave one out: how far it is from the others combined)\n";
    for (i, cp) in copies.iter().enumerate() {
        let (dx, dy) = c.shifts[i];
        s += &format!(
            "  {}. {}: shift ({dx}, {dy}), noise {:.1}{}{}{}{}\n",
            i + 1,
            cp.name,
            c.copy_noise[i],
            c.leave_one_out[i]
                .map(|v| format!(", leave one out {v:.1}"))
                .unwrap_or_default(),
            if c.swapped.contains(&i) {
                ", colours were swapped and are put back"
            } else {
                ""
            },
            cp.note
                .as_ref()
                .map(|n| format!(", {n}"))
                .unwrap_or_default(),
            if c.used.contains(&i) {
                ""
            } else {
                ", not used"
            },
        );
    }
    s += "\nWhere each line came from (copy number and share of the line):\n";
    for (y, src) in c.rows.iter().enumerate() {
        let parts: Vec<String> = src
            .iter()
            .map(|&(i, share)| format!("{} {:.0}%", i + 1, share * 100.0))
            .collect();
        s += &format!(
            "  {y:3}: {}\n",
            if parts.is_empty() {
                "not received".to_string()
            } else {
                parts.join(", ")
            }
        );
    }
    s
}

/// The estimated noise of the best single copy.
pub fn best_single(c: &Combined) -> f64 {
    c.copy_noise.iter().cloned().fold(f64::INFINITY, f64::min)
}

/// The record as JSON (one object). Lines are given as, for each line,
/// a list of [copy, share].
pub fn json(copies: &[CopyInfo], c: &Combined, file: &str) -> String {
    let mut s = String::from("{");
    s += &format!("\"file\":{},", quote(file));
    s += &format!("\"strongest\":{},", c.strongest);
    s += &format!("\"coverage\":{},", num(c.coverage));
    s += &format!("\"noise\":{},", num(c.noise));
    s += &format!("\"best_single_noise\":{},", num(best_single(c)));
    s += &format!(
        "\"missing_rows\":[{}],",
        c.missing_rows
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    s += "\"copies\":[";
    for (i, cp) in copies.iter().enumerate() {
        if i > 0 {
            s += ",";
        }
        s += &format!(
            "{{\"name\":{},\"shift\":[{},{}],\"noise\":{},\"leave_one_out\":{},\"swapped\":{},\"used\":{},\"note\":{}}}",
            quote(&cp.name),
            c.shifts[i].0,
            c.shifts[i].1,
            num(c.copy_noise[i]),
            c.leave_one_out[i].map(num).unwrap_or_else(|| "null".into()),
            c.swapped.contains(&i),
            c.used.contains(&i),
            cp.note.as_deref().map(quote).unwrap_or_else(|| "null".into()),
        );
    }
    s += "],\"rows\":[";
    for (y, src) in c.rows.iter().enumerate() {
        if y > 0 {
            s += ",";
        }
        s += "[";
        s += &src
            .iter()
            .map(|&(i, share)| format!("[{i},{}]", num(share as f64)))
            .collect::<Vec<_>>()
            .join(",");
        s += "]";
    }
    s += "]}";
    s
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{:.3}", v)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        "null".into()
    }
}

/// A JSON string.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
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

    #[test]
    fn strings_are_escaped() {
        assert_eq!(quote("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(4.5), "4.5");
        assert_eq!(num(3.0), "3");
        assert_eq!(num(f64::INFINITY), "null");
        assert_eq!(num(0.0), "0");
    }
}
