//! Item depth (PRD 3 "Item page"): components (U5) and emblem breakpoints (U7). Tables from migration 8 in
//! `tft_store::schema::MIGRATIONS`. Item holders (U6) need no table of their own: `stat_unit_items` holds them.
//!
//! - **Components** (`stat_components`): for each base component, the completed items it builds (recipes from the
//!   stored set data, items whose composition is two components), each with a weight: its place change on the
//!   units of the scope's S and A comps (the first two tiers in `config/tier-list.json`). Per comp and unit, an
//!   item's place change is its average place on that unit in that comp, minus the unit's average place in the
//!   comp; the weight is the average of those, weighted by games. Negative is better. A scope with no S or A comp
//!   falls back to the item's place change across all boards (`stat_items.place_change`). Worked out from the
//!   stat tables once every scope has its tier letters, so a full and an incremental build give the same rows.
//!   Items with fewer games than the scope's minimum ([`min_games`]) are not stored.
//! - **Emblem tiers** (`stat_emblem_tiers`): per emblem (an item the set data types as an emblem and gives a
//!   trait), the level its trait is at on each board where a unit holds it (0 when the trait is not active), with
//!   games, place, wins and top 4. A board holding two copies counts once. These are plain sums over the boards,
//!   added up like the other rate tables; they are few (emblems times levels), so every row is kept.

use anyhow::Result;
use rusqlite::{Connection, params};

/// The tables this module writes, per patch and scope.
pub const TABLES: [&str; 2] = ["stat_components", "stat_emblem_tiers"];

/// Fewest games an item needs in a scope to get a component row: the same rule as a comp row.
pub fn min_games(boards: i64) -> i64 {
    crate::COMP_MIN_GAMES.max(((boards as f64) * crate::COMP_MIN_SHARE).ceil() as i64)
}

/// The tier letters that count as "currently listed S and A comps": the first two tiers of the tier list config.
pub fn top_tiers() -> Vec<String> {
    crate::tier::config().tiers.iter().take(2).map(|t| t.tier.clone()).collect()
}

/// Emblems and the trait each gives, from the stored set data, into the temporary table `emblem_trait`.
/// Run before [`build_full`] or [`inc_sums`].
pub fn prepare(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS emblem_trait (item TEXT COLLATE NOCASE PRIMARY KEY, trait TEXT NOT NULL) WITHOUT ROWID;
         DELETE FROM emblem_trait;
         INSERT OR IGNORE INTO emblem_trait
         SELECT id, json_extract(data, '$.trait') FROM static_data
         WHERE kind = 'item' AND json_extract(data, '$.type') = 'emblem' AND json_extract(data, '$.trait') IS NOT NULL
         ORDER BY id;",
    )?;
    Ok(())
}

/// Boards `{src} b` holding an emblem, one row per board and emblem, with the level of its trait on that board.
/// `{ti}` / `{to}` are where an incremental build adds its tag columns.
const EMBLEM_SQL: &str = "SELECT {to} item, trait, tier, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4)
  FROM (SELECT {ti} ui.item, e.trait, b.placement,
          COALESCE((SELECT MAX(t.tier_current) FROM traits t WHERE t.participant_id = b.id AND t.name = e.trait), 0) tier
        FROM {src} b JOIN unit_items ui ON ui.participant_id = b.id JOIN emblem_trait e ON e.item = ui.item
        GROUP BY b.id, ui.item)
  GROUP BY {to} item, trait, tier";

fn emblem_sql(src: &str, tagged: bool) -> String {
    let (ti, to) = if tagged { ("b.band, b.w, b.seg,", "band, w, seg,") } else { ("", "") };
    EMBLEM_SQL.replace("{src}", src).replace("{ti}", ti).replace("{to}", to)
}

/// Removes one scope's rows (inside the caller's transaction).
pub fn clear(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    for table in TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), params![patch, scope])?;
    }
    Ok(())
}

