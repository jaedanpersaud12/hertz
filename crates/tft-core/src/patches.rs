//! Which patch a match was played on. Same rules as `src/patches.ts`.
//!
//! Riot's match data no longer carries a usable version ("TFT Unreal Version ?.?.?.?"), so a match is
//! dated against Community Dragon's list of patch folders, each created on release day. The calendar is
//! stored in the database (`kv` key `patch_calendar`) in the same JSON shape the TypeScript engine uses.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub const CALENDAR_KEY: &str = "patch_calendar";
pub const LISTING_URL: &str = "https://raw.communitydragon.org/json/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchStart {
    pub patch: String,
    /// Release time, milliseconds since 1970.
    pub since: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Calendar(pub Vec<PatchStart>);

/// One entry of Community Dragon's directory listing.
#[derive(Debug, Clone, Deserialize)]
pub struct ListingEntry {
    pub name: String,
    #[serde(default)]
    pub mtime: Option<String>,
}

static PATCH_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\.\d+$").unwrap());
static RELEASES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<Releases/(\d+\.\d+)>").unwrap());
static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Version (\d+\.\d+)").unwrap());

impl Calendar {
    /// Builds the calendar from the listing: patch folders only, oldest first.
    pub fn from_listing(entries: &[ListingEntry]) -> Self {
        let mut out: Vec<PatchStart> = entries
            .iter()
            .filter(|e| PATCH_NAME.is_match(&e.name))
            .filter_map(|e| {
                let t = httpdate::parse_http_date(e.mtime.as_deref()?).ok()?;
                let ms = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
                Some(PatchStart { patch: e.name.clone(), since: ms })
            })
            .collect();
        out.sort_by_key(|p| p.since); // stable, like Array.prototype.sort
        Calendar(out)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The patch live at a moment, or "unknown" before the calendar has been downloaded.
    /// Games older than the calendar count as its first patch.
    pub fn patch_at(&self, when_ms: i64) -> String {
        let Some(first) = self.0.first() else { return "unknown".into() };
        let mut current = &first.patch;
        for p in &self.0 {
            if p.since <= when_ms {
                current = &p.patch;
            }
        }
        current.clone()
    }

    /// "<Releases/15.22>" when Riot provides it, otherwise by date.
    pub fn patch_for(&self, game_version: Option<&str>, when_ms: i64) -> String {
        if let Some(v) = game_version {
            if let Some(m) = RELEASES.captures(v).or_else(|| VERSION.captures(v)) {
                return m[1].to_string();
            }
        }
        self.patch_at(when_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(rfc3339: &str) -> i64 {
        // Small helper: parse "2026-11-10T18:00:00Z" without another dependency.
        let d: Vec<i64> = rfc3339[..19].split(|c| "-T:".contains(c)).map(|x| x.parse().unwrap()).collect();
        let days = days_from_civil(d[0], d[1], d[2]);
        ((days * 24 + d[3]) * 60 + d[4]) * 60_000 + d[5] * 1000
    }
    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    #[test]
    fn same_answers_as_the_typescript_test() {
        let entry = |name: &str, mtime: &str| ListingEntry { name: name.into(), mtime: Some(mtime.into()) };
        let cal = Calendar::from_listing(&[
            entry("16.21", "Wed, 21 Oct 2026 07:15:23 GMT"),
            entry("latest", "Wed, 04 Nov 2026 07:47:21 GMT"),
            entry("16.22", "Wed, 04 Nov 2026 07:47:21 GMT"),
            entry("pbe", "Thu, 27 Feb 2025 06:58:53 GMT"),
        ]);
        assert_eq!(cal.0.iter().map(|p| p.patch.as_str()).collect::<Vec<_>>(), ["16.21", "16.22"]);
        assert_eq!(cal.patch_at(ms("2026-11-10T18:00:00Z")), "16.22");
        assert_eq!(cal.patch_at(ms("2026-11-03T18:00:00Z")), "16.21");
        assert_eq!(cal.patch_at(ms("2026-09-01T00:00:00Z")), "16.21");
        assert_eq!(Calendar::default().patch_at(0), "unknown");
        assert_eq!(cal.patch_for(Some("Linux Version 15.9.1 [PUBLIC] <Releases/15.9>"), 0), "15.9");
        assert_eq!(cal.patch_for(Some("TFT Unreal Version ?.?.?.?"), ms("2026-11-10T18:00:00Z")), "16.22");
        assert_eq!(cal.patch_for(None, ms("2026-11-03T18:00:00Z")), "16.21");
        assert_eq!(cal.0[1].since, 1_793_778_441_000);
    }

    #[test]
    fn stored_shape_is_the_typescript_one() {
        let cal = Calendar(vec![PatchStart { patch: "16.22".into(), since: 5 }]);
        assert_eq!(serde_json::to_string(&cal).unwrap(), r#"[{"patch":"16.22","since":5}]"#);
    }
}
