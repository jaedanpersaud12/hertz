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
                set_watermark(&tx, patch, &scope)?;
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

/// How many patches a rebuild covers when nobody says, as `npm run stats`.
pub const DEFAULT_PATCHES: i64 = 2;

/// The `stat_watermark` key for one patch and scope, e.g. "16.22/PLATINUM+@3d".
pub fn watermark_key(patch: &str, scope: &str) -> String {
    format!("{patch}/{scope}")
}

/// Records that this scope now holds every match stored so far (inside the caller's transaction).
fn set_watermark(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO stat_watermark (scope, last_rowid, built_at)
         VALUES (?, (SELECT COALESCE(MAX(rowid), 0) FROM matches), ?)",
        params![watermark_key(patch, scope), now_ms()],
    )?;
    Ok(())
}

/// Brings the stats for the [`DEFAULT_PATCHES`] most recent patches up to date, touching only scopes with new
/// matches. Returns the scopes it rebuilt, as [`watermark_key`]s.
pub fn build_stats_incremental(conn: &mut Connection) -> Result<Vec<String>> {
    build_stats_incremental_with(conn, DEFAULT_PATCHES, &mut |s| println!("{s}"))
}

/// [`build_stats_incremental`] for the `patch_count` most recent patches, with the progress lines sent to `log`.
///
/// Per scope, the watermark says which match rows (by rowid) the stored stats already hold. The stored stats are
/// trusted only if they count exactly the matches at or below the watermark; anything else (no watermark yet,
/// stat tables dropped by an upgrade, matches relabelled into the patch, a 3-day window that matches aged out
/// of) gets a full rebuild of that scope. Otherwise:
/// - nothing newer than the watermark: the scope is left alone, not rewritten;
/// - whole patch: the newer boards are added to the running sums, the derived columns are worked out again,
///   and the comp tables are recomputed from every board in the scope (they depend on the whole scope);
/// - last 3 days: recomputed from raw rows, because matches fall out of it.
///
/// The result equals a full rebuild ([`build_stats`]) row for row.
pub fn build_stats_incremental_with(
    conn: &mut Connection,
    patch_count: i64,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<String>> {
    let patches: Vec<String> = conn
        .prepare(
            "SELECT patch FROM matches WHERE patch != 'unknown' GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT ?",
        )?
        .query_map([patch_count], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if patches.is_empty() {
        log("No matches stored yet. Run \"npm run crawl\" first.");
        return Ok(Vec::new());
    }

    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT);
         CREATE TEMP TABLE IF NOT EXISTS topc (comp_key TEXT PRIMARY KEY) WITHOUT ROWID;",
    )?;
    let mut touched = Vec::new();
    for patch in &patches {
        for bucket in &BUCKETS {
            for window in &WINDOWS {
                let scope = scope_key(bucket.key, window.key);
                let since = window.days.map_or(0, |d| now_ms() - d * 86_400_000);
                let t0 = Instant::now();
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let how = match plan_scope(&tx, patch, &scope, bucket.min, since)? {
                    Plan::Skip => continue, // dropping tx writes nothing
                    Plan::Full => {
                        build_scope(&tx, patch, &scope, bucket.min, since)?;
                        "full"
                    }
                    Plan::Add(after) => {
                        add_to_scope(&tx, patch, &scope, bucket.min, after)?;
                        "added"
                    }
                };
                set_watermark(&tx, patch, &scope)?;
                tx.commit()?;
                let matches: i64 = conn.query_row(
                    "SELECT matches FROM stat_meta WHERE patch = ? AND bucket = ?",
                    params![patch, scope],
                    |r| r.get(0),
                )?;
                log(&format!(
                    "patch {patch}  {scope:<13} {matches:>8} matches  {how:<5}  {:.1} s",
                    t0.elapsed().as_secs_f64()
                ));
                touched.push(watermark_key(patch, &scope));
            }
        }
    }
    conn.execute_batch("DROP TABLE IF EXISTS p; DROP TABLE IF EXISTS topc")?;
    if touched.is_empty() {
        log("Stats are up to date: no new matches.");
    }
    Ok(touched)
}

