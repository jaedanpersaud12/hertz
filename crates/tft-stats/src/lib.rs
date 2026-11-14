//! Turns stored matches into the stat tables the app reads. A port of `src/stats/build.ts`: the same SQL,
//! in the same order, so the numbers match exactly (checked by `npm run parity`).

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tft_core::config::{BUCKETS, scope_key};

pub mod tier;
pub mod comp_detail;
pub mod flex;
pub mod items_depth;
pub mod cluster;
pub mod lineage;
mod onepass;
pub mod overrides;
pub mod report;
pub mod unit_depth;
pub mod variants;
#[cfg(test)]
mod cluster_tests;
pub use cluster::{Method, comp_method, set_comp_method};

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
const STAT_TABLES: [&str; 13] = [
    "stat_meta", "stat_units", "stat_unit_dist", "stat_traits", "stat_items", "stat_unit_items", "stat_unit_builds",
    "stat_comps", "stat_comp_units", "stat_comp_items", "stat_comp_traits", "stat_comp_levels", "stat_comp_variants",
];

/// SQL that keeps only ranked matches (E12): other game modes are stored but left out of every stat. `table` is
/// the name or alias of the matches table in the query. NULL is a match stored before E12, when only ranked was
/// stored. The same text as `rankedOnly` in `src/db.ts`; the ranked queue comes from `RANKED_QUEUE_ID`, as there.
pub fn ranked_only(table: &str) -> String {
    let id = tft_core::config::Config::from_env().ranked_queue_id;
    format!("({table}.queue_id IS NULL OR {table}.queue_id = {id})")
}

