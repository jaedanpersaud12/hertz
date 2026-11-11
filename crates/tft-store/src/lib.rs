//! The SQLite database: opening and upgrading it, small key-value settings, and storing matches.

mod ingest;
mod rank_sample;
pub mod schema;

pub use ingest::{Ingest, relabel_unknown};
pub use rank_sample::{
    UNRANKED,
    BucketAccuracy, RankSampleAccuracy, bucket_of, complete_sample, is_sampled, pending_samples, queue_sample,
    rank_sample_accuracy, sample_done, save_rank,
};
pub use rusqlite::{Connection, Error, params};

use anyhow::{Context, Result};
use rusqlite::{OpenFlags, OptionalExtension};
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;
use std::time::Duration;
use tft_core::patches::{CALENDAR_KEY, Calendar};

/// Opens (and creates or upgrades) the database read-write, as `openDb()` in `src/db.ts` does.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(Duration::from_millis(60_000))?; // the stats step can hold the write lock for a while
    conn.pragma_update(None, "journal_mode", "WAL")?; // lets the app read while the crawler writes
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(schema::SCHEMA)?;
    migrate(&conn)?; // adds columns to databases made by an older version, drops outdated stat tables
    conn.execute_batch(&schema::stats_schema())?;
    Ok(conn)
}

/// Opens an existing database for reading only. Used by the app and the overlay.
pub fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| format!("opening {} read-only", path.display()))?;
    conn.busy_timeout(Duration::from_millis(10_000))?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let has = |table: &str, col: &str| -> Result<bool> {
        let n: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [col],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    };
    if !has("participants", "comp_key")? {
        conn.execute_batch("ALTER TABLE participants ADD COLUMN comp_key TEXT")?;
    }
    if !has("units", "rarity")? {
        conn.execute_batch("ALTER TABLE units ADD COLUMN rarity INTEGER")?;
    }
    apply_migrations(conn)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != schema::STATS_VERSION {
        for t in schema::STAT_TABLES {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS {t}"))?;
        }
        conn.pragma_update(None, "user_version", schema::STATS_VERSION)?;
    }
    Ok(())
}

/// Applies the numbered [`schema::MIGRATIONS`] in order. Safe to call any number of times.
pub fn apply_migrations(conn: &Connection) -> Result<()> {
    for (n, sql) in schema::MIGRATIONS {
        conn.execute_batch(sql).with_context(|| format!("applying migration {n}"))?;
    }
    Ok(())
}

pub fn kv_get<T: DeserializeOwned>(conn: &Connection, key: &str) -> Result<Option<T>> {
    let raw: Option<String> =
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    Ok(match raw {
        Some(s) => Some(serde_json::from_str(&s).with_context(|| format!("reading setting {key}"))?),
        None => None,
    })
}

pub fn kv_set<T: Serialize>(conn: &Connection, key: &str, value: &T) -> Result<()> {
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

/// The patch calendar saved by the last refresh, or an empty one.
pub fn load_calendar(conn: &Connection) -> Result<Calendar> {
    Ok(kv_get(conn, CALENDAR_KEY)?.unwrap_or_default())
}

pub fn save_calendar(conn: &Connection, cal: &Calendar) -> Result<()> {
    kv_set(conn, CALENDAR_KEY, cal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_a_first_version_database() {
        let dir = std::env::temp_dir().join(format!("tft-store-test-{}", std::process::id()));
        let path = dir.join("old.db");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE participants (id INTEGER PRIMARY KEY, match_id TEXT NOT NULL, puuid TEXT NOT NULL, placement INTEGER NOT NULL, level INTEGER, last_round INTEGER, gold_left INTEGER, damage INTEGER);
                 CREATE TABLE units (participant_id INTEGER NOT NULL, character_id TEXT NOT NULL, star INTEGER NOT NULL, n_items INTEGER NOT NULL, build TEXT);
                 CREATE TABLE stat_units (patch TEXT, bucket TEXT, character_id TEXT, games INTEGER);",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let cols = |t: &str| -> Vec<String> {
            conn.prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert!(cols("participants").contains(&"comp_key".to_string()));
        assert!(cols("units").contains(&"rarity".to_string()));
        assert!(cols("stat_units").contains(&"avg_star".to_string()));

        kv_set(&conn, "x", &vec![1, 2]).unwrap();
        assert_eq!(kv_get::<Vec<i32>>(&conn, "x").unwrap(), Some(vec![1, 2]));
        assert_eq!(kv_get::<i32>(&conn, "missing").unwrap(), None);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