/// A full rebuild: the emblem sums over every board in `p`.
pub fn build_full(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    db.execute(
        &format!(
            "INSERT INTO stat_emblem_tiers (patch, bucket, item, trait, tier_current, games, sum_place, wins, top4)
             SELECT ?1, ?2, * FROM ({})",
            emblem_sql("p", false)
        ),
        params![patch, scope],
    )?;
    Ok(())
}

/// An incremental build, once per patch: the emblem sums per tag, from the boards in `inc_r`.
pub fn inc_sums(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS inc_emblem_tiers (band, w, seg, item, trait, tier_current, games, sum_place, wins, top4);
         DELETE FROM inc_emblem_tiers;",
    )?;
    db.execute(&format!("INSERT INTO inc_emblem_tiers {}", emblem_sql("inc_r", true)), [])?;
    Ok(())
}

/// Drops the scratch tables of an incremental build.
pub fn inc_drop(db: &Connection) -> Result<()> {
    db.execute_batch("DROP TABLE IF EXISTS inc_emblem_tiers; DROP TABLE IF EXISTS emblem_trait")?;
    Ok(())
}

/// An incremental build, per scope: the covered tags added up, and added to what is stored (nothing, for a
/// rebuilt scope), as the rate tables are.
pub fn write_inc(db: &Connection, patch: &str, scope: &str, band: i64, w: i64, seg: i64) -> Result<()> {
    db.execute(
        "INSERT INTO stat_emblem_tiers (patch, bucket, item, trait, tier_current, games, sum_place, wins, top4)
         SELECT ?1, ?2, item, trait, tier_current, SUM(games), SUM(sum_place), SUM(wins), SUM(top4) FROM inc_emblem_tiers
         WHERE band >= ?3 AND w >= ?4 AND seg >= ?5 GROUP BY item, trait, tier_current
         ON CONFLICT DO UPDATE SET games = games + excluded.games, sum_place = sum_place + excluded.sum_place,
           wins = wins + excluded.wins, top4 = top4 + excluded.top4",
        params![patch, scope, band, w, seg],
    )?;
    Ok(())
}

/// `stat_components` for one scope, from the stat tables as they stand (tier letters included). Replaces the
/// scope's rows.
pub fn components(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    db.execute("DELETE FROM stat_components WHERE patch = ? AND bucket = ?", params![patch, scope])?;
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS recipe (component TEXT NOT NULL, item TEXT NOT NULL COLLATE NOCASE,
           PRIMARY KEY (component, item)) WITHOUT ROWID;
         DELETE FROM recipe;
         INSERT OR IGNORE INTO recipe
         SELECT c.value, sd.id FROM static_data sd, json_each(sd.data, '$.composition') c
         WHERE sd.kind = 'item' AND json_array_length(sd.data, '$.composition') = 2 AND c.type = 'text';",
    )?;
    let boards: i64 = db
        .query_row("SELECT comps FROM stat_meta WHERE patch = ? AND bucket = ?", params![patch, scope], |r| r.get(0))
        .unwrap_or(0);
    let min = min_games(boards);
    let tiers = top_tiers();
    let (t1, t2) = (tiers.first().cloned().unwrap_or_default(), tiers.get(1).cloned().unwrap_or_default());
    let tiered: bool = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM stat_comps WHERE patch = ?1 AND bucket = ?2 AND tier IN (?3, ?4))",
        params![patch, scope, t1, t2],
        |r| r.get(0),
    )?;
    let insert = |weights: &str| {
        format!(
            "INSERT OR IGNORE INTO stat_components (patch, bucket, component, item, weight, games)
             SELECT ?1, ?2, r.component, x.item, x.weight, x.games FROM recipe r JOIN ({weights}) x ON r.item = x.item"
        )
    };
    if tiered {
        db.execute(
            &insert(
                "SELECT ci.item, SUM(ci.games) games,
                   (SUM(ci.sum_place) - SUM(ci.games * cu.sum_place * 1.0 / cu.games)) / SUM(ci.games) weight
                 FROM stat_comp_items ci
                 JOIN stat_comps c ON c.patch = ci.patch AND c.bucket = ci.bucket AND c.comp_key = ci.comp_key
                   AND c.tier IN (?4, ?5)
                 JOIN stat_comp_units cu ON cu.patch = ci.patch AND cu.bucket = ci.bucket AND cu.comp_key = ci.comp_key
                   AND cu.character_id = ci.character_id
                 WHERE ci.patch = ?1 AND ci.bucket = ?2 GROUP BY ci.item HAVING SUM(ci.games) >= ?3",
            ),
            params![patch, scope, min, t1, t2],
        )?;
    } else {
        db.execute(
            &insert(
                "SELECT item, games, place_change weight FROM stat_items
                 WHERE patch = ?1 AND bucket = ?2 AND games >= ?3 AND place_change IS NOT NULL",
            ),
            params![patch, scope, min],
        )?;
    }
    Ok(())
}

