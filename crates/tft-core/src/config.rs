//! Settings, read from the environment (and `.env`) exactly as `src/config.ts` reads them.

use std::path::PathBuf;

pub const TIERS: [&str; 10] = [
    "IRON", "BRONZE", "SILVER", "GOLD", "PLATINUM", "EMERALD", "DIAMOND", "MASTER", "GRANDMASTER", "CHALLENGER",
];

/// Position of a tier in [`TIERS`], 0 for anything unknown.
pub fn tier_rank(tier: &str) -> i64 {
    TIERS.iter().position(|t| t.eq_ignore_ascii_case(tier)).unwrap_or(0) as i64
}

/// The tier name for a rank number, as stored on players.
pub fn tier_name(rank: i64) -> &'static str {
    TIERS.get(rank.max(0) as usize).copied().unwrap_or("IRON")
}

/// Every server [`platform_route`] knows, for the app's settings.
pub const PLATFORMS: [&str; 15] =
    ["na1", "br1", "la1", "la2", "euw1", "eun1", "tr1", "ru", "me1", "kr", "jp1", "oc1", "sg2", "tw2", "vn2"];

/// The routing groups, each with its own rate limit and crawl loop.
pub const ROUTES: [&str; 4] = ["americas", "europe", "asia", "sea"];

/// Which regional host serves match data for each platform.
pub fn platform_route(platform: &str) -> Option<&'static str> {
    Some(match platform {
        "na1" | "br1" | "la1" | "la2" => "americas",
        "euw1" | "eun1" | "tr1" | "ru" | "me1" => "europe",
        "kr" | "jp1" => "asia",
        "oc1" | "sg2" | "tw2" | "vn2" => "sea",
        _ => return None,
    })
}

/// Cumulative rank filters, the same idea as "Platinum+" in MetaTFT.
pub struct Bucket {
    pub key: &'static str,
    pub min: i64,
}
pub const BUCKETS: [Bucket; 4] = [
    Bucket { key: "ALL", min: 0 },
    Bucket { key: "PLATINUM+", min: 4 },
    Bucket { key: "DIAMOND+", min: 6 },
    Bucket { key: "MASTER+", min: 7 },
];

/// Stats are stored per patch and per scope: a rank filter plus a time window, e.g. "PLATINUM+@3d".
pub fn scope_key(bucket: &str, window: &str) -> String {
    if window.is_empty() { bucket.to_string() } else { format!("{bucket}@{window}") }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub api_key: String,
    /// `{host}` is replaced with a platform (na1) or route (americas). Overridden in tests.
    pub api_base: String,
    pub platforms: Vec<String>,
    pub tiers: Vec<String>,
    pub pages_per_division: u32,
    pub days: u32,
    pub snowball: bool,
    pub ranked_queue_id: i64,
    pub db_path: PathBuf,
    pub port: u16,
    pub riot_id: String,
    pub my_platform: String,
    /// Fraction of the published rate limit we allow ourselves.
    pub rate_margin: f64,
    /// Share of stored matches (0 to 1) whose 8 players get their real rank looked up, to check how well the
    /// seed player's rank stands in for the lobby (E7). Each sampled match costs 8 requests. 0 turns it off.
    pub rank_sample_rate: f64,
}

impl Config {
    /// Reads the process environment. Call `dotenvy::dotenv()` first to pick up `.env`.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let list = |key: &str, fallback: &str| -> Vec<String> {
            get(key)
                .unwrap_or_else(|| fallback.to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let num = |key: &str, fallback: f64| get(key).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(fallback);
        Config {
            api_key: get("RIOT_API_KEY").unwrap_or_default().trim().to_string(),
            api_base: get("RIOT_API_BASE").unwrap_or_else(|| "https://{host}.api.riotgames.com".into()),
            platforms: list("PLATFORMS", "na1,euw1,kr,vn2").into_iter().map(|p| p.to_lowercase()).collect(),
            tiers: list("TIERS", "CHALLENGER,GRANDMASTER,MASTER,DIAMOND,EMERALD,PLATINUM")
                .into_iter()
                .map(|t| t.to_uppercase())
                .collect(),
            pages_per_division: num("PAGES_PER_DIVISION", 5.0) as u32,
            days: num("DAYS", 7.0) as u32,
            snowball: get("SNOWBALL").map(|v| v != "false").unwrap_or(true),
            ranked_queue_id: num("RANKED_QUEUE_ID", 1100.0) as i64,
            db_path: PathBuf::from(get("DB_PATH").unwrap_or_else(|| "data/tft.db".into())),
            port: num("PORT", 5174.0) as u16,
            riot_id: get("RIOT_ID").unwrap_or_default(),
            my_platform: get("MY_PLATFORM").unwrap_or_default().to_lowercase(),
            rate_margin: num("RATE_MARGIN", 1.0),
            rank_sample_rate: num("RANK_SAMPLE_RATE", 0.01).clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_and_buckets_line_up() {
        assert_eq!(tier_rank("platinum"), 4);
        assert_eq!(tier_rank("CHALLENGER"), 9);
        assert_eq!(tier_rank("nonsense"), 0);
        for b in &BUCKETS[1..] {
            assert_eq!(tier_name(b.min), b.key.trim_end_matches('+'));
        }
        assert_eq!(scope_key("PLATINUM+", "3d"), "PLATINUM+@3d");
        assert_eq!(scope_key("ALL", ""), "ALL");
    }

    #[test]
    fn every_known_platform_has_a_route() {
        for p in PLATFORMS {
            assert!(ROUTES.contains(&platform_route(p).unwrap()), "{p}");
        }
        assert_eq!(platform_route("pbe1"), None);
    }

    #[test]
    fn defaults_match_the_typescript_config() {
        let c = Config::from_lookup(|_| None);
        assert_eq!(c.platforms, ["na1", "euw1", "kr", "vn2"]);
        assert_eq!(c.tiers.len(), 6);
        assert_eq!((c.pages_per_division, c.days, c.snowball, c.ranked_queue_id), (5, 7, true, 1100));
        assert_eq!(c.db_path, PathBuf::from("data/tft.db"));
        assert_eq!(c.rank_sample_rate, 0.01);
        let rate = |v: &str| Config::from_lookup(|k| (k == "RANK_SAMPLE_RATE").then(|| v.into())).rank_sample_rate;
        assert_eq!((rate("0"), rate("0.05"), rate("7"), rate("junk")), (0.0, 0.05, 1.0, 0.01));
        let c = Config::from_lookup(|k| (k == "SNOWBALL").then(|| "false".into()));
        assert!(!c.snowball);
    }
}
