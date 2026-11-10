//! Turns stored matches into the stat tables the app reads. A port of `src/stats/build.ts`: the same SQL,
//! in the same order, so the numbers match exactly (checked by `npm run parity`).

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tft_core::config::{BUCKETS, scope_key};

/// A comp needs this share of all boards (and at least this many) to get its own row.
pub const COMP_MIN_SHARE: f64 = 0.002;
pub const COMP_MIN_GAMES: i64 = 5;
pub const COMP_LIMIT: i64 = 120;

/// Time windows inside a patch, as `WINDOWS` in `src/config.ts`. An empty key is the whole patch.
pub struct Window {
    pub key: &'static str,
    pub label: &'static str,
    pub days: Option<i64>,
}
pub const WINDOWS: [Window; 2] = [
    Window { key: "", label: "Whole patch", days: None },
    Window { key: "3d", label: "Last 3 days", days: Some(3) },
];

/// The stat tables a rebuild writes, in the order `STAT_TABLES` in `src/db.ts` lists them.
const STAT_TABLES: [&str; 12] = [
    "stat_meta", "stat_units", "stat_unit_dist", "stat_traits", "stat_items", "stat_unit_items", "stat_unit_builds",
    "stat_comps", "stat_comp_units", "stat_comp_items", "stat_comp_traits", "stat_comp_levels",
];

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Rebuilds the stats for the `patch_count` most recent patches, printing one line per scope.
pub fn build_stats(conn: &mut Connection, patch_count: i64) -> Result<()> {
    build_stats_with(conn, patch_count, &mut |s| println!("{s}"))
}

/// [`build_stats`] with the progress lines sent to `log`.
pub fn build_stats_with(conn: &mut Connection, patch_count: i64, log: &mut dyn FnMut(&str)) -> Result<()> {
    let patches: Vec<String> = conn
        .prepare(
            "SELECT patch FROM matches WHERE patch != 'unknown' GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT ?",
        )?
        .query_map([patch_count], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    if patches.is_empty() {
        log("No matches stored yet. Run \"npm run crawl\" first.");
        return Ok(());
    }

    let unlabelled: i64 = conn.query_row("SELECT COUNT(*) n FROM participants WHERE comp_key IS NULL", [], |r| r.get(0))?;
    let all: i64 = conn.query_row("SELECT COUNT(*) n FROM participants", [], |r| r.get(0))?;
    if all != 0 && (unlabelled as f64) > (all as f64) * 0.5 {
        log("Most boards have no comp label yet. Run \"npm run recluster\" once, then \"npm run stats\" again.");
    }

    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT);
         CREATE TEMP TABLE IF NOT EXISTS topc (comp_key TEXT PRIMARY KEY) WITHOUT ROWID;",
    )?;

    for patch in &patches {
        for bucket in &BUCKETS {
            for window in &WINDOWS {
                let scope = scope_key(bucket.key, window.key);
                let since = window.days.map_or(0, |d| now_ms() - d * 86_400_000);
                let t0 = Instant::now();
                // Take the write lock up front, so a crawler writing at the same time makes this wait instead of fail.
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                build_scope(&tx, patch, &scope, bucket.min, since)?;
                tx.commit()?;

                let (matches, comps): (i64, i64) = conn.query_row(
                    "SELECT matches, comps FROM stat_meta WHERE patch = ? AND bucket = ?",
                    params![patch, scope],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                log(&format!(
                    "patch {patch}  {scope:<13} {matches:>8} matches {comps:>9} comps  {:.1} s",
                    t0.elapsed().as_secs_f64()
                ));
            }
        }
    }
    conn.execute_batch("DROP TABLE IF EXISTS p; DROP TABLE IF EXISTS topc")?;
    conn.execute_batch("PRAGMA optimize")?;
    Ok(())
}