/// [`components`] for each of `scopes` ([`crate::watermark_key`]s: "patch/scope").
pub fn components_for(db: &Connection, scopes: &[String]) -> Result<()> {
    for key in scopes {
        if let Some((patch, scope)) = key.split_once('/') {
            components(db, patch, scope)?;
        }
    }
    db.execute_batch("DROP TABLE IF EXISTS recipe")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_stats_with, now_ms};

    /// (unit, items) on a board; (comp label, placement, units, active level of trait "Tr" or 0).
    type Board = (String, i64, Vec<(String, Vec<&'static str>)>, i64);

    fn world(name: &str, boards: &[Board]) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-items-depth-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        for (i, (comp, place, units, level)) in boards.iter().enumerate() {
            let m = format!("M{}", i / 8);
            conn.execute(
                "INSERT OR IGNORE INTO matches VALUES (?, 'na1', '99.1', 99, ?, 2000, 8, NULL)",
                params![m, now_ms() - 3_600_000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, 8, ?)",
                params![m, format!("P{i}"), place, comp],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for (unit, items) in units {
                conn.execute("INSERT INTO units VALUES (?, ?, 2, ?, NULL, 1)", params![id, unit, items.len() as i64]).unwrap();
                for it in items {
                    conn.execute("INSERT INTO unit_items VALUES (?, ?, ?)", params![id, unit, it]).unwrap();
                }
            }
            if *level > 0 {
                conn.execute("INSERT INTO traits VALUES (?, 'Tr', ?, ?, 1)", params![id, 2 * level, level]).unwrap();
            }
            conn.execute("INSERT INTO traits VALUES (?, 'T', 3, 1, 1)", [id]).unwrap();
        }
        // Recipes: X = C1 + C2, Y = C1 + C3, Z = C2 + C2. E is an emblem for Tr; F an emblem with no trait known.
        conn.execute_batch(
            r#"INSERT INTO static_data VALUES ('item', 'X', 'X', NULL, '{"type":"craftable","composition":["C1","C2"]}'),
                 ('item', 'Y', 'Y', NULL, '{"type":"craftable","composition":["C1","C3"]}'),
                 ('item', 'Z', 'Z', NULL, '{"type":"craftable","composition":["C2","C2"]}'),
                 ('item', 'e', 'E', NULL, '{"type":"emblem","composition":["C4","C1"],"trait":"Tr"}'),
                 ('item', 'F', 'F', NULL, '{"type":"emblem","composition":[]}'),
                 ('item', 'W', 'W', NULL, '{"type":"craftable","composition":["C5","C1"]}');"#,
        )
        .unwrap();
        (dir, conn)
    }

    /// Twelve comps on their own units, comp k placing worse as k grows, so the first ones get the top tiers. On
    /// each comp's carry, X goes on its best boards in the top three comps and its worst boards in the rest, and Y
    /// the other way round: across all boards Y looks better, on the S and A comps X is.
    fn boards(comps: usize) -> Vec<Board> {
        let mut out = Vec::new();
        for k in 0..comps {
            for j in 0..10usize {
                let place = (1 + (k as i64 * 6) / 11 + (j % 3) as i64).min(8);
                let top = k < 3;
                let carry = match (j % 3, top) {
                    (0, true) | (2, false) => vec!["X"],
                    (2, true) | (0, false) => vec!["Y"],
                    _ => vec!["Z", "Z"],
                };
                let second = if j % 2 == 0 { vec!["E"] } else if j % 5 == 1 { vec!["F"] } else { vec![] };
                // W: on one board per comp, so the three S and A comps give it too few games for a row.
                let third = if j == 1 { vec!["W"] } else { vec![] };
                let mut units = vec![(format!("K{k}a"), carry), (format!("K{k}b"), second), (format!("K{k}c"), third)];
                if j == 4 {
                    units.push((format!("K{k}c"), vec!["E"])); // a second emblem on the same board counts once
                }
                out.push((format!("K{k}|K{k}a"), place, units, (j % 3) as i64));
            }
        }
        out
    }

    fn q<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> T {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    type Row = (String, String, f64, i64);
    fn rows(conn: &Connection, sql: &str) -> Vec<Row> {
        conn.prepare(sql).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
    }

    fn same(got: &[Row], want: &[Row]) {
        assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
        for (g, w) in got.iter().zip(want) {
            assert!(g.0 == w.0 && g.1 == w.1 && g.3 == w.3 && (g.2 - w.2).abs() < 1e-9, "{g:?} vs {w:?}");
        }
    }

    /// U5: stat_components equals a direct count over the raw boards of the S and A comps.
    #[test]
    fn components_equal_a_recount_over_s_and_a_boards() {
        let (dir, mut conn) = world("recount", &boards(12));
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let top: i64 = q(&conn, "SELECT COUNT(*) FROM stat_comps WHERE bucket = 'ALL' AND tier IN ('S', 'A')");
        assert!((2..12).contains(&top), "{top} S and A comps");

        // Per board, carry unit and item: placement minus the unit's average placement in its comp.
        let want = rows(
            &conn,
            "WITH sa AS (SELECT comp_key FROM stat_comps WHERE bucket = 'ALL' AND tier IN ('S', 'A')),
                  on_board AS (SELECT DISTINCT pa.id, pa.comp_key, pa.placement, u.character_id FROM participants pa
                               JOIN units u ON u.participant_id = pa.id WHERE pa.comp_key IN (SELECT comp_key FROM sa)),
                  base AS (SELECT comp_key, character_id, AVG(placement) avg FROM on_board GROUP BY 1, 2),
                  held AS (SELECT DISTINCT b.id, b.comp_key, b.placement, b.character_id, ui.item FROM on_board b
                           JOIN unit_items ui ON ui.participant_id = b.id AND ui.character_id = b.character_id)
             SELECT r.c, h.item, AVG(h.placement - base.avg), COUNT(*) FROM held h JOIN base USING (comp_key, character_id)
             JOIN (SELECT 'C1' c, 'X' i UNION ALL SELECT 'C2', 'X' UNION ALL SELECT 'C1', 'Y' UNION ALL SELECT 'C3', 'Y'
                   UNION ALL SELECT 'C2', 'Z' UNION ALL SELECT 'C4', 'E' UNION ALL SELECT 'C1', 'E'
                   UNION ALL SELECT 'C5', 'W' UNION ALL SELECT 'C1', 'W') r ON r.i = h.item
             GROUP BY 1, 2 HAVING COUNT(*) >= 5 ORDER BY 1, 2",
        );
        let got = rows(&conn, "SELECT component, item, weight, games FROM stat_components WHERE bucket = 'ALL' ORDER BY 1, 2");
        same(&got, &want);
        assert!(got.iter().any(|r| r.0 == "C4" && r.1 == "E"), "an emblem built from components is listed, by its match id");
        assert_eq!(got.iter().filter(|r| r.0 == "C2" && r.1 == "Z").count(), 1, "Z = C2 + C2 is listed once");
        assert!(got.iter().all(|r| r.1 != "W"), "W has under 5 games on S and A comps: no row");
        assert_eq!(q::<i64>(&conn, "SELECT COUNT(DISTINCT bucket) FROM stat_components"), 8, "every scope gets rows");
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// U5: an item that does well on the S and A comps' units outranks one that only does well elsewhere, even
    /// though across all boards the other one looks better.
    #[test]
    fn component_weights_follow_the_s_and_a_comps() {
        let (dir, mut conn) = world("weights", &boards(12));
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let w = |item: &str| -> f64 { q(&conn, &format!("SELECT weight FROM stat_components WHERE bucket = 'ALL' AND component = 'C1' AND item = '{item}'")) };
        let all = |item: &str| -> f64 { q(&conn, &format!("SELECT place_change FROM stat_items WHERE bucket = 'ALL' AND item = '{item}'")) };
        assert!(all("Y") < all("X"), "across all boards Y looks better: {} vs {}", all("Y"), all("X"));
        assert!(w("X") < w("Y"), "on S and A comps X is better: {} vs {}", w("X"), w("Y"));
        assert!(w("X") < 0.0 && w("Y") > 0.0);
        let first: String = q(&conn, "SELECT item FROM stat_components WHERE bucket = 'ALL' AND component = 'C1' ORDER BY weight LIMIT 1");
        assert_eq!(first, "X");
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// U5 edge case: a scope with no S or A comp falls back to place change across all boards.
    #[test]
    fn without_s_or_a_comps_weights_are_place_change_over_all_boards() {
        let (dir, mut conn) = world("fallback", &boards(1)); // one comp: graded B
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(q::<i64>(&conn, "SELECT COUNT(*) FROM stat_comps WHERE bucket = 'ALL' AND tier IN ('S', 'A')"), 0);
        let got = rows(&conn, "SELECT component, item, weight, games FROM stat_components WHERE bucket = 'ALL' ORDER BY 1, 2");
        let want = rows(
            &conn,
            "SELECT r.c, si.item, si.place_change, si.games FROM stat_items si
             JOIN (SELECT 'C1' c, 'X' i UNION ALL SELECT 'C2', 'X' UNION ALL SELECT 'C1', 'Y' UNION ALL SELECT 'C3', 'Y'
                   UNION ALL SELECT 'C2', 'Z' UNION ALL SELECT 'C4', 'E' UNION ALL SELECT 'C1', 'E') r ON r.i = si.item
             WHERE si.bucket = 'ALL' AND si.games >= 5 ORDER BY 1, 2",
        );
        assert!(!want.is_empty());
        same(&got, &want);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// U7: stat_emblem_tiers equals a direct count: boards holding the emblem (once per board), by the level of
    /// its trait there. An emblem with no trait in the set data gets no rows.
    #[test]
    fn emblem_tiers_equal_a_recount() {
        let (dir, mut conn) = world("emblems", &boards(12));
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        type R = (String, String, i64, i64, i64, i64, i64);
        let get = |sql: &str| -> Vec<R> {
            conn.prepare(sql)
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let got = get("SELECT item, trait, tier_current, games, sum_place, wins, top4 FROM stat_emblem_tiers WHERE bucket = 'ALL' ORDER BY 1, 2, 3");
        let want = get(
            "SELECT 'E', 'Tr', IFNULL(t.tier_current, 0) lv, COUNT(*), SUM(pa.placement), SUM(pa.placement = 1), SUM(pa.placement <= 4)
             FROM participants pa LEFT JOIN traits t ON t.participant_id = pa.id AND t.name = 'Tr'
             WHERE pa.id IN (SELECT participant_id FROM unit_items WHERE item = 'E') GROUP BY lv ORDER BY lv",
        );
        assert_eq!(got, want);
        assert_eq!(got.len(), 3, "levels 0, 1 and 2");
        // 12 comps of 10 boards, the emblem on boards 0, 2, 4, 6, 8 of each (board 4 twice): 60 boards.
        assert_eq!(got.iter().map(|r| r.3).sum::<i64>(), 60);
        assert_eq!(q::<i64>(&conn, "SELECT COUNT(*) FROM stat_emblem_tiers WHERE item = 'F'"), 0);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