enum Plan {
    Skip,
    Full,
    /// Add the matches with a rowid above this one.
    Add(i64),
}

fn plan_scope(db: &Connection, patch: &str, scope: &str, min_rank: i64, since: i64) -> Result<Plan> {
    use rusqlite::OptionalExtension;
    let mark: Option<i64> = db
        .query_row("SELECT last_rowid FROM stat_watermark WHERE scope = ?", [watermark_key(patch, scope)], |r| r.get(0))
        .optional()?
        .flatten();
    let held: Option<i64> = db
        .query_row("SELECT matches FROM stat_meta WHERE patch = ? AND bucket = ?", params![patch, scope], |r| r.get(0))
        .optional()?;
    let (Some(mark), Some(held)) = (mark, held) else { return Ok(Plan::Full) };
    let (old, new): (i64, i64) = db.query_row(
        "SELECT COALESCE(SUM(rowid <= ?4), 0), COALESCE(SUM(rowid > ?4), 0) FROM matches
         WHERE patch = ?1 AND rank >= ?2 AND game_datetime >= ?3",
        params![patch, min_rank, since, mark],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(if old != held {
        Plan::Full
    } else if new == 0 {
        Plan::Skip
    } else if since != 0 {
        Plan::Full // the 3-day window is recomputed from raw rows
    } else {
        Plan::Add(mark)
    })
}

/// The rate tables [`build_rates`] writes, with the columns that are not summed: their key, and for traits the
/// smallest unit count. Everything else (games, sum_place, wins, top4) adds up.
const SUM_TABLES: [(&str, &str); 5] = [
    ("stat_unit_dist", "character_id, star, n_items"),
    ("stat_traits", "name, tier_current, num_units"),
    ("stat_unit_items", "character_id, item"),
    ("stat_items", "item"),
    ("stat_unit_builds", "character_id, build"),
];

/// Adds the boards from matches with a rowid above `after` to a whole-patch scope, inside the caller's transaction.
fn add_to_scope(db: &Connection, patch: &str, scope: &str, min_rank: i64, after: i64) -> Result<()> {
    let k = params![patch, scope];
    // The new boards' sums are written by the same SQL a full rebuild uses, under a scratch bucket that only
    // exists inside this transaction, then folded into the scope.
    let scratch = format!("{scope}#new");
    let ks = params![patch, scratch];
    db.execute_batch("DELETE FROM p; DELETE FROM topc")?;
    db.execute(
        "INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
         JOIN matches m ON m.match_id = pa.match_id
         WHERE m.patch = ? AND m.rank >= ? AND m.rowid > ?",
        params![patch, min_rank, after],
    )?;
    let new_comps: i64 = db.query_row("SELECT COUNT(*) n FROM p", [], |r| r.get(0))?;
    let new_matches: i64 = db.query_row(
        "SELECT COUNT(*) n FROM matches WHERE patch = ? AND rank >= ? AND rowid > ?",
        params![patch, min_rank, after],
        |r| r.get(0),
    )?;
    db.execute(
        "UPDATE stat_meta SET matches = matches + ?, comps = comps + ?, built_at = ? WHERE patch = ? AND bucket = ?",
        params![new_matches, new_comps, now_ms(), patch, scope],
    )?;
    for table in STAT_TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), ks)?;
    }
    build_rates(db, patch, &scratch)?;
    for (table, keys) in SUM_TABLES {
        let cols = format!("{keys}, games, sum_place, wins, top4");
        let min = if table == "stat_traits" { ", num_units = MIN(num_units, excluded.num_units)" } else { "" };
        db.execute(
            &format!(
                "INSERT INTO {table} (patch, bucket, {cols}) SELECT patch, ?3, {cols} FROM {table} WHERE patch = ?1 AND bucket = ?2
                 ON CONFLICT DO UPDATE SET games = games + excluded.games, sum_place = sum_place + excluded.sum_place,
                   wins = wins + excluded.wins, top4 = top4 + excluded.top4{min}"
            ),
            params![patch, scratch, scope],
        )?;
    }
    for table in STAT_TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), ks)?;
    }

    // Derived rows and columns, from the new sums.
    db.execute("DELETE FROM stat_units WHERE patch = ? AND bucket = ?", k)?;
    db.execute("DELETE FROM stat_traits WHERE patch = ? AND bucket = ? AND tier_current = 0", k)?;
    build_derived(db, patch, scope)?;

    // Comps, from every board in the scope.
    for table in ["stat_comps", "stat_comp_units", "stat_comp_items", "stat_comp_traits", "stat_comp_levels"] {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), k)?;
    }
    db.execute_batch("DELETE FROM p")?;
    db.execute(
        "INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
         JOIN matches m ON m.match_id = pa.match_id
         WHERE m.patch = ? AND m.rank >= ? AND m.game_datetime >= 0",
        params![patch, min_rank],
    )?;
    let comps: i64 = db.query_row("SELECT comps FROM stat_meta WHERE patch = ? AND bucket = ?", k, |r| r.get(0))?;
    if comps > 0 {
        build_comps(db, patch, scope, comps)?;
    }
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

    build_rates(db, patch, scope)?;
    build_derived(db, patch, scope)?;
    build_comps(db, patch, scope, comps)
}