/// The `patch_count` most recent patches that have ranked matches.
fn recent_patches(conn: &Connection, patch_count: i64) -> Result<Vec<String>> {
    Ok(conn
        .prepare(&format!(
            "SELECT patch FROM matches WHERE patch != 'unknown' AND {} GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT ?",
            ranked_only("matches")
        ))?
        .query_map([patch_count], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Settings for a stats build on this connection: a 256 MB page cache and up to 2 GB of the file memory-mapped,
/// instead of SQLite's 2 MB default, so the many passes over the raw rows read from memory, not the disk.
fn tune_for_build(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "cache_size", -262_144)?;
    conn.pragma_update(None, "mmap_size", 2_147_483_648i64)?;
    Ok(())
}

/// Rebuilds the stats for the `patch_count` most recent patches, printing one line per scope.
pub fn build_stats(conn: &mut Connection, patch_count: i64) -> Result<()> {
    build_stats_with(conn, patch_count, &mut |s| println!("{s}"))
}

/// [`build_stats`] with the progress lines sent to `log`.
///
/// Every scope is rebuilt from the raw rows, read once per patch: the same steps as an incremental build
/// ([`build_stats_incremental_with`]) takes for a scope it cannot add to. The tables equal
/// [`build_stats_reference_with`]'s row for row.
pub fn build_stats_with(conn: &mut Connection, patch_count: i64, log: &mut dyn FnMut(&str)) -> Result<()> {
    tune_for_build(conn)?;
    let patches = recent_patches(conn, patch_count)?;

    if patches.is_empty() {
        log("No matches stored yet. Run \"npm run crawl\" first.");
        return Ok(());
    }

    let unlabelled: i64 = conn.query_row("SELECT COUNT(*) n FROM participants WHERE comp_key IS NULL", [], |r| r.get(0))?;
    let all: i64 = conn.query_row("SELECT COUNT(*) n FROM participants", [], |r| r.get(0))?;
    if all != 0 && (unlabelled as f64) > (all as f64) * 0.5 {
        log("Most boards have no comp label yet. Run \"npm run recluster\" once, then \"npm run stats\" again.");
    }

    conn.execute_batch(INC_TEMP)?;
    let mut rebuilt = Vec::new();
    for patch in &patches {
        update_patch(conn, patch, log, &mut rebuilt, true)?;
    }
    drop_scratch(conn)?;
    if comp_method(conn)? == Method::Clusters {
        lineage::update(conn)?; // PRD 2 C13; before the trends, which follow it to last patch's comp
    }
    finish_trends(conn, &rebuilt)?;
    conn.execute_batch("PRAGMA optimize")?;
    Ok(())
}

/// Drops the scratch tables of [`update_patch`].
fn drop_scratch(conn: &Connection) -> Result<()> {
    conn.execute_batch(INC_DROP)?;
    conn.execute_batch("DROP TABLE IF EXISTS p; DROP TABLE IF EXISTS topc")?;
    comp_detail::inc_drop(conn)?;
    items_depth::inc_drop(conn)?;
    unit_depth::inc_drop(conn)?;
    Ok(())
}

/// The full rebuild as it was first written, a port of `src/stats/build.ts`: each scope on its own, straight from
/// the raw rows, in SQL. It is the definition of every number, and what the tests compare [`build_stats_with`]
/// and the incremental build against. It reads the raw rows once per scope and step, so it is slow on a large
/// database (about ten minutes per scope at a million boards); nothing but tests should call it.
pub fn build_stats_reference_with(conn: &mut Connection, patch_count: i64, log: &mut dyn FnMut(&str)) -> Result<()> {
    tune_for_build(conn)?;
    let patches = recent_patches(conn, patch_count)?;

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
    items_depth::prepare(conn)?;

    let mut rebuilt = Vec::new();
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
                rebuilt.push(watermark_key(patch, &scope));

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
    items_depth::inc_drop(conn)?;
    if comp_method(conn)? == Method::Clusters {
        lineage::update(conn)?; // PRD 2 C13; before the trends, which follow it to last patch's comp
    }
    finish_trends(conn, &rebuilt)?;
    conn.execute_batch("PRAGMA optimize")?;
    Ok(())
}

/// Tier letters and trends once every scope is written ([`tier::finish_all`]). Each scope already got them when
/// it was written, from the other scopes as they stood then; trends read other scopes, so they are worked out
/// again here, and a full and an incremental build end with the same values. Then the component weights of the
/// `rebuilt` scopes (PRD 3 U5), which read the tier letters.
fn finish_trends(conn: &mut Connection, rebuilt: &[String]) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tier::finish_all(&tx)?;
    items_depth::components_for(&tx, rebuilt)?;
    tx.commit()?;
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
/// The raw rows are read once per patch, not once per scope: every board is tagged with its rank band, time
/// window and watermark segment, the sums are taken per tag (and, for comps, per stored label), and each scope
/// adds up the tags it covers. The scopes are nested by rank and time, so this gives the same sums.
///
/// The result equals a full rebuild ([`build_stats`]) row for row.
pub fn build_stats_incremental_with(
    conn: &mut Connection,
    patch_count: i64,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<String>> {
    let patches = recent_patches(conn, patch_count)?;
    if patches.is_empty() {
        log("No matches stored yet. Run \"npm run crawl\" first.");
        return Ok(Vec::new());
    }

    tune_for_build(conn)?;
    conn.execute_batch(INC_TEMP)?;
    let mut touched = Vec::new();
    for patch in &patches {
        update_patch(conn, patch, log, &mut touched, false)?;
    }
    drop_scratch(conn)?;
    if !touched.is_empty() && comp_method(conn)? == Method::Clusters {
        lineage::update(conn)?; // PRD 2 C13; before the trends, which follow it to last patch's comp
    }
    if touched.is_empty() {
        log("Stats are up to date: no new matches.");
    } else {
        finish_trends(conn, &touched)?;
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
        &format!(
            "SELECT COALESCE(SUM(rowid <= ?4), 0), COALESCE(SUM(rowid > ?4), 0) FROM matches m
             WHERE patch = ?1 AND rank >= ?2 AND game_datetime >= ?3 AND {}",
            ranked_only("m")
        ),
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

/// Scratch tables for an incremental build. Every board of the patch the build needs goes in inc_b, tagged:
/// - band: how many bucket minimums its match's rank reaches (bucket i, counting from 1, is band >= i);
/// - w: how many window starts its match is at or after (a window is w >= its position, oldest start first);
/// - seg: how many watermarks its match is above (a scope adds the boards above its own watermark).
///
/// inc_r is the boards whose rate sums are needed. The other tables hold sums per tag: rates per band, window
/// and segment; comps per band, window and stored label, before look-alikes are merged.
const INC_TEMP: &str = "
CREATE TEMP TABLE IF NOT EXISTS inc_b (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT,
  band INTEGER NOT NULL, w INTEGER NOT NULL, seg INTEGER NOT NULL);
CREATE TEMP TABLE IF NOT EXISTS inc_r (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL,
  band INTEGER NOT NULL, w INTEGER NOT NULL, seg INTEGER NOT NULL);
CREATE TEMP TABLE IF NOT EXISTS inc_unit_dist (band, w, seg, character_id, star, n_items, games, sum_place, wins, top4);
CREATE TEMP TABLE IF NOT EXISTS inc_traits (band, w, seg, name, tier_current, num_units, games, sum_place, wins, top4);
CREATE TEMP TABLE IF NOT EXISTS inc_unit_items (band, w, seg, character_id, item, games, sum_place, wins, top4);
CREATE TEMP TABLE IF NOT EXISTS inc_items (band, w, seg, item, games, sum_place, wins, top4);
CREATE TEMP TABLE IF NOT EXISTS inc_unit_builds (band, w, seg, character_id, build, games, sum_place, wins, top4);
CREATE TEMP TABLE IF NOT EXISTS inc_comps (band, w, comp_key, n, sum_place, wins, top4, sum_level);
CREATE TEMP TABLE IF NOT EXISTS inc_comp_units (band, w, comp_key, character_id, games, sum_star, sum_items, full_items, sum_place);
CREATE TEMP TABLE IF NOT EXISTS inc_comp_items (band, w, comp_key, character_id, item, games, sum_place);
CREATE TEMP TABLE IF NOT EXISTS inc_comp_traits (band, w, comp_key, name, tier_current, num_units, style, games);
CREATE TEMP TABLE IF NOT EXISTS inc_comp_levels (band, w, comp_key, level, games, sum_place);
CREATE TEMP TABLE IF NOT EXISTS inc_cand (comp_key TEXT PRIMARY KEY) WITHOUT ROWID;
CREATE TEMP TABLE IF NOT EXISTS inc_map (src TEXT PRIMARY KEY, dst TEXT NOT NULL) WITHOUT ROWID;
CREATE TEMP TABLE IF NOT EXISTS inc_top (comp_key TEXT PRIMARY KEY) WITHOUT ROWID;
";
const INC_TABLES: [&str; 15] = [
    "inc_b", "inc_r", "inc_unit_dist", "inc_traits", "inc_unit_items", "inc_items", "inc_unit_builds", "inc_comps",
    "inc_comp_units", "inc_comp_items", "inc_comp_traits", "inc_comp_levels", "inc_cand", "inc_map", "inc_top",
];
const INC_DROP: &str = "
DROP TABLE IF EXISTS inc_b; DROP TABLE IF EXISTS inc_r; DROP TABLE IF EXISTS inc_unit_dist; DROP TABLE IF EXISTS inc_traits;
DROP TABLE IF EXISTS inc_unit_items; DROP TABLE IF EXISTS inc_items; DROP TABLE IF EXISTS inc_unit_builds;
DROP TABLE IF EXISTS inc_comps; DROP TABLE IF EXISTS inc_comp_units; DROP TABLE IF EXISTS inc_comp_items;
DROP TABLE IF EXISTS inc_comp_traits; DROP TABLE IF EXISTS inc_comp_levels; DROP TABLE IF EXISTS inc_cand;
DROP TABLE IF EXISTS inc_map; DROP TABLE IF EXISTS inc_top;
";

/// The rate sums per tag, as [`build_rates`] takes them per scope (the same groups, with the tag added).
const INC_RATES: [&str; 5] = [
    "INSERT INTO inc_unit_dist
     SELECT band, w, seg, character_id, star, n_items, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4)
     FROM (SELECT r.band, r.w, r.seg, u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, r.placement
           FROM inc_r r CROSS JOIN units u ON u.participant_id = r.id GROUP BY u.participant_id, u.character_id)
     GROUP BY band, w, seg, character_id, star, n_items",
    "INSERT INTO inc_traits
     SELECT r.band, r.w, r.seg, t.name, t.tier_current, MIN(t.num_units), COUNT(*), SUM(r.placement), SUM(r.placement = 1),
       SUM(r.placement <= 4)
     FROM inc_r r CROSS JOIN traits t ON t.participant_id = r.id GROUP BY r.band, r.w, r.seg, t.name, t.tier_current",
    "INSERT INTO inc_unit_items
     SELECT band, w, seg, character_id, item, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4)
     FROM (SELECT DISTINCT ui.participant_id, ui.character_id, ui.item, r.placement, r.band, r.w, r.seg
           FROM inc_r r CROSS JOIN unit_items ui ON ui.participant_id = r.id)
     GROUP BY band, w, seg, character_id, item",
    "INSERT INTO inc_items
     SELECT band, w, seg, item, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4)
     FROM (SELECT DISTINCT ui.participant_id, ui.item, r.placement, r.band, r.w, r.seg
           FROM inc_r r CROSS JOIN unit_items ui ON ui.participant_id = r.id)
     GROUP BY band, w, seg, item",
    "INSERT INTO inc_unit_builds
     SELECT r.band, r.w, r.seg, u.character_id, u.build, COUNT(*), SUM(r.placement), SUM(r.placement = 1), SUM(r.placement <= 4)
     FROM inc_r r CROSS JOIN units u ON u.participant_id = r.id WHERE u.build IS NOT NULL
     GROUP BY r.band, r.w, r.seg, u.character_id, u.build",
];

/// The comp sums per band, window and stored label, as [`build_comps`] and [`merge_lookalikes`] take them per
/// scope and merged label. Each board has one stored label, so a merged label's sums are its labels' sums added.
const INC_COMPS: [&str; 5] = [
    "INSERT INTO inc_comps
     SELECT band, w, comp_key, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), SUM(COALESCE(level, 0))
     FROM inc_b WHERE comp_key IS NOT NULL GROUP BY band, w, comp_key",
    "INSERT INTO inc_comp_units
     SELECT band, w, comp_key, character_id, COUNT(*), SUM(star), SUM(n_items), SUM(n_items = 3), SUM(placement)
     FROM (SELECT b.band, b.w, b.comp_key, u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, b.placement
           FROM inc_b b CROSS JOIN units u ON u.participant_id = b.id WHERE b.comp_key IS NOT NULL
           GROUP BY b.id, u.character_id)
     GROUP BY band, w, comp_key, character_id",
    "INSERT INTO inc_comp_items
     SELECT band, w, comp_key, character_id, item, COUNT(*), SUM(placement)
     FROM (SELECT DISTINCT b.id, b.band, b.w, b.comp_key, ui.character_id, ui.item, b.placement
           FROM inc_b b CROSS JOIN unit_items ui ON ui.participant_id = b.id WHERE b.comp_key IS NOT NULL)
     GROUP BY band, w, comp_key, character_id, item",
    "INSERT INTO inc_comp_traits
     SELECT b.band, b.w, b.comp_key, t.name, t.tier_current, MIN(t.num_units), MAX(t.style), COUNT(*)
     FROM inc_b b CROSS JOIN traits t ON t.participant_id = b.id WHERE b.comp_key IS NOT NULL
     GROUP BY b.band, b.w, b.comp_key, t.name, t.tier_current",
    "INSERT INTO inc_comp_levels
     SELECT band, w, comp_key, COALESCE(level, 0), COUNT(*), SUM(placement)
     FROM inc_b WHERE comp_key IS NOT NULL GROUP BY band, w, comp_key, COALESCE(level, 0)",
];

/// The rate tables: stat table, its sums per tag, its key columns. stat_traits also keeps the smallest unit count.
const RATE_SUMS: [(&str, &str, &str); 5] = [
    ("stat_unit_dist", "inc_unit_dist", "character_id, star, n_items"),
    ("stat_traits", "inc_traits", "name, tier_current"),
    ("stat_unit_items", "inc_unit_items", "character_id, item"),
    ("stat_items", "inc_items", "item"),
    ("stat_unit_builds", "inc_unit_builds", "character_id, build"),
];

/// One scope an incremental build rewrites, with the tags it covers.
struct Job {
    scope: String,
    /// Boards with band >= this, w >= this and (rate sums only) seg >= this.
    band: i64,
    w: i64,
    seg: i64,
    /// Adding to stored sums (true) or rebuilding the scope (false).
    add: bool,
    matches: i64,
    comps: i64,
}

/// Brings one patch's scopes up to date. The raw rows are read in one snapshot (a read transaction, so the
/// crawler keeps writing meanwhile); then each scope is written in its own short write transaction, from the
/// sums, with the watermark of that snapshot. With `force`, every scope is rebuilt whatever it holds (a full
/// rebuild); otherwise only the scopes with new matches are touched.
fn update_patch(
    conn: &mut Connection,
    patch: &str,
    log: &mut dyn FnMut(&str),
    touched: &mut Vec<String>,
    force: bool,
) -> Result<()> {
    let t0 = Instant::now();
    let now = now_ms();
    let since: Vec<i64> = WINDOWS.iter().map(|w| w.days.map_or(0, |d| now - d * 86_400_000)).collect();
    debug_assert!(BUCKETS.windows(2).all(|b| b[0].min < b[1].min), "buckets are nested, lowest first");

    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let top: i64 = tx.query_row("SELECT COALESCE(MAX(rowid), 0) FROM matches", [], |r| r.get(0))?;
    // (job, rank minimum, window start, watermark if adding)
    let mut planned: Vec<(Job, i64, i64, Option<i64>)> = Vec::new();
    for (bi, bucket) in BUCKETS.iter().enumerate() {
        for (wi, window) in WINDOWS.iter().enumerate() {
            let scope = scope_key(bucket.key, window.key);
            let plan = if force { Plan::Full } else { plan_scope(&tx, patch, &scope, bucket.min, since[wi])? };
            let mark = match plan {
                Plan::Skip => continue,
                Plan::Full => None,
                Plan::Add(after) => Some(after),
            };
            let matches: i64 = tx.query_row(
                &format!(
                    "SELECT COUNT(*) n FROM matches m WHERE patch = ? AND rank >= ? AND game_datetime >= ? AND {}",
                    ranked_only("m")
                ),
                params![patch, bucket.min, since[wi]],
                |r| r.get(0),
            )?;
            let w = since.iter().filter(|s| **s <= since[wi]).count() as i64;
            let job = Job { scope, band: bi as i64 + 1, w, seg: 0, add: mark.is_some(), matches, comps: 0 };
            planned.push((job, bucket.min, since[wi], mark));
        }
    }
    if planned.is_empty() {
        return Ok(()); // read only: dropping tx ends it
    }
    let mut marks: Vec<i64> = planned.iter().filter_map(|j| j.3).collect();
    marks.sort_unstable();
    marks.dedup();
    for (job, _, _, mark) in &mut planned {
        if let Some(m) = mark {
            job.seg = marks.iter().position(|x| x == m).unwrap() as i64 + 1;
        }
    }

    // Tag the boards. Values are integers from the database or from this function, so they go in as text.
    let sum_of = |terms: Vec<String>| if terms.is_empty() { "0".to_string() } else { terms.join(" + ") };
    let band = sum_of(BUCKETS.iter().map(|b| format!("(m.rank >= {})", b.min)).collect());
    let w = sum_of(since.iter().map(|s| format!("(m.game_datetime >= {s})")).collect());
    let seg = sum_of(marks.iter().map(|m| format!("(m.rowid > {m})")).collect());
    let min_rank = planned.iter().map(|j| j.1).min().unwrap();
    let min_since = planned.iter().map(|j| j.2).min().unwrap();
    for table in INC_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    tx.execute(
        &format!(
            "INSERT INTO inc_b SELECT pa.id, pa.placement, pa.level, pa.comp_key, {band}, {w}, {seg}
             FROM participants pa JOIN matches m ON m.match_id = pa.match_id
             WHERE m.patch = ? AND m.rank >= ? AND m.game_datetime >= ? AND {}",
            ranked_only("m")
        ),
        params![patch, min_rank, min_since],
    )?;
    let need: Vec<String> =
        planned.iter().map(|(j, ..)| format!("(band >= {} AND w >= {} AND seg >= {})", j.band, j.w, j.seg)).collect();
    tx.execute(
        &format!("INSERT INTO inc_r SELECT id, placement, band, w, seg FROM inc_b WHERE {}", need.join(" OR ")),
        [],
    )?;
    for sql in INC_RATES.iter().chain(INC_COMPS.iter()) {
        tx.execute(sql, [])?;
    }
    comp_detail::inc_sums(&tx)?;
    items_depth::prepare(&tx)?;
    items_depth::inc_sums(&tx)?;
    let boards = unit_depth::inc_sums(&tx)?;
    for (job, ..) in &mut planned {
        job.comps =
            tx.query_row("SELECT COUNT(*) FROM inc_b WHERE band >= ? AND w >= ?", params![job.band, job.w], |r| r.get(0))?;
    }
    tx.commit()?;

    for (i, (job, ..)) in planned.iter().enumerate() {
        let t = if i == 0 { t0 } else { Instant::now() };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_scope(&tx, patch, job, &boards)?;
        tx.execute(
            "INSERT OR REPLACE INTO stat_watermark (scope, last_rowid, built_at) VALUES (?, ?, ?)",
            params![watermark_key(patch, &job.scope), top, now_ms()],
        )?;
        tx.commit()?;
        if force {
            log(&format!(
                "patch {patch}  {:<13} {:>8} matches {:>9} comps  {:.1} s",
                job.scope,
                job.matches,
                job.comps,
                t.elapsed().as_secs_f64()
            ));
        } else {
            log(&format!(
                "patch {patch}  {:<13} {:>8} matches  {:<5}  {:.1} s",
                job.scope,
                job.matches,
                if job.add { "added" } else { "full" },
                t.elapsed().as_secs_f64()
            ));
        }
        touched.push(watermark_key(patch, &job.scope));
    }
    Ok(())
}

/// Writes one scope from the sums per tag, inside the caller's transaction.
fn write_scope(db: &Connection, patch: &str, job: &Job, boards: &onepass::PatchBoards) -> Result<()> {
    let k = params![patch, job.scope];
    if job.add {
        db.execute(
            "UPDATE stat_meta SET matches = ?, comps = ?, built_at = ? WHERE patch = ? AND bucket = ?",
            params![job.matches, job.comps, now_ms(), patch, job.scope],
        )?;
        db.execute("DELETE FROM stat_units WHERE patch = ? AND bucket = ?", k)?;
        db.execute("DELETE FROM stat_traits WHERE patch = ? AND bucket = ? AND tier_current = 0", k)?;
        for table in ["stat_comps", "stat_comp_units", "stat_comp_items", "stat_comp_traits", "stat_comp_levels", "stat_comp_variants"] {
            db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), k)?;
        }
        comp_detail::clear(db, patch, &job.scope)?;
        flex::clear(db, patch, &job.scope)?;
        unit_depth::clear(db, patch, &job.scope)?;
    } else {
        for table in STAT_TABLES {
            db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), k)?;
        }
        comp_detail::clear(db, patch, &job.scope)?;
        flex::clear(db, patch, &job.scope)?;
        items_depth::clear(db, patch, &job.scope)?;
        unit_depth::clear(db, patch, &job.scope)?;
        db.execute("INSERT INTO stat_meta VALUES (?,?,?,?,?)", params![patch, job.scope, job.matches, job.comps, now_ms()])?;
        if job.comps == 0 {
            return Ok(());
        }
    }

    // Rates: the covered tags added up, and added to what is stored (nothing, for a rebuilt scope).
    for (table, sums, keys) in RATE_SUMS {
        let traits = table == "stat_traits";
        let (col, sel, upd) = if traits {
            (", num_units", ", MIN(num_units)", ", num_units = MIN(num_units, excluded.num_units)")
        } else {
            ("", "", "")
        };
        db.execute(
            &format!(
                "INSERT INTO {table} (patch, bucket, {keys}{col}, games, sum_place, wins, top4)
                 SELECT ?1, ?2, {keys}{sel}, SUM(games), SUM(sum_place), SUM(wins), SUM(top4) FROM {sums}
                 WHERE band >= ?3 AND w >= ?4 AND seg >= ?5 GROUP BY {keys}
                 ON CONFLICT DO UPDATE SET games = games + excluded.games, sum_place = sum_place + excluded.sum_place,
                   wins = wins + excluded.wins, top4 = top4 + excluded.top4{upd}"
            ),
            params![patch, job.scope, job.band, job.w, job.seg],
        )?;
    }
    items_depth::write_inc(db, patch, &job.scope, job.band, job.w, job.seg)?;
    build_derived(db, patch, &job.scope)?;
    unit_depth::write_inc(db, patch, &job.scope, job.band, job.w)?;
    if job.comps > 0 && comp_method(db)? == Method::Clusters {
        cluster::comps_from_inc(db, patch, &job.scope, job.band, job.w, job.comps, boards)?;
    } else if job.comps > 0 {
        write_comps(db, patch, job, boards)?;
    }
    Ok(())
}

