//! The pipeline's health (E10): the engine log of warnings and errors, and the numbers the Data page shows.
//!
//! - `engine_log` holds what went wrong, newest last, capped at [`LOG_CAP`] rows. Region is a routing group
//!   (americas) for crawl problems, a server (na1) for ladder reads, and empty for the supervisor's jobs.
//! - `match_stored` records when each match was stored and for which server, so matches per hour can be counted.
//!   It is written by [`crate::Ingest::store`] in the same transaction as the match.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// The engine log keeps only this many of its newest rows.
pub const LOG_CAP: i64 = 10_000;

pub const WARN: &str = "warn";
pub const ERROR: &str = "error";

const HOUR_MS: i64 = 3_600_000;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Adds a row to the engine log and drops rows beyond [`LOG_CAP`].
pub fn log_event(conn: &Connection, region: &str, level: &str, message: &str) -> Result<()> {
    log_event_at(conn, now_ms(), region, level, message)
}

pub fn log_event_at(conn: &Connection, at: i64, region: &str, level: &str, message: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO engine_log (at, region, level, message) VALUES (?1, ?2, ?3, ?4)",
        params![at, region, level, message],
    )?;
    let newest = conn.last_insert_rowid();
    conn.execute("DELETE FROM engine_log WHERE rowid <= ?1", [newest - LOG_CAP])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegionHealth {
    /// A server, such as na1.
    pub region: String,
    pub matches_last_hour: i64,
    /// Matches stored in each of the last 24 hours, oldest first; the last entry is the last hour.
    pub matches_last_24h_per_hour: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueDepth {
    pub region: String,
    /// Match ids waiting to be fetched.
    pub pending: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub at: i64,
    pub region: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Health {
    /// Every server that stored a match in the last 24 hours or has matches queued, by name.
    pub per_region: Vec<RegionHealth>,
    pub queue_depth_per_region: Vec<QueueDepth>,
    /// The newest warning or error in the engine log.
    pub last_error: Option<LogEntry>,
    /// When the stat tables were last built (ms), from `stat_meta`.
    pub stats_built_at: Option<i64>,
    /// When the newest match was stored (ms).
    pub newest_match_at: Option<i64>,
    /// Whole minutes since the last stats build.
    pub stats_age_minutes: Option<i64>,
}

/// The health numbers as of now.
pub fn health(conn: &Connection) -> Result<Health> {
    health_at(conn, now_ms())
}

/// The health numbers as of `now` (ms).
pub fn health_at(conn: &Connection, now: i64) -> Result<Health> {
    let mut hours: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    {
        let mut q = conn.prepare(
            "SELECT platform, (?1 - at) / ?2 AS ago, COUNT(*) FROM match_stored WHERE at > ?1 - 24 * ?2 AND at <= ?1
             GROUP BY platform, ago",
        )?;
        let rows =
            q.query_map(params![now, HOUR_MS], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (platform, ago, n) = row?;
            if (0..24).contains(&ago) {
                hours.entry(platform).or_insert_with(|| vec![0; 24])[23 - ago as usize] += n;
            }
        }
    }
    // Through the queue_pending index (route, status).
    let queue: Vec<QueueDepth> = conn
        .prepare(
            "SELECT platform, COUNT(*) FROM match_queue WHERE route IN ('americas', 'europe', 'asia', 'sea') AND status = 0
             GROUP BY platform ORDER BY platform",
        )?
        .query_map([], |r| Ok(QueueDepth { region: r.get(0)?, pending: r.get(1)? }))?
        .collect::<rusqlite::Result<_>>()?;
    for q in &queue {
        hours.entry(q.region.clone()).or_insert_with(|| vec![0; 24]);
    }
    let per_region = hours
        .into_iter()
        .map(|(region, h)| RegionHealth { region, matches_last_hour: h[23], matches_last_24h_per_hour: h })
        .collect();

    let last_error = conn
        .query_row(
            "SELECT at, COALESCE(region, ''), COALESCE(level, ''), COALESCE(message, '') FROM engine_log
             ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok(LogEntry { at: r.get(0)?, region: r.get(1)?, level: r.get(2)?, message: r.get(3)? }),
        )
        .optional()?;
    let has_meta: bool =
        conn.query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = 'stat_meta'", [], |r| r.get(0))?;
    let stats_built_at: Option<i64> =
        if has_meta { conn.query_row("SELECT MAX(built_at) FROM stat_meta", [], |r| r.get(0))? } else { None };
    let newest_match_at: Option<i64> = conn.query_row("SELECT MAX(at) FROM match_stored", [], |r| r.get(0))?;
    Ok(Health {
        per_region,
        queue_depth_per_region: queue,
        last_error,
        stats_built_at,
        newest_match_at,
        stats_age_minutes: stats_built_at.map(|b| (now - b).max(0) / 60_000),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        crate::apply_migrations(&conn).unwrap();
        conn.execute_batch(&crate::schema::stats_schema()).unwrap();
        conn
    }

    fn stored(conn: &Connection, id: &str, platform: &str, at: i64) {
        conn.execute("INSERT INTO match_stored (match_id, platform, at) VALUES (?1, ?2, ?3)", params![id, platform, at])
            .unwrap();
    }

    fn queued(conn: &Connection, id: &str, route: &str, platform: &str, status: i64) {
        conn.execute(
            "INSERT INTO match_queue (match_id, route, platform, seed_rank, status) VALUES (?1, ?2, ?3, 9, ?4)",
            params![id, route, platform, status],
        )
        .unwrap();
    }

    #[test]
    fn an_empty_database_has_no_numbers() {
        let h = health_at(&db(), NOW).unwrap();
        assert_eq!(
            h,
            Health {
                per_region: vec![],
                queue_depth_per_region: vec![],
                last_error: None,
                stats_built_at: None,
                newest_match_at: None,
                stats_age_minutes: None,
            }
        );
    }

    #[test]
    fn health_numbers_on_a_hand_made_database() {
        let conn = db();
        // na1: 3 in the last hour (one exactly at now), 2 about 5 hours ago, 1 just inside 24 h, 1 just outside.
        stored(&conn, "N1", "na1", NOW);
        stored(&conn, "N2", "na1", NOW - 10 * MIN);
        stored(&conn, "N3", "na1", NOW - 59 * MIN);
        stored(&conn, "N4", "na1", NOW - 5 * 60 * MIN - 1);
        stored(&conn, "N5", "na1", NOW - 5 * 60 * MIN - 30 * MIN);
        stored(&conn, "N6", "na1", NOW - 24 * 60 * MIN + 1);
        stored(&conn, "N7", "na1", NOW - 24 * 60 * MIN);
        // euw1: one 90 minutes ago, none in the last hour.
        stored(&conn, "E1", "euw1", NOW - 90 * MIN);
        // kr has queued matches only; stored (1) and skipped (2) rows do not count.
        queued(&conn, "K1", "asia", "kr", 0);
        queued(&conn, "K2", "asia", "kr", 0);
        queued(&conn, "K3", "asia", "kr", 1);
        queued(&conn, "N9", "americas", "na1", 0);
        queued(&conn, "N8", "americas", "na1", 2);
        conn.execute(
            "INSERT INTO stat_meta (patch, bucket, matches, comps, built_at) VALUES ('16.1', 'ALL', 1, 1, ?1), ('16.1', 'MASTER+', 1, 1, ?2)",
            params![NOW - 90 * MIN, NOW - 25 * MIN - 30_000],
        )
        .unwrap();
        log_event_at(&conn, NOW - 50 * MIN, "europe", WARN, "Riot is not answering. Pausing this region for a minute.").unwrap();
        log_event_at(&conn, NOW - 40 * MIN, "americas", ERROR, "The API key was refused.").unwrap();

        let h = health_at(&conn, NOW).unwrap();
        let regions: Vec<&str> = h.per_region.iter().map(|r| r.region.as_str()).collect();
        assert_eq!(regions, ["euw1", "kr", "na1"]);
        let na1 = &h.per_region[2];
        assert_eq!(na1.matches_last_hour, 3);
        let mut want = vec![0; 24];
        want[23] = 3; // last hour
        want[23 - 5] = 2; // 5 to 6 hours ago
        want[0] = 1; // 23 to 24 hours ago
        assert_eq!(na1.matches_last_24h_per_hour, want);
        let euw1 = &h.per_region[0];
        assert_eq!(euw1.matches_last_hour, 0);
        assert_eq!(euw1.matches_last_24h_per_hour.iter().sum::<i64>(), 1);
        assert_eq!(euw1.matches_last_24h_per_hour[22], 1, "90 minutes ago is the hour before last");
        assert_eq!(h.per_region[1].matches_last_24h_per_hour, vec![0; 24], "kr is listed for its queue");
        assert_eq!(
            h.queue_depth_per_region,
            [QueueDepth { region: "kr".into(), pending: 2 }, QueueDepth { region: "na1".into(), pending: 1 }]
        );
        assert_eq!(
            h.last_error,
            Some(LogEntry { at: NOW - 40 * MIN, region: "americas".into(), level: ERROR.into(), message: "The API key was refused.".into() })
        );
        assert_eq!(h.stats_built_at, Some(NOW - 25 * MIN - 30_000), "the newest build of any scope");
        assert_eq!(h.stats_age_minutes, Some(25));
        assert_eq!(h.newest_match_at, Some(NOW));
    }

    #[test]
    fn the_engine_log_keeps_only_the_newest_rows() {
        let conn = db();
        for i in 0..LOG_CAP + 25 {
            log_event_at(&conn, i, "", WARN, &format!("problem {i}")).unwrap();
        }
        let (n, oldest): (i64, i64) =
            conn.query_row("SELECT COUNT(*), MIN(at) FROM engine_log", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((n, oldest), (LOG_CAP, 25));
        let last = health_at(&conn, 0).unwrap().last_error.unwrap();
        assert_eq!(last.message, format!("problem {}", LOG_CAP + 24));
    }
}
