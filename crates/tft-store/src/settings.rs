//! Crawl settings chosen in the app (E9): which servers to crawl and which ranked tiers to seed from. Stored in
//! `kv` under [`CRAWL_SETTINGS`]; until they are set, `PLATFORMS` and `TIERS` from `.env` apply. The crawler reads
//! them at the top of every round, so a change takes effect without a restart.

use crate::{kv_get, kv_set};
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tft_core::config::{Config, TIERS, platform_route};

/// `kv` key of the saved [`CrawlSettings`].
pub const CRAWL_SETTINGS: &str = "crawl_settings";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlSettings {
    /// Servers, lower case, as Riot names them (na1, euw1).
    pub platforms: Vec<String>,
    /// Ranked tiers, upper case (CHALLENGER, DIAMOND).
    pub tiers: Vec<String>,
}

/// Why [`set_crawl_settings`] refused a change. The message is written for the person who made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    NoPlatforms,
    NoTiers,
    UnknownPlatforms(Vec<String>),
    UnknownTiers(Vec<String>),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::NoPlatforms => write!(f, "Choose at least one server to crawl."),
            SettingsError::NoTiers => write!(f, "Choose at least one tier to collect players from."),
            SettingsError::UnknownPlatforms(p) => write!(f, "Unknown server(s): {}.", p.join(", ")),
            SettingsError::UnknownTiers(t) => write!(f, "Unknown tier(s): {}.", t.join(", ")),
        }
    }
}

impl std::error::Error for SettingsError {}

impl CrawlSettings {
    /// The `.env` values, used while nothing is saved.
    pub fn from_config(config: &Config) -> Self {
        CrawlSettings { platforms: config.platforms.clone(), tiers: config.tiers.clone() }
    }

    /// Trims, fixes the case, drops repeats (keeping the first) and checks every name is known.
    pub fn validated(&self) -> Result<CrawlSettings, SettingsError> {
        fn clean(xs: &[String], upper: bool) -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for x in xs {
                let x = x.trim();
                let x = if upper { x.to_uppercase() } else { x.to_lowercase() };
                if !x.is_empty() && !out.contains(&x) {
                    out.push(x);
                }
            }
            out
        }
        let platforms = clean(&self.platforms, false);
        let tiers = clean(&self.tiers, true);
        if platforms.is_empty() {
            return Err(SettingsError::NoPlatforms);
        }
        if tiers.is_empty() {
            return Err(SettingsError::NoTiers);
        }
        let unknown: Vec<String> = platforms.iter().filter(|p| platform_route(p).is_none()).cloned().collect();
        if !unknown.is_empty() {
            return Err(SettingsError::UnknownPlatforms(unknown));
        }
        let unknown: Vec<String> = tiers.iter().filter(|t| !TIERS.contains(&t.as_str())).cloned().collect();
        if !unknown.is_empty() {
            return Err(SettingsError::UnknownTiers(unknown));
        }
        Ok(CrawlSettings { platforms, tiers })
    }
}

/// The saved settings, or the `.env` ones when none are saved. A saved value that cannot be read or is no longer
/// valid (edited by hand) also falls back to `.env`, so the crawl keeps going.
pub fn get_crawl_settings(conn: &Connection, config: &Config) -> Result<CrawlSettings> {
    let saved = kv_get::<CrawlSettings>(conn, CRAWL_SETTINGS).ok().flatten();
    Ok(saved.and_then(|s| s.validated().ok()).unwrap_or_else(|| CrawlSettings::from_config(config)))
}

/// Checks and saves new settings, returning them as saved (cleaned up). A refused change returns a
/// [`SettingsError`] inside the error, and nothing is saved.
pub fn set_crawl_settings(conn: &Connection, settings: &CrawlSettings) -> Result<CrawlSettings> {
    let clean = settings.validated()?;
    kv_set(conn, CRAWL_SETTINGS, &clean)?;
    Ok(clean)
}