/// [`build_comps`] for one scope, from the comp sums per stored label instead of from the boards. The queries
/// keep the same shape (group by label, then order by games, then limit), so labels with the same number of
/// games come out in the same order as in a full rebuild.
fn write_comps(db: &Connection, patch: &str, job: &Job, boards: &onepass::PatchBoards) -> Result<()> {
    let min_games = COMP_MIN_GAMES.max(((job.comps as f64) * COMP_MIN_SHARE).ceil() as i64);
    db.execute_batch("DELETE FROM inc_cand; DELETE FROM inc_map; DELETE FROM inc_top")?;
    merge_lookalike_sums(db, job, min_games)?;
    overrides::apply_to_map(db)?; // PRD 2 C14, as apply_to_boards in a full rebuild

    // A label's sums, under the label it was merged into.
    let merged = |sums: &str, cols: &str| {
        format!(
            "SELECT COALESCE(m.dst, c.comp_key) comp_key, {cols} FROM {sums} c LEFT JOIN inc_map m ON m.src = c.comp_key
             WHERE c.band >= ?3 AND c.w >= ?4"
        )
    };
    let k4 = params![patch, job.scope, job.band, job.w];
    db.execute(
        &format!(
            "INSERT INTO stat_comps (patch, bucket, comp_key, games, sum_place, wins, top4, sum_level)
             SELECT ?1, ?2, comp_key, SUM(n), SUM(sum_place), SUM(wins), SUM(top4), SUM(sum_level)
             FROM ({}) GROUP BY comp_key HAVING SUM(n) >= ?5 ORDER BY SUM(n) DESC LIMIT ?6",
            merged("inc_comps", "n, sum_place, wins, top4, sum_level")
        ),
        params![patch, job.scope, job.band, job.w, min_games, COMP_LIMIT],
    )?;
    db.execute("INSERT INTO inc_top SELECT comp_key FROM stat_comps WHERE patch = ? AND bucket = ?", params![patch, job.scope])?;
    db.execute(
        &format!(
            "INSERT INTO stat_comp_units
             SELECT ?1, ?2, comp_key, character_id, SUM(games), SUM(sum_star), SUM(sum_items), SUM(full_items), SUM(sum_place)
             FROM ({}) JOIN inc_top USING (comp_key) GROUP BY comp_key, character_id",
            merged("inc_comp_units", "character_id, games, sum_star, sum_items, full_items, sum_place")
        ),
        k4,
    )?;
    db.execute(
        &format!(
            "INSERT INTO stat_comp_items
             SELECT ?1, ?2, comp_key, character_id, item, SUM(games), SUM(sum_place)
             FROM ({}) JOIN inc_top USING (comp_key) GROUP BY comp_key, character_id, item",
            merged("inc_comp_items", "character_id, item, games, sum_place")
        ),
        k4,
    )?;
    db.execute(
        &format!(
            "INSERT INTO stat_comp_traits
             SELECT ?1, ?2, comp_key, name, tier_current, MIN(num_units), MAX(style), SUM(games)
             FROM ({}) JOIN inc_top USING (comp_key) GROUP BY comp_key, name, tier_current",
            merged("inc_comp_traits", "name, tier_current, num_units, style, games")
        ),
        k4,
    )?;
    db.execute(
        &format!(
            "INSERT INTO stat_comp_levels
             SELECT ?1, ?2, comp_key, level, SUM(games), SUM(sum_place)
             FROM ({}) JOIN inc_top USING (comp_key) GROUP BY comp_key, level",
            merged("inc_comp_levels", "level, games, sum_place")
        ),
        k4,
    )?;
    comp_detail::write_inc(db, patch, &job.scope, job.band, job.w)?;
    flex::write_fast(db, patch, &job.scope, boards, &flex::Members::from_tags(db, boards, job.band, job.w)?)?;
    tier::finish_scope(db, patch, &job.scope)
}