/// One patch and scope, inside the caller's transaction.
fn build_scope(db: &Connection, patch: &str, scope: &str, min_rank: i64, since: i64) -> Result<()> {
    let k = params![patch, scope];
    db.execute_batch("DELETE FROM p; DELETE FROM topc")?;
    db.execute(
        "INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
         JOIN matches m ON m.match_id = pa.match_id
         WHERE m.patch = ? AND m.rank >= ? AND m.game_datetime >= ?",
        params![patch, min_rank, since],
    )?;

    for table in STAT_TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), k)?;
    }

    let comps: i64 = db.query_row("SELECT COUNT(*) n FROM p", [], |r| r.get(0))?;
    let matches: i64 = db.query_row(
        "SELECT COUNT(*) n FROM matches WHERE patch = ? AND rank >= ? AND game_datetime >= ?",
        params![patch, min_rank, since],
        |r| r.get(0),
    )?;
    db.execute("INSERT INTO stat_meta VALUES (?,?,?,?,?)", params![patch, scope, matches, comps, now_ms()])?;
    if comps == 0 {
        return Ok(());
    }

    // ---- Units ----
    // One row per unit, star level and item count. A board with two copies of a unit counts once.
    db.execute(
        "INSERT INTO stat_unit_dist
         SELECT ?, ?, character_id, star, n_items, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4)
         FROM (SELECT u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, p.placement
               FROM units u JOIN p ON p.id = u.participant_id GROUP BY u.participant_id, u.character_id)
         GROUP BY character_id, star, n_items",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_units
         SELECT patch, bucket, character_id, SUM(games), SUM(sum_place), SUM(wins), SUM(top4), SUM(star * games) * 1.0 / SUM(games)
         FROM stat_unit_dist WHERE patch = ? AND bucket = ? GROUP BY character_id",
        k,
    )?;

    // ---- Traits ---- per activation level, then one row (tier_current = 0) for "active at any level".
    db.execute(
        "INSERT INTO stat_traits
         SELECT ?, ?, t.name, t.tier_current, MIN(t.num_units), COUNT(*), SUM(p.placement), SUM(p.placement = 1), SUM(p.placement <= 4)
         FROM traits t JOIN p ON p.id = t.participant_id GROUP BY t.name, t.tier_current",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_traits
         SELECT patch, bucket, name, 0, MIN(num_units), SUM(games), SUM(sum_place), SUM(wins), SUM(top4)
         FROM stat_traits WHERE patch = ? AND bucket = ? GROUP BY name",
        k,
    )?;

    // ---- Items ----
    db.execute(
        "INSERT INTO stat_unit_items
         SELECT ?, ?, character_id, item, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), NULL
         FROM (SELECT DISTINCT ui.participant_id, ui.character_id, ui.item, p.placement
               FROM unit_items ui JOIN p ON p.id = ui.participant_id)
         GROUP BY character_id, item",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_items
         SELECT ?, ?, item, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), NULL
         FROM (SELECT DISTINCT ui.participant_id, ui.item, p.placement
               FROM unit_items ui JOIN p ON p.id = ui.participant_id)
         GROUP BY item",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_unit_builds
         SELECT ?, ?, u.character_id, u.build, COUNT(*), SUM(p.placement), SUM(p.placement = 1), SUM(p.placement <= 4), NULL
         FROM units u JOIN p ON p.id = u.participant_id WHERE u.build IS NOT NULL
         GROUP BY u.character_id, u.build",
        k,
    )?;

    // Place change: how the unit places with this item (or build) compared with the unit overall.
    // Negative is better. This removes "good units make their items look good".
    for table in ["stat_unit_items", "stat_unit_builds"] {
        db.execute(
            &format!(
                "UPDATE {table} SET place_change = sum_place * 1.0 / games - (
                   SELECT su.sum_place * 1.0 / su.games FROM stat_units su
                   WHERE su.patch = {table}.patch AND su.bucket = {table}.bucket AND su.character_id = {table}.character_id)
                 WHERE patch = ? AND bucket = ?"
            ),
            k,
        )?;
    }
    // For an item overall: the average of its per-unit place changes, weighted by how often each unit holds it.
    db.execute(
        "UPDATE stat_items SET place_change = (
           SELECT SUM(sui.place_change * sui.games) / SUM(sui.games) FROM stat_unit_items sui
           WHERE sui.patch = stat_items.patch AND sui.bucket = stat_items.bucket AND sui.item = stat_items.item)
         WHERE patch = ? AND bucket = ?",
        k,
    )?;

    // ---- Comps ----
    let min_games = COMP_MIN_GAMES.max(((comps as f64) * COMP_MIN_SHARE).ceil() as i64);
    merge_lookalikes(db, min_games)?;
    db.execute(
        "INSERT INTO stat_comps
         SELECT ?, ?, comp_key, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), SUM(COALESCE(level, 0))
         FROM p WHERE comp_key IS NOT NULL GROUP BY comp_key HAVING COUNT(*) >= ?
         ORDER BY COUNT(*) DESC LIMIT ?",
        params![patch, scope, min_games, COMP_LIMIT],
    )?;
    db.execute("INSERT INTO topc SELECT comp_key FROM stat_comps WHERE patch = ? AND bucket = ?", k)?;

    db.execute(
        "INSERT INTO stat_comp_units
         SELECT ?, ?, comp_key, character_id, COUNT(*), SUM(star), SUM(n_items), SUM(n_items = 3), SUM(placement)
         FROM (SELECT p.comp_key, u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, p.placement
               FROM p JOIN topc USING (comp_key) JOIN units u ON u.participant_id = p.id
               GROUP BY p.id, u.character_id)
         GROUP BY comp_key, character_id",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_comp_items
         SELECT ?, ?, comp_key, character_id, item, COUNT(*), SUM(placement)
         FROM (SELECT DISTINCT p.id, p.comp_key, ui.character_id, ui.item, p.placement
               FROM p JOIN topc USING (comp_key) JOIN unit_items ui ON ui.participant_id = p.id)
         GROUP BY comp_key, character_id, item",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_comp_traits
         SELECT ?, ?, p.comp_key, t.name, t.tier_current, MIN(t.num_units), MAX(t.style), COUNT(*)
         FROM p JOIN topc USING (comp_key) JOIN traits t ON t.participant_id = p.id
         GROUP BY p.comp_key, t.name, t.tier_current",
        k,
    )?;
    db.execute(
        "INSERT INTO stat_comp_levels
         SELECT ?, ?, p.comp_key, COALESCE(p.level, 0), COUNT(*), SUM(p.placement)
         FROM p JOIN topc USING (comp_key) GROUP BY p.comp_key, COALESCE(p.level, 0)",
        k,
    )?;
    Ok(())
}