/// Forgets the saved settings, so `.env` applies again.
pub fn reset_crawl_settings(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM kv WHERE key = ?1", [CRAWL_SETTINGS])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        conn
    }

    fn s(p: &[&str], t: &[&str]) -> CrawlSettings {
        CrawlSettings { platforms: p.iter().map(|x| x.to_string()).collect(), tiers: t.iter().map(|x| x.to_string()).collect() }
    }

    fn refused(conn: &Connection, settings: CrawlSettings) -> SettingsError {
        let err = set_crawl_settings(conn, &settings).unwrap_err();
        err.downcast_ref::<SettingsError>().cloned().unwrap_or_else(|| panic!("not a settings error: {err:#}"))
    }

    #[test]
    fn env_applies_until_settings_are_saved() {
        let conn = db();
        let config = Config::from_lookup(|k| match k {
            "PLATFORMS" => Some("na1,kr".into()),
            "TIERS" => Some("challenger".into()),
            _ => None,
        });
        assert_eq!(get_crawl_settings(&conn, &config).unwrap(), s(&["na1", "kr"], &["CHALLENGER"]));
        let saved = set_crawl_settings(&conn, &s(&[" EUW1 ", "oc1", "euw1"], &["diamond", "Master"])).unwrap();
        assert_eq!(saved, s(&["euw1", "oc1"], &["DIAMOND", "MASTER"]), "cleaned: trimmed, lower/upper case, no repeats");
        assert_eq!(get_crawl_settings(&conn, &config).unwrap(), saved);
        reset_crawl_settings(&conn).unwrap();
        assert_eq!(get_crawl_settings(&conn, &config).unwrap(), s(&["na1", "kr"], &["CHALLENGER"]));
    }

    #[test]
    fn bad_settings_are_refused_with_a_clear_message_and_nothing_is_saved() {
        let conn = db();
        let config = Config::from_lookup(|_| None);
        let before = get_crawl_settings(&conn, &config).unwrap();
        assert_eq!(refused(&conn, s(&[], &["MASTER"])), SettingsError::NoPlatforms);
        assert_eq!(refused(&conn, s(&["  "], &["MASTER"])), SettingsError::NoPlatforms);
        assert_eq!(refused(&conn, s(&["na1"], &[])), SettingsError::NoTiers);
        let e = refused(&conn, s(&["na1", "mars1", "pbe1"], &["MASTER"]));
        assert_eq!(e, SettingsError::UnknownPlatforms(vec!["mars1".into(), "pbe1".into()]));
        assert_eq!(e.to_string(), "Unknown server(s): mars1, pbe1.");
        let e = refused(&conn, s(&["na1"], &["MASTER", "WOOD"]));
        assert_eq!(e, SettingsError::UnknownTiers(vec!["WOOD".into()]));
        assert_eq!(e.to_string(), "Unknown tier(s): WOOD.");
        assert_eq!(SettingsError::NoPlatforms.to_string(), "Choose at least one server to crawl.");
        assert_eq!(get_crawl_settings(&conn, &config).unwrap(), before, "nothing was saved");
        assert_eq!(kv_get::<CrawlSettings>(&conn, CRAWL_SETTINGS).unwrap(), None);
    }

    #[test]
    fn a_hand_edited_bad_value_falls_back_to_env() {
        let conn = db();
        let config = Config::from_lookup(|_| None);
        kv_set(&conn, CRAWL_SETTINGS, &s(&["mars1"], &["MASTER"])).unwrap();
        assert_eq!(get_crawl_settings(&conn, &config).unwrap(), CrawlSettings::from_config(&config));
        conn.execute("UPDATE kv SET value = 'not json' WHERE key = ?1", [CRAWL_SETTINGS]).unwrap();
        assert_eq!(get_crawl_settings(&conn, &config).unwrap(), CrawlSettings::from_config(&config));
    }
}