/// [`merge_lookalikes`] from the comp sums: the same candidates, core units and folding, written to inc_map
/// (stored label to the label it folds into) instead of relabelling boards.
fn merge_lookalike_sums(db: &Connection, job: &Job, min_games: i64) -> Result<()> {
    let keys: Vec<(String, i64)> = db
        .prepare(
            "SELECT comp_key, SUM(n) games FROM inc_comps WHERE band >= ?1 AND w >= ?2
             GROUP BY comp_key HAVING SUM(n) >= ?3 ORDER BY games DESC LIMIT 400",
        )?
        .query_map(params![job.band, job.w, 3.max(min_games / 4)], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if keys.len() < 2 {
        return Ok(());
    }
    {
        let mut add = db.prepare("INSERT INTO inc_cand VALUES (?)")?;
        for (key, _) in &keys {
            add.execute([key])?;
        }
    }
    let games: HashMap<&str, i64> = keys.iter().map(|(k, g)| (k.as_str(), *g)).collect();
    let mut core: HashMap<String, HashSet<String>> = HashMap::new();
    {
        let mut freq = db.prepare(
            "SELECT comp_key, character_id, SUM(games) FROM inc_comp_units JOIN inc_cand USING (comp_key)
             WHERE band >= ?1 AND w >= ?2 GROUP BY comp_key, character_id",
        )?;
        let mut rows = freq.query(params![job.band, job.w])?;
        while let Some(r) = rows.next()? {
            let (key, unit, n): (String, String, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            if (n as f64) / (games[key.as_str()] as f64) < 0.5 {
                continue;
            }
            core.entry(key).or_default().insert(unit);
        }
    }

    let empty = HashSet::new();
    let mut kept: Vec<&str> = Vec::new();
    let mut remap = db.prepare("INSERT INTO inc_map VALUES (?, ?)")?;
    for (key, _) in &keys {
        let mine = core.get(key).unwrap_or(&empty);
        let twin = kept.iter().find(|other| {
            let theirs = &core[**other];
            let shared = mine.intersection(theirs).count();
            let union = mine.union(theirs).count();
            union > 0 && (shared as f64) / (union as f64) >= 0.75
        });
        if let Some(twin) = twin {
            remap.execute([key.as_str(), *twin])?;
        } else if !mine.is_empty() {
            kept.push(key);
        }
    }
    Ok(())
}

/// One patch and scope, inside the caller's transaction.
fn build_scope(db: &Connection, patch: &str, scope: &str, min_rank: i64, since: i64) -> Result<()> {
    let k = params![patch, scope];
    db.execute_batch("DELETE FROM p; DELETE FROM topc")?;
    db.execute(
        &format!(
            "INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
             JOIN matches m ON m.match_id = pa.match_id
             WHERE m.patch = ? AND m.rank >= ? AND m.game_datetime >= ? AND {}",
            ranked_only("m")
        ),
        params![patch, min_rank, since],
    )?;

    for table in STAT_TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), k)?;
    }
    comp_detail::clear(db, patch, scope)?;
    flex::clear(db, patch, scope)?;
    items_depth::clear(db, patch, scope)?;
    unit_depth::clear(db, patch, scope)?;

    let comps: i64 = db.query_row("SELECT COUNT(*) n FROM p", [], |r| r.get(0))?;
    let matches: i64 = db.query_row(
        &format!(
            "SELECT COUNT(*) n FROM matches m WHERE patch = ? AND rank >= ? AND game_datetime >= ? AND {}",
            ranked_only("m")
        ),
        params![patch, min_rank, since],
        |r| r.get(0),
    )?;
    db.execute("INSERT INTO stat_meta VALUES (?,?,?,?,?)", params![patch, scope, matches, comps, now_ms()])?;
    if comps == 0 {
        return Ok(());
    }

    build_rates(db, patch, scope)?;
    items_depth::build_full(db, patch, scope)?;
    build_derived(db, patch, scope)?;
    unit_depth::build_full(db, patch, scope)?; // before build_comps, which relabels the boards in p
    build_comps(db, patch, scope, comps, None)
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
///
/// `boards` is the patch's boards in memory when the caller has them (an incremental build that put its scope
/// in p); the flex tables are then counted from them instead of in SQL.
fn build_comps(db: &Connection, patch: &str, scope: &str, comps: i64, boards: Option<&onepass::PatchBoards>) -> Result<()> {
    let k = params![patch, scope];
    // ---- Comps ----
    let min_games = COMP_MIN_GAMES.max(((comps as f64) * COMP_MIN_SHARE).ceil() as i64);
    // PRD 2 C1: the comp method in use labels the boards in p; then your merges (C14) are applied.
    let labels = if comp_method(db)? == Method::Clusters {
        Some(cluster::label(db, min_games, COMP_LIMIT)?)
    } else {
        merge_lookalikes(db, min_games)?;
        None
    };
    overrides::apply_to_boards(db)?;
    db.execute(
        "INSERT INTO stat_comps (patch, bucket, comp_key, games, sum_place, wins, top4, sum_level)
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
    comp_detail::build_full(db, patch, scope)?;
    if let Some(labels) = labels {
        cluster::write_shape(db, patch, scope, &labels)?; // core, flex and variants (C1, C2)
    }
    match boards {
        Some(pb) => flex::write_fast(db, patch, scope, pb, &flex::Members::from_p(db, pb)?)?,
        None => flex::build_full(db, patch, scope)?,
    }
    tier::finish_scope(db, patch, scope)
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
                "INSERT OR IGNORE INTO matches VALUES (?, 'na1', '99.1', 99, ?, 2000, 8, NULL)",
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

    /// PRD 2 C3, C4: tier letters, last patch's and the last 3 days' average place on each comp row.
    #[test]
    fn comps_get_a_tier_and_their_trend() {
        let a: &[&str] = &["U1", "U2", "U3", "U4"];
        let c: &[&str] = &["U6", "U7", "U8", "U9"];
        let mut boards: Vec<(&str, i64, &[&str])> = Vec::new();
        boards.extend((1..=8).map(|p| ("X|A", p, a))); // M0: last patch, avg 4.5
        boards.extend([1, 1, 2, 2, 3, 3, 4, 4].map(|p| ("X|A", p, a))); // M1: this patch, 5 days ago
        boards.extend([1, 1, 1, 1, 2, 2, 2, 2].map(|p| ("X|A", p, a))); // M2: this patch, an hour ago
        boards.extend([5, 6, 7, 8, 5, 6, 7, 8].map(|p| ("Y|C", p, c))); // M3: this patch, an hour ago
        let (dir, mut conn) = world("trend", &boards);
        let now = now_ms();
        conn.execute("UPDATE matches SET patch = '99.0', game_datetime = ? WHERE match_id = 'M0'", [now - 10 * 86_400_000]).unwrap();
        conn.execute("UPDATE matches SET game_datetime = ? WHERE match_id = 'M1'", [now - 5 * 86_400_000]).unwrap();
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();

        type Row = (String, String, Option<String>, Option<f64>, Option<f64>);
        let rows = |patch: &str, scope: &str| -> Vec<Row> {
            conn.prepare("SELECT comp_key, bucket, tier, prev_place, d3_place FROM stat_comps WHERE patch = ? AND bucket = ? ORDER BY comp_key")
                .unwrap()
                .query_map([patch, scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let row = |k: &str, sc: &str, t: &str, prev: Option<f64>, d3: Option<f64>| -> Row {
            (k.into(), sc.into(), Some(t.into()), prev, d3)
        };
        // This patch, whole: X|A averages 2.0 (32 / 16), 4.5 last patch, 1.5 in the last 3 days. Y|C is new.
        // Two comps: the better one is A, the other C (see tier::tests).
        assert_eq!(rows("99.1", "ALL"), [row("X|A", "ALL", "A", Some(4.5), Some(1.5)), row("Y|C", "ALL", "C", None, Some(6.5))]);
        // The 3-day rows compare with last patch only.
        assert_eq!(rows("99.1", "ALL@3d"), [row("X|A", "ALL@3d", "A", Some(4.5), None), row("Y|C", "ALL@3d", "C", None, None)]);
        // Last patch has nothing before it, and its one comp sits in the middle.
        assert_eq!(rows("99.0", "ALL"), [row("X|A", "ALL", "B", None, None)]);
        let scores: Vec<f64> = conn
            .prepare("SELECT score FROM stat_comps WHERE patch = '99.1' AND bucket = 'ALL' ORDER BY comp_key")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // Two comps: every z is +1 or -1, so score is +-1, times confidence 16/416 and 8/408.
        assert!((scores[0] - 16.0 / 416.0).abs() < 1e-12 && (scores[1] + 8.0 / 408.0).abs() < 1e-12, "{scores:?}");
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

    pub(crate) const DAY: i64 = 86_400_000;

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
    pub(crate) type Game = (String, i64, i64);

    pub(crate) fn store(conn: &mut Connection, cal: &Calendar, seed: u64, games: &[Game]) {
        store_with(conn, cal, seed, games, 5);
    }

    pub(crate) fn store_with(conn: &mut Connection, cal: &Calendar, seed: u64, games: &[Game], floor: i64) {
        let mut rng = Rng(seed);
        for (id, rank, at) in games {
            let ing = Ingest { platform: "na1", rank: *rank, tier: tft_core::config::tier_name(*rank), snowball: false };
            assert!(ing.store(conn, &lobby(&mut rng, id, *at, floor), cal).unwrap());
        }
    }

    pub(crate) fn fresh_db(name: &str) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-stats-inc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        (dir, conn)
    }

    /// Set data making four of the synthetic items emblems, so the comp page's special items have rows.
    fn special_items(conn: &Connection) {
        for i in 0..4 {
            conn.execute(
                "INSERT INTO static_data VALUES ('item', ?, 'E', NULL, '{\"type\":\"emblem\"}')",
                [format!("I{i:02}")],
            )
            .unwrap();
        }
    }

    /// Set data for the item depth tables (PRD 3 U5, U7): the emblems [`special_items`] made give traits, and
    /// most other items are built from two of five components.
    fn item_depth_data(conn: &Connection) {
        conn.execute("UPDATE static_data SET data = json_set(data, '$.trait', 'T' || substr(id, 2)) WHERE kind = 'item'", []).unwrap();
        for i in 10..40 {
            conn.execute(
                "INSERT INTO static_data VALUES ('item', ?, 'X', NULL, json_object('type', 'craftable', 'composition', json_array(?, ?)))",
                params![format!("I{i:02}"), format!("C{}", i % 5), format!("C{}", (i / 5) % 5)],
            )
            .unwrap();
        }
    }

    /// Set data giving the synthetic units traits (by name, as Community Dragon lists them), so the trait unit sets
    /// (PRD 3 U8) have rows.
    pub(crate) fn depth_static(conn: &Connection) {
        for t in 0..20 {
            conn.execute("INSERT INTO static_data VALUES ('trait', ?, ?, NULL, '{}')", params![format!("T{t:02}"), format!("Trait {t:02}")])
                .unwrap();
        }
        for u in 0..60 {
            let traits = serde_json::json!({ "traits": [format!("Trait {:02}", u % 20), format!("Trait {:02}", (u * 3) % 20)] });
            conn.execute("INSERT INTO static_data VALUES ('unit', ?, 'U', NULL, ?)", params![format!("U{u:02}"), traits.to_string()])
                .unwrap();
        }
    }

    /// Every stat table in `a` equals the one in the database at `b`, row for row and value for value
    /// (stat_meta.built_at aside). Returns the number of rows compared.
    pub(crate) fn assert_same_stats(a: &Connection, b: &std::path::Path) -> i64 {
        a.execute("ATTACH DATABASE ? AS fresh", [b.to_str().unwrap()]).unwrap();
        let mut rows = 0;
        for table in
            STAT_TABLES.into_iter().chain(comp_detail::TABLES).chain(flex::TABLES).chain(items_depth::TABLES).chain(unit_depth::TABLES)
        {
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
        special_items(&conn);
        item_depth_data(&conn);
        depth_static(&conn);
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
        special_items(&fresh);
        item_depth_data(&fresh);
        depth_static(&fresh);
        store(&mut fresh, &cal, 1, &first);
        store_with(&mut fresh, &cal, 2, &more, 2);
        build_stats_reference_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        let compared = assert_same_stats(&conn, &fresh_dir.join("t.db"));
        assert!(compared > 1000, "only {compared} rows compared");
        let comps: i64 = conn.query_row("SELECT COUNT(*) FROM stat_comps", [], |r| r.get(0)).unwrap();
        assert!(comps > 20, "the synthetic matches should make comps ({comps})");
        // PRD 2 C3, C4: every comp row is graded, and the trend columns are filled where there is something to
        // compare with, so the comparison above covers them.
        let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(q("SELECT COUNT(*) FROM stat_comps WHERE tier IS NULL OR score IS NULL"), 0);
        assert!(q("SELECT COUNT(*) FROM stat_comps WHERE patch = '90.2' AND prev_place IS NOT NULL") > 10);
        assert!(q("SELECT COUNT(*) FROM stat_comps WHERE bucket NOT LIKE '%@3d' AND d3_place IS NOT NULL") > 10);
        assert_eq!(q("SELECT COUNT(DISTINCT tier) FROM stat_comps"), 5, "every letter is used");
        for table in comp_detail::TABLES.into_iter().chain(flex::TABLES).chain(items_depth::TABLES) {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap();
            assert!(n > 50, "{table} has only {n} rows, so the comparison proves little");
        }
        // PRD 3 U5: the S and A comps decide the weights in every scope that has them.
        assert!(q("SELECT COUNT(DISTINCT patch || bucket) FROM stat_components") >= 12);
        for (table, least) in [("stat_unit_pairs", 1000), ("stat_teammates", 20), ("stat_trait_sets", 50)] {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap();
            assert!(n > least, "{table} has only {n} rows, so the comparison proves little");
        }

        // Nothing new: nothing touched.
        assert!(build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap().is_empty());
        assert_eq!(built_at(&conn), after);

        // A full rebuild (every scope, raw rows read once per patch) gives the reference build's tables too.
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(assert_same_stats(&conn, &fresh_dir.join("t.db")), compared);
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
        build_stats_reference_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        assert_same_stats(&conn, &fresh_dir.join("t.db"));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&fresh_dir).unwrap();
    }

    /// PRD 1 E12: Double Up, Hyper Roll and revival lobbies are stored with their queue id, and no stat table counts
    /// them, in a full rebuild or an incremental one. A patch with only other-mode matches gets no stats at all.
    /// Ranked matches stored before E12 have no queue id and are counted.
    #[test]
    fn other_game_modes_are_stored_with_their_queue_and_left_out_of_every_stat() {
        let now = now_ms();
        let cal = Calendar(vec![
            PatchStart { patch: "90.1".into(), since: 0 },
            PatchStart { patch: "90.2".into(), since: now - 8 * DAY },
            PatchStart { patch: "90.3".into(), since: now - DAY / 2 }, // other modes only
        ]);
        let ranks = [0, 3, 4, 5, 6, 7, 9];
        let ranked = |first: usize, n: usize, from: i64, step: i64| -> Vec<Game> {
            (first..first + n).map(|i| (format!("R{i}"), ranks[i % 7], from + (i - first) as i64 * step)).collect()
        };
        // Other modes: a queue per match, every third one spelt queueId as Riot sometimes sends it.
        let others = |rng: &mut Rng, first: usize, n: usize, from: i64, step: i64| -> Vec<RiotMatch> {
            (first..first + n)
                .map(|i| {
                    let mut m = lobby(rng, &format!("O{i}"), from + (i - first) as i64 * step, 1 + (i % 4) as i64);
                    let q = [1160, 1130, 6000][i % 3];
                    if i % 3 == 2 {
                        (m.info.queue_id, m.info.queue_id_camel) = (None, Some(q));
                    } else {
                        m.info.queue_id = Some(q);
                    }
                    m
                })
                .collect()
        };
        let put = |conn: &mut Connection, ms: &[RiotMatch]| {
            for (i, m) in ms.iter().enumerate() {
                let ing = Ingest { platform: "na1", rank: ranks[i % 7], tier: "X", snowball: false };
                assert!(ing.store(conn, m, &cal).unwrap());
            }
        };
        let old = ranked(300, 20, now - 30 * DAY, 1_000_000); // 90.1
        let first = ranked(0, 200, now - 7 * DAY, 1_000_000); // 90.2, older than 3 days
        let more = ranked(200, 60, now - 4 * DAY, 2_000_000); // 90.2, both sides of 3 days
        let mut rng = Rng(99);
        let other_first = others(&mut rng, 0, 90, now - 7 * DAY, 5_000_000); // 90.2, both sides of 3 days
        let other_more = others(&mut rng, 90, 30, now - DAY / 4, 100_000); // 90.3 only
        let other_last = others(&mut rng, 120, 20, now - 6 * DAY, 1_000_000);

        // Mixed: ranked and other modes interleaved, full rebuild, then more of both and an incremental build.
        let (dir, mut conn) = fresh_db("modes");
        store(&mut conn, &cal, 6, &old);
        store(&mut conn, &cal, 3, &first[..100]);
        put(&mut conn, &other_first);
        store_with(&mut conn, &cal, 4, &first[100..], 5);
        // One ranked match stored before E12: no queue id.
        conn.execute("UPDATE matches SET queue_id = NULL WHERE match_id = 'R7'", []).unwrap();
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        put(&mut conn, &other_more);
        store_with(&mut conn, &cal, 5, &more, 3);
        put(&mut conn, &other_last);
        let touched = build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert!(!touched.is_empty() && touched.iter().all(|s| s.starts_with("90.2/")), "{touched:?}");

        let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(q("SELECT COUNT(*) FROM matches"), 280 + 140, "every match is stored");
        assert_eq!(q("SELECT COUNT(*) FROM matches WHERE queue_id = 1100"), 279);
        assert_eq!(q("SELECT COUNT(*) FROM matches WHERE queue_id IS NULL"), 1);
        for (queue, n) in [(1160, 47), (1130, 47), (6000, 46)] {
            assert_eq!(q(&format!("SELECT COUNT(*) FROM matches WHERE queue_id = {queue}")), n, "queue {queue}");
        }
        assert_eq!(q("SELECT COUNT(*) FROM stat_meta WHERE patch = '90.3'"), 0, "a patch of other modes only has no stats");
        assert_eq!(q("SELECT COUNT(DISTINCT patch) FROM stat_meta"), 2, "90.3 does not push 90.1 out of the two patches");
        assert_eq!(q("SELECT matches FROM stat_meta WHERE patch = '90.2' AND bucket = 'ALL'"), 260);

        // Only other modes new: nothing to rebuild.
        put(&mut conn, &others(&mut rng, 140, 10, now - 2 * DAY, 1_000_000));
        assert_eq!(build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap(), Vec::<String>::new());

        // The same ranked matches alone, full rebuild: every stat table must be the same.
        let (fresh_dir, mut fresh) = fresh_db("modes-fresh");
        store(&mut fresh, &cal, 6, &old);
        store(&mut fresh, &cal, 3, &first[..100]);
        store_with(&mut fresh, &cal, 4, &first[100..], 5);
        store_with(&mut fresh, &cal, 5, &more, 3);
        build_stats_reference_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        let compared = assert_same_stats(&conn, &fresh_dir.join("t.db"));
        assert!(compared > 1000, "only {compared} rows compared");
        // And a full rebuild of the mixed database gives them too.
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_same_stats(&conn, &fresh_dir.join("t.db"));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&fresh_dir).unwrap();
    }

    /// Boards straight into the tables, eight to a match (rank 8, an hour ago): (comp label, units) each.
    fn add_boards(conn: &Connection, first_match: usize, boards: &[(String, Vec<String>)]) {
        let now = now_ms();
        for (i, (comp, units)) in boards.iter().enumerate() {
            let m = format!("T{}", first_match + i / 8);
            conn.execute("INSERT OR IGNORE INTO matches VALUES (?, 'na1', '91.1', 99, ?, 2000, 8, NULL)", params![m, now - 3_600_000])
                .unwrap();
            conn.execute(
                "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, ?, ?)",
                params![m, format!("{m}-{i}"), 1 + (i % 8) as i64, 7 + (i % 3) as i64, comp],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for u in units {
                conn.execute("INSERT INTO units VALUES (?, ?, 2, 3, NULL, 1)", params![id, u]).unwrap();
                conn.execute("INSERT INTO unit_items VALUES (?, ?, 'I1')", params![id, u]).unwrap();
            }
            conn.execute("INSERT INTO traits VALUES (?, 'T', 3, 1, 1)", [id]).unwrap();
        }
    }

    /// Comps with the same number of games: which labels make the 400 merge candidates, which label of a
    /// look-alike pair the other folds into, and which make the top 120, all depend on the order of ties. The
    /// incremental build must break them the way a full rebuild does.
    #[test]
    fn tied_comp_counts_come_out_as_in_a_full_rebuild() {
        // 60 look-alike pairs (P|A and P|B share their units), 4 boards per label, and 300 single labels with
        // 5 boards. Candidates need 3 boards: 420 labels, so 20 of the tied pairs' labels miss the cut of 400 and
        // their pairs are not merged. The merged pairs (8 boards) get rows; the singles (5) tie for the rest of
        // the top 120.
        let mut labels: Vec<(String, Vec<String>, usize)> = Vec::new();
        for j in 0..60 {
            let units: Vec<String> = (0..4).map(|u| format!("P{j:03}u{u}")).collect();
            labels.push((format!("P{j:03}|A"), units.clone(), 4));
            labels.push((format!("P{j:03}|B"), units, 4));
        }
        for k in 0..300 {
            labels.push((format!("L{k:03}"), (0..4).map(|u| format!("L{k:03}u{u}")).collect(), 5));
        }
        // Not in label order, so the order boards are stored in differs from the order labels sort in.
        let n = labels.len();
        let shuffled: Vec<_> = (0..n).map(|i| &labels[(i * 11) % n]).collect(); // 11 and 420 share no factor
        let first: Vec<(String, Vec<String>)> =
            shuffled.iter().flat_map(|(c, u, _)| std::iter::repeat_n((c.clone(), u.clone()), 2)).collect();
        let more: Vec<(String, Vec<String>)> =
            shuffled.iter().rev().flat_map(|(c, u, g)| std::iter::repeat_n((c.clone(), u.clone()), g - 2)).collect();

        let (dir, mut conn) = fresh_db("ties");
        add_boards(&conn, 0, &first);
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let next = first.len().div_ceil(8);
        add_boards(&conn, next, &more);
        let mut lines = Vec::new();
        build_stats_incremental_with(&mut conn, 2, &mut |s| lines.push(s.to_string())).unwrap();
        assert!(lines.iter().any(|l| l.contains(" ALL ") && l.contains("added")), "{lines:?}");

        let (fresh_dir, mut fresh) = fresh_db("ties-fresh");
        add_boards(&fresh, 0, &first);
        add_boards(&fresh, next, &more);
        build_stats_reference_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        assert_same_stats(&conn, &fresh_dir.join("t.db"));

        // The ties were real: some pairs were split by the cut, and the singles were picked from more than fit.
        let top: Vec<(String, i64)> = conn
            .prepare("SELECT comp_key, games FROM stat_comps WHERE bucket = 'ALL'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(top.len(), 120);
        let pairs = top.iter().filter(|(k, g)| k.starts_with('P') && *g == 8).count();
        let singles = top.iter().filter(|(k, g)| k.starts_with('L') && *g == 5).count();
        assert_eq!(pairs + singles, 120, "{top:?}");
        assert!(pairs > 0 && pairs < 60, "{pairs} merged pairs: the cut of 400 should split some");
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
