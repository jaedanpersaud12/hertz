//! String ordering that matches JavaScript's `a.localeCompare(b)` for the ids Riot uses.
//!
//! The TypeScript engine breaks ties with `localeCompare`, which sorts the way a dictionary does:
//! punctuation before digits before letters, and letters case-insensitively first ("bastion" before
//! "Bilgewater"). Plain byte order would put every capital letter first, pick a different carry or trait
//! on a tie, and so give a different comp label. The test vectors in `locale_vectors.json` were produced by
//! Node and pin the behaviour.

use std::cmp::Ordering;

/// Primary weight: which group the character sorts in, then its case-folded value.
fn primary(c: char) -> (u8, u32) {
    let base = fold(c);
    if c.is_whitespace() {
        (0, base as u32)
    } else if base.is_ascii_digit() {
        (2, base as u32)
    } else if base.is_alphabetic() {
        (3, base as u32)
    } else {
        // Punctuation and symbols, in the order the Unicode collation table gives them ("_" before "-").
        let rank = PUNCTUATION.find(c).map(|i| i as u32).unwrap_or(1000 + c as u32);
        (1, rank)
    }
}

const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

/// Case and accent folding for the characters that show up in ids and names.
fn fold(c: char) -> char {
    let c = match c {
        'À'..='Å' | 'à'..='å' => 'a',
        'Ç' | 'ç' => 'c',
        'È'..='Ë' | 'è'..='ë' => 'e',
        'Ì'..='Ï' | 'ì'..='ï' => 'i',
        'Ñ' | 'ñ' => 'n',
        'Ò'..='Ö' | 'ò'..='ö' => 'o',
        'Ù'..='Ü' | 'ù'..='ü' => 'u',
        _ => c,
    };
    c.to_lowercase().next().unwrap_or(c)
}

/// Secondary weight: unaccented before accented.
fn accent(c: char) -> u8 {
    u8::from(c.is_alphabetic() && !c.is_ascii())
}

/// Tertiary weight: lower case before upper case.
fn case(c: char) -> u8 {
    u8::from(c.is_uppercase())
}

pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    let level = |f: &dyn Fn(char) -> u64| a.chars().map(f).cmp(b.chars().map(f));
    level(&|c| {
        let (g, v) = primary(c);
        ((g as u64) << 32) | v as u64
    })
    .then_with(|| level(&|c| accent(c) as u64))
    .then_with(|| level(&|c| case(c) as u64))
    .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_node_locale_compare() {
        let vectors: Vec<(String, String, i32)> = serde_json::from_str(include_str!("locale_vectors.json")).unwrap();
        let mut wrong = Vec::new();
        for (a, b, want) in &vectors {
            let got = match locale_cmp(a, b) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            if got != *want {
                wrong.push(format!("{a:?} vs {b:?}: node {want}, ours {got}"));
            }
        }
        assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), vectors.len(), wrong.join("\n"));
    }
}
