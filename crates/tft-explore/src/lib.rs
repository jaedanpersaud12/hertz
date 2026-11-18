//! The Explorer index (PRD 4, X5): any "boards with these conditions" question answered without touching SQLite.
//!
//! One file per patch, next to the database (`tft.db.explore/16.22.00000003.hzx`), written by the stats step and
//! memory-mapped by the app. It holds:
//!
//! - **Rows**: one fixed-width [`Row`] per board (placement, level, rank, time, comp, then offsets into the unit,
//!   trait and item arrays), in participant id order.
//! - **Arrays**: every board's units (id, star, items held) and traits (id, tier), packed into `u16`s, and held
//!   items (unit, item) in `u32`s, laid end to end.
//! - **Bitmaps**: one Roaring bitmap of row numbers per unit, unit at star, unit with item, trait, trait at tier,
//!   item, level, placement, rank and queue. A condition is a bitmap and a query is their intersection; rank
//!   buckets and the ranked-queue rule are unions of the rank and queue bitmaps, so one index serves every scope.
//!
//! Grouping walks only the matching rows, split across cores ([`Index::explore`]).
//!
//! Each write is a new file with the next generation number, renamed into place once complete, so a reader always
//! has the last complete snapshot and a writer never changes a file someone has mapped. A file that is missing,
//! cut short or from another format version is not used: the app answers from SQL and rebuilds it ([`Explorer`]).
//! A stats run adds only the boards stored since the last file when nothing older changed ([`refresh`]).

mod build;
#[cfg(test)]
mod build_tests;
mod file;
mod query;
mod store;

pub use build::{Builder, Refreshed, RefreshKind, refresh};
pub use file::{FORMAT_VERSION, Index, Kind, MAX_TRAITS, MAX_UNITS, Row, key, trait_parts, unit_parts};
pub use query::{Answer, Counts, Query};
pub use store::{Explorer, Mode, PatchStatus};

use std::path::{Path, PathBuf};

/// The folder that holds a database's index files: the database's path with `.explore` added.
pub fn index_dir(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_owned();
    s.push(".explore");
    PathBuf::from(s)
}

/// A patch name as a file name: letters, digits, `.`, `_` and `-` as they are, any other byte as `%XX`.
fn stem(patch: &str) -> String {
    let mut out = String::new();
    for b in patch.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn file_name(patch: &str, generation: u64) -> String {
    format!("{}.{generation:08}.hzx", stem(patch))
}

/// Every complete index file for `patch` in `dir`, oldest generation first.
pub fn generations(dir: &Path, patch: &str) -> Vec<(u64, PathBuf)> {
    let want = stem(patch);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_suffix(".hzx") else { continue };
        let Some((s, g)) = rest.rsplit_once('.') else { continue };
        if s == want && g.len() == 8 && let Ok(g) = g.parse::<u64>() {
            out.push((g, e.path()));
        }
    }
    out.sort();
    out
}

/// The newest complete index file for `patch`, if any.
pub fn newest(dir: &Path, patch: &str) -> Option<(u64, PathBuf)> {
    generations(dir, patch).pop()
}

/// Deletes the files for `patch` older than `generation`, as far as it can. One still mapped (a question in flight,
/// or on Windows any mapping at all) stays, and goes at the next call: every write and every switch to a newer file
/// calls this.
pub fn remove_older(dir: &Path, patch: &str, generation: u64) {
    for (g, old) in generations(dir, patch) {
        if g < generation {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// Deletes every index file for every patch (after `tft recluster`, which relabels boards the files hold).
pub fn remove_all(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_keep_patches_apart() {
        assert_eq!(file_name("16.22", 3), "16.22.00000003.hzx");
        assert_eq!(stem("a/b c"), "a%2Fb%20c");
        assert_eq!(stem("%"), "%25");
        assert_ne!(stem("16.2"), stem("16.2 "));
    }
}