/// The per-board counts for units, traits and items: plain sums over the boards in p, so they can be added up.
fn build_rates(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    let k = params![patch, scope];
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

    // ---- Traits ---- per activation level.
    db.execute(
        "INSERT INTO stat_traits
         SELECT ?, ?, t.name, t.tier_current, MIN(t.num_units), COUNT(*), SUM(p.placement), SUM(p.placement = 1), SUM(p.placement <= 4)
         FROM traits t JOIN p ON p.id = t.participant_id GROUP BY t.name, t.tier_current",
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
    Ok(())
}

/// Rows and columns worked out from the sums in [`build_rates`]: unit totals, "trait active at any level"
/// and place change. Reads the stat tables only, so it gives the same answer however the sums were reached.
fn build_derived(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    let k = params![patch, scope];
    db.execute(
        "INSERT INTO stat_units
         SELECT patch, bucket, character_id, SUM(games), SUM(sum_place), SUM(wins), SUM(top4), SUM(star * games) * 1.0 / SUM(games)
         FROM stat_unit_dist WHERE patch = ? AND bucket = ? GROUP BY character_id",
        k,
    )?;
    // One row (tier_current = 0) for "active at any level".
    db.execute(
        "INSERT INTO stat_traits
         SELECT patch, bucket, name, 0, MIN(num_units), SUM(games), SUM(sum_place), SUM(wins), SUM(top4)
         FROM stat_traits WHERE patch = ? AND bucket = ? GROUP BY name",
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
    Ok(())
}

/// The comp tables for every board in p. These depend on the whole scope (look-alike merging, the share a comp
/// needs, the top 120), so they are never added up: an incremental build recomputes them for each scope it touches.
fn build_comps(db: &Connection, patch: &str, scope: &str, comps: i64) -> Result<()> {
    let k = params![patch, scope];
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

/// Incremental rebuilds (PRD 1 E5), checked against full rebuilds on synthetic matches.
#[cfg(test)]
mod incremental_tests {
    use super::*;
    use tft_core::patches::{Calendar, PatchStart};
    use tft_core::riot::{Info, Metadata, Participant, RiotMatch, Trait, Unit};
    use tft_store::Ingest;

    const DAY: i64 = 86_400_000;

    /// A small deterministic random source (xorshift), so every run sees the same matches.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> i64 {
            (self.next() % n) as i64
        }
    }

    /// One synthetic lobby. Boards follow 30 archetypes (skewed so a few are popular), and which of two carries
    /// holds the items varies, so look-alike labels exist for the comp merging to fold.
    /// `floor` is the fewest units the main trait can have.
    fn lobby(rng: &mut Rng, id: &str, at: i64, floor: i64) -> RiotMatch {
        let mut places: Vec<i64> = (1..=8).collect();
        for i in (1..8).rev() {
            places.swap(i, rng.below(i as u64 + 1) as usize);
        }
        let participants = places
            .iter()
            .enumerate()
            .map(|(seat, &placement)| {
                let a = (rng.below(30) * rng.below(30)) / 30; // 0..29, low numbers more common
                let size = 6 + rng.below(4) as usize;
                let carry = rng.below(2) as usize;
                let units = (0..size)
                    .map(|j| {
                        let unit = if j == size - 1 && rng.below(3) == 0 { rng.below(60) } else { (a * 7 + j as i64 * 3) % 60 };
                        let n_items = if j == carry { 3 } else if j < 4 { rng.below(3) } else { 0 };
                        Unit {
                            character_id: format!("U{unit:02}"),
                            tier: 1 + rng.below(3),
                            rarity: Some(unit % 7),
                            item_names: Some((0..n_items).map(|_| format!("I{:02}", (a * 3 + rng.below(12)) % 40)).collect()),
                        }
                    })
                    .collect();
                let traits = (0..3)
                    .map(|t| {
                        let n = if t == 0 { floor + rng.below(4) } else { floor - 1 + rng.below(4) };
                        Trait {
                            name: format!("T{:02}", (a + t * 7) % 20),
                            num_units: n,
                            style: 1 + rng.below(4),
                            tier_current: rng.below(4), // 0 is inactive, dropped at ingest
                        }
                    })
                    .collect();
                Participant {
                    puuid: format!("{id}-{seat}"),
                    riot_id_game_name: None,
                    riot_id_tagline: None,
                    placement,
                    level: Some(6 + rng.below(5)),
                    last_round: Some(20 + rng.below(20)),
                    gold_left: Some(rng.below(30)),
                    total_damage_to_players: Some(rng.below(150)),
                    traits,
                    units,
                }
            })
            .collect();
        RiotMatch {
            metadata: Metadata { match_id: id.to_string(), participants: Vec::new() },
            info: Info {
                game_datetime: at,
                game_length: Some(2000.0),
                game_version: None,
                queue_id: Some(1100),
                queue_id_camel: None,
                tft_set_number: Some(99),
                participants,
            },
        }
    }

    /// A match to store: id, rank of the player it was found through, time played.
    type Game = (String, i64, i64);

    fn store(conn: &mut Connection, cal: &Calendar, seed: u64, games: &[Game]) {
        store_with(conn, cal, seed, games, 5);
    }

    fn store_with(conn: &mut Connection, cal: &Calendar, seed: u64, games: &[Game], floor: i64) {
        let mut rng = Rng(seed);
        for (id, rank, at) in games {
            let ing = Ingest { platform: "na1", rank: *rank, tier: tft_core::config::tier_name(*rank), snowball: false };
            assert!(ing.store(conn, &lobby(&mut rng, id, *at, floor), cal).unwrap());
        }
    }

    fn fresh_db(name: &str) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-stats-inc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        (dir, conn)
    }

    /// Every stat table in `a` equals the one in the database at `b`, row for row and value for value
    /// (stat_meta.built_at aside). Returns the number of rows compared.
    fn assert_same_stats(a: &Connection, b: &std::path::Path) -> i64 {
        a.execute("ATTACH DATABASE ? AS fresh", [b.to_str().unwrap()]).unwrap();
        let mut rows = 0;
        for table in STAT_TABLES {
            let cols = if table == "stat_meta" { "patch, bucket, matches, comps".to_string() } else { "*".to_string() };
            let count = |sql: &str| -> i64 { a.query_row(sql, [], |r| r.get(0)).unwrap() };
            let n = count(&format!("SELECT COUNT(*) FROM main.{table}"));
            assert_eq!(n, count(&format!("SELECT COUNT(*) FROM fresh.{table}")), "{table}: row count");
            let only_a = count(&format!("SELECT COUNT(*) FROM (SELECT {cols} FROM main.{table} EXCEPT SELECT {cols} FROM fresh.{table})"));
            let only_b = count(&format!("SELECT COUNT(*) FROM (SELECT {cols} FROM fresh.{table} EXCEPT SELECT {cols} FROM main.{table})"));
            assert_eq!((only_a, only_b), (0, 0), "{table}: rows differ from a full rebuild");
            rows += n;
        }
        a.execute("DETACH DATABASE fresh", []).unwrap();
        rows
    }

    /// built_at from stat_meta and stat_watermark, per scope.
    fn built_at(conn: &Connection) -> HashMap<String, (i64, i64)> {
        conn.prepare(
            "SELECT w.scope, m.built_at, w.built_at FROM stat_watermark w
             JOIN stat_meta m ON w.scope = m.patch || '/' || m.bucket",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    #[test]
    fn incremental_equals_a_full_rebuild_and_leaves_quiet_scopes_alone() {
        let now = now_ms();
        let cal = Calendar(vec![
            PatchStart { patch: "90.1".into(), since: 0 },
            PatchStart { patch: "90.2".into(), since: now - 8 * DAY },
        ]);
        let ranks = [0, 3, 4, 5, 6, 7, 9];
        // 90.1 is old; 90.2 runs from 7 days ago to now, so some of it is in the 3-day window.
        let mut first: Vec<Game> = (0..120).map(|i| (format!("A{i}"), ranks[i % 7], now - 20 * DAY + i as i64 * 60_000)).collect();
        first.extend((0..240).map(|i| (format!("B{i}"), ranks[i % 7], now - 7 * DAY + i as i64 * 2_000_000)));
        // New matches: patch 90.2 only, found through Platinum and Emerald players only, on both sides of 3 days.
        let more: Vec<Game> =
            (0..200).map(|i| (format!("C{i}"), 4 + (i % 2) as i64, now - 6 * DAY + i as i64 * 2_500_000)).collect();

        let (dir, mut conn) = fresh_db("main");
        tft_store::save_calendar(&conn, &cal).unwrap();
        store(&mut conn, &cal, 1, &first);
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let before = built_at(&conn);
        assert_eq!(before.len(), 16, "a full rebuild records a watermark for each of the 16 scopes");
        std::thread::sleep(std::time::Duration::from_millis(5));

        // The new boards can have fewer units in their main trait than any before, so the smallest-unit-count
        // column only comes out right if it is combined, not just kept or overwritten.
        store_with(&mut conn, &cal, 2, &more, 2);
        let mut lines = Vec::new();
        let touched = build_stats_incremental_with(&mut conn, 2, &mut |s| lines.push(s.to_string())).unwrap();
        assert_eq!(touched, ["90.2/ALL", "90.2/ALL@3d", "90.2/PLATINUM+", "90.2/PLATINUM+@3d"]);
        // The whole-patch scopes were added to, not rebuilt; the 3-day ones were recomputed.
        let how = |scope: &str| lines.iter().find(|l| l.contains(&format!(" {scope} "))).unwrap().clone();
        assert!(how("ALL").contains("added") && how("PLATINUM+").contains("added"), "{lines:?}");
        assert!(how("ALL@3d").contains("full"), "{lines:?}");

        let after = built_at(&conn);
        for (scope, stamps) in &before {
            if touched.contains(scope) {
                assert!(after[scope].0 > stamps.0 && after[scope].1 > stamps.1, "{scope} was rebuilt");
            } else {
                assert_eq!(&after[scope], stamps, "{scope} has no new matches and must not be rewritten");
            }
        }

        // The same matches, stored in the same order, through a full rebuild.
        let (fresh_dir, mut fresh) = fresh_db("fresh");
        store(&mut fresh, &cal, 1, &first);
        store_with(&mut fresh, &cal, 2, &more, 2);
        build_stats_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        let compared = assert_same_stats(&conn, &fresh_dir.join("t.db"));
        assert!(compared > 1000, "only {compared} rows compared");
        let comps: i64 = conn.query_row("SELECT COUNT(*) FROM stat_comps", [], |r| r.get(0)).unwrap();
        assert!(comps > 20, "the synthetic matches should make comps ({comps})");

        // Nothing new: nothing touched.
        assert!(build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap().is_empty());
        assert_eq!(built_at(&conn), after);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&fresh_dir).unwrap();
    }

    #[test]
    fn stats_it_cannot_trust_get_a_full_rebuild() {
        let now = now_ms();
        let cal = Calendar(vec![PatchStart { patch: "90.1".into(), since: 0 }]);
        let games: Vec<Game> = (0..80).map(|i| (format!("A{i}"), (i % 10) as i64, now - 2 * DAY + i as i64 * 60_000)).collect();
        let (dir, mut conn) = fresh_db("untrusted");
        store(&mut conn, &cal, 7, &games);

        // No stats or watermarks yet (a new database, or the TypeScript engine built the stats): every scope is built.
        let touched = build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(touched.len(), 8);
        // Stats edited behind the watermark's back (here: a stored match moved into the patch from elsewhere,
        // as relabelling does) no longer count the matches the watermark claims, so that scope is rebuilt.
        conn.execute("UPDATE matches SET patch = 'unknown' WHERE match_id = 'A0'", []).unwrap();
        let mut lines = Vec::new();
        let touched = build_stats_incremental_with(&mut conn, 2, &mut |s| lines.push(s.to_string())).unwrap();
        assert!(touched.contains(&"90.1/ALL".to_string()), "{touched:?}");
        assert!(lines.iter().all(|l| l.contains("full")), "{lines:?}");

        let (fresh_dir, mut fresh) = fresh_db("untrusted-fresh");
        store(&mut fresh, &cal, 7, &games);
        fresh.execute("UPDATE matches SET patch = 'unknown' WHERE match_id = 'A0'", []).unwrap();
        build_stats_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        assert_same_stats(&conn, &fresh_dir.join("t.db"));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&fresh_dir).unwrap();
    }

    /// PRD 1 E5: 50,000 new matches take under 60 seconds incrementally. Slow to set up (it stores every match
    /// through the real ingest), so it runs on demand:
    /// `cargo test --release -p tft-stats -- --ignored --nocapture fifty_thousand`.
    /// `HERTZ_BENCH_BASE` sets how many matches are already in the scopes (default 50,000).
    #[test]
    #[ignore]
    fn fifty_thousand_new_matches_take_under_a_minute() {
        let base: usize = std::env::var("HERTZ_BENCH_BASE").ok().and_then(|v| v.parse().ok()).unwrap_or(50_000);
        let now = now_ms();
        let cal = Calendar(vec![PatchStart { patch: "90.1".into(), since: 0 }]);
        let ranks = [3, 4, 5, 6, 7, 8, 9];
        // Everything in one recent patch, across the last 6 days, so every scope (3-day ones included) is touched.
        let game = |i: usize, n: usize| (format!("M{i}"), ranks[i % 7], now - 6 * DAY + (i * (6 * DAY as usize) / n) as i64);
        let n = base + 50_000;
        let (dir, mut conn) = fresh_db("bench");
        let t = Instant::now();
        store(&mut conn, &cal, 3, &(0..base).map(|i| game(i, n)).collect::<Vec<_>>());
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let full_base = t.elapsed();
        store(&mut conn, &cal, 4, &(base..n).map(|i| game(i, n)).collect::<Vec<_>>());

        let t = Instant::now();
        let mut lines = Vec::new();
        let touched = build_stats_incremental_with(&mut conn, 2, &mut |s| lines.push(s.to_string())).unwrap();
        let took = t.elapsed();
        let t = Instant::now();
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let full_all = t.elapsed();
        println!("{}", lines.join("\n"));
        println!(
            "base {base} matches (stored + full build {:.1} s); 50,000 new: incremental {:.1} s for {} scopes; full rebuild of all {n} afterwards {:.1} s",
            full_base.as_secs_f64(),
            took.as_secs_f64(),
            touched.len(),
            full_all.as_secs_f64()
        );
        assert_eq!(touched.len(), 8);
        assert!(took.as_secs() < 60, "incremental build took {:.1} s", took.as_secs_f64());
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
