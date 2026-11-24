//! Session identity: colour and two-letter glyph, unique among live sessions.

/// 12 colours, all ≥ 4.5:1 against the #0D1117 background, spread in lightness and hue.
pub const PALETTE: [&str; 12] = [
    "#FFA657", // orange
    "#58A6FF", // blue
    "#7EE787", // light green
    "#D2A8FF", // lavender
    "#FF7B72", // coral
    "#56D4DD", // cyan
    "#E3B341", // gold
    "#DB61A2", // magenta
    "#3FB950", // green (darker)
    "#A5D6FF", // pale blue (light)
    "#FF9BCE", // pink (light)
    "#B4A0FF", // violet (mid)
];

pub const BACKGROUND: &str = "#0D1117";

fn channel(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn luminance(hex: &str) -> f64 {
    let h = hex.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    0.2126 * channel(p(0)) + 0.7152 * channel(p(2)) + 0.0722 * channel(p(4))
}

pub fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Lowest palette colour not used by `taken`; if all are taken, the least used one.
pub fn pick_color(taken: &[&str]) -> String {
    let p: Vec<String> = PALETTE.iter().map(|s| s.to_string()).collect();
    pick_color_from(&p, taken)
}

pub fn pick_color_from(palette: &[String], taken: &[&str]) -> String {
    for c in palette {
        if !taken.iter().any(|t| t.eq_ignore_ascii_case(c)) {
            return c.clone();
        }
    }
    let mut best = (usize::MAX, &palette[0]);
    for c in palette {
        let n = taken.iter().filter(|t| t.eq_ignore_ascii_case(c)).count();
        if n < best.0 {
            best = (n, c);
        }
    }
    best.1.clone()
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'A' | 'E' | 'I' | 'O' | 'U')
}

/// Candidate glyphs for a title, best first.
/// "Pix fee labels" → PX (first letter + next consonant of the first word), then PF, PL, ...
pub fn glyph_candidates(title: &str) -> Vec<String> {
    let words: Vec<Vec<char>> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_uppercase().chars().collect())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let push = |a: char, b: char, out: &mut Vec<String>| {
        if a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric() {
            let s: String = [a, b].iter().collect();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    };
    if let Some(first) = words.first() {
        let a = first[0];
        for &c in first.iter().skip(1) {
            if !is_vowel(c) && c.is_ascii_alphabetic() {
                push(a, c, &mut out);
                break;
            }
        }
        for w in words.iter().skip(1) {
            push(a, w[0], &mut out);
        }
        for &c in first.iter().skip(1) {
            push(a, c, &mut out);
        }
        for w in words.iter().skip(1) {
            for &c in w.iter().skip(1) {
                push(a, c, &mut out);
            }
        }
        for (i, w) in words.iter().enumerate() {
            for v in words.iter().skip(i + 1) {
                push(w[0], v[0], &mut out);
            }
        }
    }
    let a = words.first().map(|w| w[0]).unwrap_or('S');
    for b in 'A'..='Z' {
        push(a, b, &mut out);
    }
    for a in 'A'..='Z' {
        for b in 'A'..='Z' {
            push(a, b, &mut out);
        }
    }
    out
}

pub fn pick_glyph(title: &str, taken: &[&str]) -> String {
    glyph_candidates(title)
        .into_iter()
        .find(|g| !taken.contains(&g.as_str()))
        .unwrap_or_else(|| "??".into())
}

/// First title from a prompt: up to 6 words, single line.
pub fn title_from_prompt(prompt: &str) -> String {
    let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let words: Vec<&str> = line.split_whitespace().take(6).collect();
    let mut t = words.join(" ");
    if t.chars().count() > 48 {
        t = t.chars().take(47).collect::<String>() + "…";
    }
    if t.is_empty() { "New session".into() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_contrast_is_accessible() {
        for c in PALETTE {
            let r = contrast(c, BACKGROUND);
            assert!(r >= 4.5, "{c} contrast {r:.2} < 4.5");
        }
    }

    #[test]
    fn palette_is_distinct_and_varies_in_lightness() {
        let mut l: Vec<f64> = PALETTE.iter().map(|c| luminance(c)).collect();
        l.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(l.last().unwrap() - l.first().unwrap() > 0.3);
        let set: std::collections::HashSet<_> = PALETTE.iter().collect();
        assert_eq!(set.len(), 12);
    }

    #[test]
    fn spec_glyph_example() {
        assert_eq!(pick_glyph("Pix fee labels", &[]), "PX");
    }

    #[test]
    fn glyphs_unique() {
        let mut taken: Vec<String> = vec![];
        for _ in 0..30 {
            let refs: Vec<&str> = taken.iter().map(|s| s.as_str()).collect();
            let g = pick_glyph("Pix fee labels", &refs);
            assert!(!taken.contains(&g));
            taken.push(g);
        }
    }

    #[test]
    fn colors_unique_until_exhausted() {
        let mut taken: Vec<String> = vec![];
        for _ in 0..12 {
            let refs: Vec<&str> = taken.iter().map(|s| s.as_str()).collect();
            let c = pick_color(&refs);
            assert!(!taken.contains(&c));
            taken.push(c);
        }
    }

    #[test]
    fn title_from_prompt_caps_words() {
        assert_eq!(title_from_prompt("\n  fix the pix fee labels in the checkout page please"), "fix the pix fee labels in");
        assert_eq!(title_from_prompt(""), "New session");
    }
}