/// The comp label can split one real comp in two, for example when a comp has two carries and either can end
/// up holding more items. Labels whose usual boards are nearly the same get folded into the more common one.
/// Works on the temporary table p only; stored labels are untouched.
fn merge_lookalikes(db: &Connection, min_games: i64) -> Result<()> {
    let keys: Vec<(String, i64)> = db
        .prepare(
            "SELECT comp_key, COUNT(*) games FROM p WHERE comp_key IS NOT NULL
             GROUP BY comp_key HAVING COUNT(*) >= ? ORDER BY games DESC LIMIT 400",
        )?
        .query_map([3.max(min_games / 4)], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if keys.len() < 2 {
        return Ok(());
    }

    db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS cand (comp_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM cand")?;
    {
        let mut add = db.prepare("INSERT INTO cand VALUES (?)")?;
        for (key, _) in &keys {
            add.execute([key])?;
        }
    }
    let games: HashMap<&str, i64> = keys.iter().map(|(k, g)| (k.as_str(), *g)).collect();
    let mut core: HashMap<String, HashSet<String>> = HashMap::new();
    {
        let mut freq = db.prepare(
            "SELECT comp_key, character_id, COUNT(*) games
             FROM (SELECT DISTINCT p.id, p.comp_key, u.character_id FROM p JOIN cand USING (comp_key) JOIN units u ON u.participant_id = p.id)
             GROUP BY comp_key, character_id",
        )?;
        let mut rows = freq.query([])?;
        while let Some(r) = rows.next()? {
            let (key, unit, n): (String, String, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            if (n as f64) / (games[key.as_str()] as f64) < 0.5 {
                continue;
            }
            core.entry(key).or_default().insert(unit);
        }
    }

    // Biggest first, so smaller look-alikes fold into the label most people ended up with.
    let empty = HashSet::new();
    let mut kept: Vec<&str> = Vec::new();
    let mut remap = db.prepare("UPDATE p SET comp_key = ? WHERE comp_key = ?")?;
    for (key, _) in &keys {
        let mine = core.get(key).unwrap_or(&empty);
        let twin = kept.iter().find(|other| {
            let theirs = &core[**other];
            let shared = mine.intersection(theirs).count();
            let union = mine.union(theirs).count();
            union > 0 && (shared as f64) / (union as f64) >= 0.75
        });
        if let Some(twin) = twin {
            remap.execute([*twin, key.as_str()])?;
        } else if !mine.is_empty() {
            kept.push(key);
        }
    }
    db.execute_batch("DROP TABLE IF EXISTS cand")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database with one patch: `boards` is (comp_key, placement, units) per board.
    fn world(name: &str, boards: &[(&str, i64, &[&str])]) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-stats-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        let now = now_ms();
        for (i, (comp, place, units)) in boards.iter().enumerate() {
            let m = format!("M{}", i / 8);
            conn.execute(
                "INSERT OR IGNORE INTO matches VALUES (?, 'na1', '99.1', 99, ?, 2000, 8)",
                params![m, now - 3_600_000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, 8, ?)",
                params![m, format!("P{i}"), place, comp],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for u in *units {
                conn.execute("INSERT INTO units VALUES (?, ?, 2, 0, NULL, 1)", params![id, u]).unwrap();
            }
            conn.execute("INSERT INTO traits VALUES (?, 'T', 3, 1, 1)", [id]).unwrap();
        }
        (dir, conn)
    }

    fn comps(conn: &Connection, scope: &str) -> Vec<(String, i64)> {
        conn.prepare("SELECT comp_key, games FROM stat_comps WHERE bucket = ? ORDER BY games DESC, comp_key")
            .unwrap()
            .query_map([scope], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn folds_lookalike_comps_into_the_common_one() {
        let a: &[&str] = &["U1", "U2", "U3", "U4"];
        let b: &[&str] = &["U1", "U2", "U3", "U4", "U5"]; // 4 of 5 shared: a look-alike
        let c: &[&str] = &["U6", "U7", "U8", "U9"];
        let mut boards = Vec::new();
        boards.extend((0..8).map(|i| ("X|A", 1 + i % 8, a)));
        boards.extend((0..6).map(|i| ("X|B", 1 + i % 8, b)));
        boards.extend((0..6).map(|i| ("Y|C", 1 + i % 8, c)));
        boards.extend((0..2).map(|i| ("Z|D", 1 + i, c))); // too few boards to be a merge candidate or get a row
        let (dir, mut conn) = world("merge", &boards);
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();

        // X|B folds into X|A (14 boards). Y|C keeps its row. Z|D has 2 boards: merge candidates need 3 and a row
        // needs 5, so it is neither merged nor listed.
        assert_eq!(comps(&conn, "ALL"), vec![("X|A".into(), 14), ("Y|C".into(), 6)]);
        assert_eq!(comps(&conn, "ALL@3d"), comps(&conn, "ALL"));
        // Stored labels are untouched.
        let stored: i64 =
            conn.query_row("SELECT COUNT(*) FROM participants WHERE comp_key = 'X|B'", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 6);
        let meta: (i64, i64) = conn
            .query_row("SELECT matches, comps FROM stat_meta WHERE bucket = 'ALL'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(meta, (3, 22));
        // Rank 8 matches show up in every bucket up to Master+.
        assert_eq!(comps(&conn, "MASTER+").len(), 2);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn buckets_and_window_narrow_the_matches() {
        let units: &[&str] = &["U1", "U2"];
        let boards: Vec<(&str, i64, &[&str])> = (0..40).map(|i| ("X|A", 1 + i % 8, units)).collect();
        let (dir, mut conn) = world("scopes", &boards);
        // Five matches: ranks exactly at each bucket minimum (0, 4, 6, 7) plus one below Platinum.
        // M0 and M1 are old (one just outside 3 days); M2 is just inside.
        let now = now_ms();
        for (m, rank, age_ms) in [
            ("M0", 7, 10 * 86_400_000),
            ("M1", 6, 3 * 86_400_000 + 60_000),
            ("M2", 4, 3 * 86_400_000 - 60_000),
            ("M3", 0, 3_600_000),
            ("M4", 3, 3_600_000),
        ] {
            conn.execute("UPDATE matches SET rank = ?, game_datetime = ? WHERE match_id = ?", params![rank, now - age_ms, m])
                .unwrap();
        }
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let meta = |scope: &str| -> (i64, i64) {
            conn.query_row("SELECT matches, comps FROM stat_meta WHERE bucket = ?", [scope], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        assert_eq!(meta("ALL"), (5, 40));
        assert_eq!(meta("PLATINUM+"), (3, 24));
        assert_eq!(meta("DIAMOND+"), (2, 16));
        assert_eq!(meta("MASTER+"), (1, 8));
        assert_eq!(meta("ALL@3d"), (3, 24));
        assert_eq!(meta("PLATINUM+@3d"), (1, 8));
        assert_eq!(meta("DIAMOND+@3d"), (0, 0));
        assert_eq!(meta("MASTER+@3d"), (0, 0));
        // An empty scope writes its meta row and nothing else.
        let units_rows: i64 =
            conn.query_row("SELECT COUNT(*) FROM stat_units WHERE bucket = 'MASTER+@3d'", [], |r| r.get(0)).unwrap();
        assert_eq!(units_rows, 0);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn says_so_when_nothing_is_stored() {
        let (dir, mut conn) = world("empty", &[]);
        let mut lines = Vec::new();
        build_stats_with(&mut conn, 2, &mut |s| lines.push(s.to_string())).unwrap();
        assert_eq!(lines, vec!["No matches stored yet. Run \"npm run crawl\" first.".to_string()]);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
