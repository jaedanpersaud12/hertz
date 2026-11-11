//! Keeping the database small (PRD 1 E8): raw rows for old patches are deleted, their stat tables are kept, and
//! the file is compacted. The scheduler runs both daily, when the engine is idle.

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior};

/// What [`prune_old_patches`] removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pruned {
    /// The patches whose raw rows were deleted, newest first.
    pub patches: Vec<String>,
    pub matches: usize,
    pub participants: usize,
}

/// Deletes the raw rows (matches, participants, units, unit items, traits and queue entries) of every patch older
/// than the newest `keep` patches. Stat tables are kept, so old patches still show their stats. Patches are ordered
/// by their latest match, as the stats rebuild orders them; matches still waiting for a patch label are kept.
pub fn prune_old_patches(conn: &mut Connection, keep: usize) -> Result<Pruned> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let patches: Vec<String> = tx
        .prepare(
            "SELECT patch FROM matches WHERE patch != 'unknown' GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT -1 OFFSET ?",
        )?
        .query_map([keep as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if patches.is_empty() {
        return Ok(Pruned::default());
    }

    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS gone_matches (match_id TEXT PRIMARY KEY) WITHOUT ROWID;
         CREATE TEMP TABLE IF NOT EXISTS gone_participants (id INTEGER PRIMARY KEY);
         DELETE FROM gone_matches; DELETE FROM gone_participants;",
    )?;
    {
        let mut add = tx.prepare("INSERT INTO gone_matches SELECT match_id FROM matches WHERE patch = ?")?;
        for patch in &patches {
            add.execute([patch])?;
        }
    }
    tx.execute(
        "INSERT INTO gone_participants SELECT id FROM participants WHERE match_id IN (SELECT match_id FROM gone_matches)",
        [],
    )?;
    for table in ["units", "unit_items", "traits"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE participant_id IN (SELECT id FROM gone_participants)"),
            [],
        )?;
    }
    let participants =
        tx.execute("DELETE FROM participants WHERE id IN (SELECT id FROM gone_participants)", [])?;
    tx.execute("DELETE FROM match_queue WHERE match_id IN (SELECT match_id FROM gone_matches)", [])?;
    let matches = tx.execute("DELETE FROM matches WHERE match_id IN (SELECT match_id FROM gone_matches)", [])?;
    tx.execute_batch("DROP TABLE gone_matches; DROP TABLE gone_participants;")?;
    tx.commit()?;
    Ok(Pruned { patches, matches, participants })
}

/// The database size before and after [`compact`], in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compacted {
    pub before: u64,
    pub after: u64,
}

/// Rewrites the database file without its free pages (VACUUM) and folds the write-ahead log back in, so the
/// space freed by pruning goes back to the disk. Needs no other connection writing at the time.
pub fn compact(conn: &Connection) -> Result<Compacted> {
    let size = |conn: &Connection| -> Result<u64> {
        let pages: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
        Ok((pages * page) as u64)
    };
    let before = size(conn)?;
    conn.execute_batch("VACUUM")?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    Ok(Compacted { before, after: size(conn)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// Raw row counts for one patch: matches, participants, units, unit items, traits, queue entries.
    fn raw_rows(conn: &Connection, patch: &str) -> Vec<i64> {
        let one = |sql: &str| -> i64 { conn.query_row(sql, [patch], |r| r.get(0)).unwrap() };
        let pids = "SELECT pa.id FROM participants pa JOIN matches m USING (match_id) WHERE m.patch = ?";
        vec![
            one("SELECT COUNT(*) FROM matches WHERE patch = ?"),
            one("SELECT COUNT(*) FROM participants WHERE match_id IN (SELECT match_id FROM matches WHERE patch = ?)"),
            one(&format!("SELECT COUNT(*) FROM units WHERE participant_id IN ({pids})")),
            one(&format!("SELECT COUNT(*) FROM unit_items WHERE participant_id IN ({pids})")),
            one(&format!("SELECT COUNT(*) FROM traits WHERE participant_id IN ({pids})")),
            one("SELECT COUNT(*) FROM match_queue WHERE match_id LIKE ? || '-%'"), // test ids are "{patch}-{n}"
        ]
    }

    #[test]
    fn prunes_patches_older_than_the_newest_three_and_compacts() {
        let dir = std::env::temp_dir().join(format!("tft-store-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("t.db");
        let mut conn = crate::open(&path).unwrap();
        let patches = ["90.1", "90.2", "90.3", "90.4", "90.5"];
        {
            let tx = conn.transaction().unwrap();
            for (p, patch) in patches.iter().enumerate() {
                for m in 0..60 {
                    let id = format!("{patch}-{m}");
                    let at = (p as i64) * 1_000_000 + m;
                    tx.execute("INSERT INTO matches VALUES (?, 'na1', ?, 99, ?, 2000, 8)", params![id, patch, at]).unwrap();
                    tx.execute("INSERT INTO match_queue VALUES (?, 'americas', 'na1', 8, 1)", [&id]).unwrap();
                    for s in 0..8 {
                        tx.execute(
                            "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, 8, 'T|U')",
                            params![id, format!("{id}-{s}"), s + 1],
                        )
                        .unwrap();
                        let pid = tx.last_insert_rowid();
                        for u in 0..8 {
                            tx.execute("INSERT INTO units VALUES (?, ?, 2, 1, NULL, 1)", params![pid, format!("U{u}")]).unwrap();
                            tx.execute("INSERT INTO unit_items VALUES (?, ?, 'Some item with a longish name')", params![pid, format!("U{u}")]).unwrap();
                        }
                        tx.execute("INSERT INTO traits VALUES (?, 'T', 3, 1, 1)", [pid]).unwrap();
                    }
                }
                tx.execute("INSERT INTO stat_meta VALUES (?, 'ALL', 60, 480, 1)", [patch]).unwrap();
                tx.execute("INSERT INTO stat_units VALUES (?, 'ALL', 'U1', 480, 2160, 60, 240, 2.0)", [patch]).unwrap();
            }
            // A match still waiting for a patch label is not part of any patch, so it is kept.
            tx.execute("INSERT INTO matches VALUES ('waiting', 'na1', 'unknown', 99, 0, 2000, 8)", []).unwrap();
            // A queue entry with no stored match (still pending) is kept.
            tx.execute("INSERT INTO match_queue VALUES ('pending', 'americas', 'na1', 8, 0)", []).unwrap();
            tx.commit().unwrap();
        }
        let newer: Vec<Vec<i64>> = patches[2..].iter().map(|p| raw_rows(&conn, p)).collect();
        assert_eq!(newer[0], vec![60, 480, 3840, 3840, 480, 60]);

        let pruned = prune_old_patches(&mut conn, 3).unwrap();
        assert_eq!(pruned, Pruned { patches: vec!["90.2".into(), "90.1".into()], matches: 120, participants: 960 });
        for old in &patches[..2] {
            assert_eq!(raw_rows(&conn, old), vec![0; 6], "{old}: raw rows are gone");
            let stats: i64 = conn
                .query_row("SELECT (SELECT COUNT(*) FROM stat_meta WHERE patch = ?1) + (SELECT COUNT(*) FROM stat_units WHERE patch = ?1)", [old], |r| r.get(0))
                .unwrap();
            assert_eq!(stats, 2, "{old}: stat rows are kept");
        }
        let after: Vec<Vec<i64>> = patches[2..].iter().map(|p| raw_rows(&conn, p)).collect();
        assert_eq!(after, newer, "the newest three patches are untouched");
        let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(one("SELECT COUNT(*) FROM matches WHERE patch = 'unknown'"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM match_queue WHERE match_id = 'pending'"), 1);
        // No orphans: every raw row still belongs to a stored participant and match.
        assert_eq!(one("SELECT COUNT(*) FROM participants WHERE match_id NOT IN (SELECT match_id FROM matches)"), 0);
        for t in ["units", "unit_items", "traits"] {
            assert_eq!(one(&format!("SELECT COUNT(*) FROM {t} WHERE participant_id NOT IN (SELECT id FROM participants)")), 0, "{t}");
        }
        // Pruning again finds nothing to do.
        assert_eq!(prune_old_patches(&mut conn, 3).unwrap(), Pruned::default());

        // Compaction: the file on disk (database plus write-ahead log) gets smaller.
        let on_disk = || -> u64 {
            ["t.db", "t.db-wal"].iter().map(|f| std::fs::metadata(dir.join(f)).map(|m| m.len()).unwrap_or(0)).sum()
        };
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
        let disk_before = on_disk();
        let c = compact(&conn).unwrap();
        assert!(c.after < c.before, "{c:?}");
        assert!(on_disk() < disk_before, "file {} bytes before, {} after", disk_before, on_disk());
        assert!((c.after as f64) < (c.before as f64) * 0.75, "two of five patches gone should free over a quarter: {c:?}");
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
