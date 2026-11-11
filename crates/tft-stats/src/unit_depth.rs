//! Unit and trait page depth (PRD 3 U3, U4, U8). Tables from migration 7 in `tft_store::schema::MIGRATIONS`,
//! written per patch and scope like the other stat tables (and, like `comp_detail`, kept out of `STAT_TABLES`).
//!
//! - **Item pairs** (`stat_unit_pairs`, U3): per unit, boards where one copy of it holds both `item_a` and `item_b`
//!   (sorted; the same item twice is a real pair). `item_c` is the third item of a three-item build containing
//!   that pair, or '' for "any third item or none" (the PRD's NULL; '' because it is part of the key). Two copies of
//!   one unit on a board pool their items for the '' rows, as `stat_unit_items` does; the third-item rows come from
//!   each copy's own build. A board counts once per row.
//! - **Teammates** (`stat_teammates`, U4): per pair of different units, boards with both on them.
//! - **Trait unit sets** (`stat_trait_sets`, U8): per trait and active breakpoint, which of the board's units carry
//!   the trait (by the stored set data's unit traits), as sorted ids joined with '|'. Without set data there are no
//!   rows, and the trait page says so.
//!
//! The star by items grid (U1) is `stat_unit_dist`, which already holds games, place, top 4 and wins per unit, star
//! level and item count; it is not copied.
//!
//! Units the data file `no_item_units.json` names (summons, units that cannot hold items) get no pair rows.
//!
//! Every value is a count or sum over a scope's boards, so a full rebuild takes them from the boards in `p` and an
//! incremental build adds up its sums per rank band and time window from `inc_b`: the same SQL, run on either
//! source. Rows under the scope minimum are not stored (the minimum is applied to the scope's total, so a full and
//! an incremental build keep the same rows).

use anyhow::Result;
use rusqlite::{Connection, params};

/// The tables this module writes, per patch and scope.
pub const TABLES: [&str; 3] = ["stat_unit_pairs", "stat_teammates", "stat_trait_sets"];

/// Fewest boards for a pair or trait set row to be stored.
pub const DEPTH_MIN_GAMES: i64 = 5;
/// Fewest shared boards for a teammate row (PRD 3 U4). Below it nothing is shown, so nothing is stored.
pub const TEAMMATE_MIN_GAMES: i64 = 200;

/// The data file of units left out of pair stats.
pub const NO_ITEM_UNITS: &str = include_str!("no_item_units.json");

/// The LIKE patterns from [`NO_ITEM_UNITS`], as a JSON array (for `json_each`).
pub fn no_item_patterns() -> String {
    let v: serde_json::Value = serde_json::from_str(NO_ITEM_UNITS).expect("no_item_units.json is valid JSON");
    v["patterns"].to_string()
}

/// One summed table: `sql` makes rows from boards `{src} b` (columns id, placement); `{ti}` / `{to}` are where an
/// incremental build adds its tag columns (inside the board-level query and outside it).
struct Sum {
    table: &'static str,
    inc: &'static str,
    keys: &'static str,
    min: i64,
    sql: &'static str,
}

/// A unit id that may hold items: not matched by a pattern in the data file.
const HOLDS_ITEMS: &str = "NOT EXISTS (SELECT 1 FROM dep_no_items n WHERE {u}.character_id LIKE n.pat)";

const SUMS: [Sum; 3] = [
    Sum {
        table: "stat_unit_pairs",
        inc: "inc_dep_pairs",
        keys: "character_id, item_a, item_b, item_c",
        min: DEPTH_MIN_GAMES,
        sql: "SELECT {to} character_id, item_a, item_b, item_c, COUNT(*) games, SUM(placement) sum_place FROM (
                SELECT DISTINCT {ti} b.id, x.character_id, MIN(x.item, y.item) item_a, MAX(x.item, y.item) item_b, '' item_c, b.placement
                FROM {src} b JOIN unit_items x ON x.participant_id = b.id
                JOIN unit_items y ON y.participant_id = b.id AND y.character_id = x.character_id AND y.rowid > x.rowid
                WHERE {holds_x}
                UNION ALL
                SELECT DISTINCT {to} id, character_id,
                  CASE k WHEN 1 THEN i2 ELSE i1 END, CASE k WHEN 3 THEN i2 ELSE i3 END,
                  CASE k WHEN 1 THEN i1 WHEN 2 THEN i2 ELSE i3 END, placement
                FROM (SELECT {to} id, character_id, placement, substr(build, 1, instr(build, '|') - 1) i1,
                        substr(rest, 1, instr(rest, '|') - 1) i2, substr(rest, instr(rest, '|') + 1) i3
                      FROM (SELECT {ti} b.id, u.character_id, b.placement, u.build, substr(u.build, instr(u.build, '|') + 1) rest
                            FROM {src} b JOIN units u ON u.participant_id = b.id WHERE u.build IS NOT NULL AND {holds_u}))
                CROSS JOIN (SELECT 1 k UNION ALL SELECT 2 UNION ALL SELECT 3))
              GROUP BY {to} character_id, item_a, item_b, item_c",
    },
    Sum {
        table: "stat_teammates",
        inc: "inc_dep_teammates",
        keys: "character_id, other",
        min: TEAMMATE_MIN_GAMES,
        // Each pair is counted once (the lower id first), then written both ways.
        sql: "WITH bu AS MATERIALIZED (
                SELECT DISTINCT {ti} b.id, u.character_id, b.placement FROM {src} b JOIN units u ON u.participant_id = b.id),
              h AS MATERIALIZED (
                SELECT {xto} x.character_id lo, y.character_id hi, COUNT(*) games, SUM(x.placement) sum_place
                FROM bu x JOIN bu y ON y.id = x.id AND y.character_id > x.character_id GROUP BY {xto} x.character_id, y.character_id)
              SELECT {to} lo character_id, hi other, games, sum_place FROM h
              UNION ALL SELECT {to} hi, lo, games, sum_place FROM h",
    },
    Sum {
        table: "stat_trait_sets",
        inc: "inc_dep_trait_sets",
        keys: "name, tier_current, units",
        min: DEPTH_MIN_GAMES,
        sql: "SELECT {to} name, tier_current, units, COUNT(*) games, SUM(placement) sum_place FROM (
                SELECT {to} id, name, tier_current, placement, group_concat(cid, '|' ORDER BY cid) units FROM (
                  SELECT DISTINCT {ti} b.id, t.name, t.tier_current, b.placement, u.character_id cid
                  FROM {src} b JOIN traits t ON t.participant_id = b.id JOIN units u ON u.participant_id = b.id
                  JOIN dep_unit_traits ut ON ut.unit = u.character_id AND ut.trait = t.name)
                GROUP BY {to} id, name, tier_current)
              GROUP BY {to} name, tier_current, units",
    },
];

fn fill(sql: &str, src: &str, tagged: bool) -> String {
    let (ti, to, xto) = if tagged { ("b.band, b.w,", "band, w,", "x.band, x.w,") } else { ("", "", "") };
    sql.replace("{holds_x}", &HOLDS_ITEMS.replace("{u}", "x"))
        .replace("{holds_u}", &HOLDS_ITEMS.replace("{u}", "u"))
        .replace("{src}", src)
        .replace("{ti}", ti)
        .replace("{xto}", xto)
        .replace("{to}", to)
}

/// The lookup tables the sums read: units left out of pairs, and each unit's traits from the set data (trait ids
/// as matches store them; the set data lists a unit's traits by name).
fn prepare(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS dep_no_items (pat TEXT NOT NULL);
         CREATE TEMP TABLE IF NOT EXISTS dep_unit_traits (unit TEXT NOT NULL COLLATE NOCASE, trait TEXT NOT NULL COLLATE NOCASE);
         CREATE INDEX IF NOT EXISTS temp.dep_unit_traits_unit ON dep_unit_traits(unit, trait);
         DELETE FROM dep_no_items; DELETE FROM dep_unit_traits;",
    )?;
    db.execute("INSERT INTO dep_no_items SELECT value FROM json_each(?)", [no_item_patterns()])?;
    db.execute_batch(
        "INSERT INTO dep_unit_traits SELECT DISTINCT u.id, t.id FROM static_data u, json_each(u.data, '$.traits') j
           JOIN static_data t ON t.kind = 'trait' AND t.name = j.value
         WHERE u.kind = 'unit' AND json_valid(u.data) AND json_type(u.data, '$.traits') = 'array'",
    )?;
    Ok(())
}

/// Removes one scope's rows (inside the caller's transaction).
pub fn clear(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    for table in TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), params![patch, scope])?;
    }
    Ok(())
}

/// A full rebuild: every board of the scope, in `p`.
pub fn build_full(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    prepare(db)?;
    for s in &SUMS {
        db.execute(
            &format!(
                "INSERT INTO {t} (patch, bucket, {k}, games, sum_place)
                 SELECT ?1, ?2, {k}, games, sum_place FROM ({sql}) WHERE games >= ?3",
                t = s.table,
                k = s.keys,
                sql = fill(s.sql, "(SELECT id, placement FROM p)", false)
            ),
            params![patch, scope, s.min],
        )?;
    }
    Ok(())
}

/// An incremental build, once per patch: the sums per rank band and time window, from `inc_b`.
pub fn inc_sums(db: &Connection) -> Result<()> {
    prepare(db)?;
    for s in &SUMS {
        db.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {inc} (band, w, {k}, games, sum_place); DELETE FROM {inc};",
            inc = s.inc,
            k = s.keys
        ))?;
        db.execute(&format!("INSERT INTO {} SELECT * FROM ({})", s.inc, fill(s.sql, "(SELECT id, placement, band, w FROM inc_b)", true)), [])?;
    }
    Ok(())
}

/// Drops the scratch tables [`inc_sums`] made.
pub fn inc_drop(db: &Connection) -> Result<()> {
    for s in &SUMS {
        db.execute_batch(&format!("DROP TABLE IF EXISTS {}", s.inc))?;
    }
    Ok(())
}

/// An incremental build, per scope: the covered tags added up. The scope's rows are rewritten in full.
pub fn write_inc(db: &Connection, patch: &str, scope: &str, band: i64, w: i64) -> Result<()> {
    for s in &SUMS {
        db.execute(
            &format!(
                "INSERT INTO {t} (patch, bucket, {k}, games, sum_place)
                 SELECT ?1, ?2, {k}, SUM(games), SUM(sum_place) FROM {inc} WHERE band >= ?3 AND w >= ?4
                 GROUP BY {k} HAVING SUM(games) >= ?5",
                t = s.table,
                k = s.keys,
                inc = s.inc
            ),
            params![patch, scope, band, w, s.min],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incremental_tests::{DAY, Game, depth_static, fresh_db, store};
    use crate::{build_stats_with, now_ms};
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use tft_core::patches::{Calendar, PatchStart};

    type Counts<K> = BTreeMap<K, (i64, i64)>;

    fn add<K: Ord>(m: &mut Counts<K>, k: K, place: i64) {
        let e = m.entry(k).or_insert((0, 0));
        e.0 += 1;
        e.1 += place;
    }

    fn stored<K: Ord>(conn: &Connection, sql: &str, key: impl Fn(&rusqlite::Row) -> K) -> Counts<K> {
        let mut stmt = conn.prepare(sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut out = BTreeMap::new();
        while let Some(r) = rows.next().unwrap() {
            out.insert(key(r), (r.get("games").unwrap(), r.get("sum_place").unwrap()));
        }
        out
    }

    /// Every new table equals a count over the raw rows made in Rust, without the build's SQL: pairs per copy of a
    /// unit (with items pooled across copies for "any third item"), teammates per board, trait unit sets from the
    /// set data. A summoned unit gets teammates but no pairs.
    #[test]
    fn every_unit_and_trait_depth_stat_equals_a_recount_over_raw_rows() {
        let now = now_ms();
        let cal = Calendar(vec![PatchStart { patch: "90.1".into(), since: 0 }]);
        let games: Vec<Game> = (0..300).map(|i| (format!("R{i}"), (i % 10) as i64, now - 2 * DAY + i as i64 * 60_000)).collect();
        let (dir, mut conn) = fresh_db("depth-recount");
        depth_static(&conn);
        store(&mut conn, &cal, 11, &games);
        const SUMMON: &str = "TFT90_SummonU07";
        for t in ["units", "unit_items"] {
            conn.execute(&format!("UPDATE {t} SET character_id = ? WHERE character_id = 'U07'"), [SUMMON]).unwrap();
        }
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();

        let boards: HashMap<i64, i64> = conn
            .prepare("SELECT pa.id, pa.placement FROM participants pa JOIN matches m USING (match_id) WHERE m.patch = '90.1'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(boards.len(), 2400);
        let units: Vec<(i64, String, Option<String>)> = conn
            .prepare("SELECT participant_id, character_id, build FROM units")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let items: Vec<(i64, String, String)> = conn
            .prepare("SELECT participant_id, character_id, item FROM unit_items ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let traits: Vec<(i64, String, i64)> = conn
            .prepare("SELECT participant_id, name, tier_current FROM traits")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let summoned = |u: &str| u.to_lowercase().contains("summon");

        // Pairs.
        let mut keys: BTreeSet<(i64, String, String, String, String)> = BTreeSet::new();
        let mut held: HashMap<(i64, String), Vec<String>> = HashMap::new();
        for (b, u, it) in &items {
            held.entry((*b, u.clone())).or_default().push(it.clone());
        }
        for ((b, u), its) in &held {
            for i in 0..its.len() {
                for j in i + 1..its.len() {
                    let (a, c) = if its[i] <= its[j] { (&its[i], &its[j]) } else { (&its[j], &its[i]) };
                    keys.insert((*b, u.clone(), a.clone(), c.clone(), String::new()));
                }
            }
        }
        for (b, u, build) in &units {
            let Some(build) = build else { continue };
            let x: Vec<&str> = build.split('|').collect();
            assert_eq!(x.len(), 3);
            for (third, rest) in [(x[0], [x[1], x[2]]), (x[1], [x[0], x[2]]), (x[2], [x[0], x[1]])] {
                keys.insert((*b, u.clone(), rest[0].into(), rest[1].into(), third.into()));
            }
        }
        let mut pairs: Counts<(String, String, String, String)> = BTreeMap::new();
        for (b, u, a, c, third) in keys {
            if boards.contains_key(&b) && !summoned(&u) {
                add(&mut pairs, (u, a, c, third), boards[&b]);
            }
        }
        pairs.retain(|_, v| v.0 >= DEPTH_MIN_GAMES);
        let got = stored(&conn, "SELECT * FROM stat_unit_pairs WHERE patch = '90.1' AND bucket = 'ALL'", |r| {
            (r.get("character_id").unwrap(), r.get("item_a").unwrap(), r.get("item_b").unwrap(), r.get("item_c").unwrap())
        });
        assert!(pairs.len() > 200, "{} pair rows", pairs.len());
        assert!(pairs.keys().any(|k| k.1 == k.2 && k.3.is_empty()), "two of the same item is a pair");
        assert!(pairs.keys().any(|k| !k.3.is_empty()), "third-item rows");
        assert_eq!(got, pairs, "stat_unit_pairs differs from a recount");
        assert!(got.keys().all(|k| k.0 != SUMMON));

        // Teammates.
        let mut on: HashMap<i64, BTreeSet<String>> = HashMap::new();
        for (b, u, _) in &units {
            on.entry(*b).or_default().insert(u.clone());
        }
        let mut mates: Counts<(String, String)> = BTreeMap::new();
        for (b, us) in &on {
            let Some(place) = boards.get(b) else { continue };
            for x in us {
                for y in us.iter().filter(|y| *y != x) {
                    add(&mut mates, (x.clone(), y.clone()), *place);
                }
            }
        }
        mates.retain(|_, v| v.0 >= TEAMMATE_MIN_GAMES);
        let got = stored(&conn, "SELECT * FROM stat_teammates WHERE patch = '90.1' AND bucket = 'ALL'", |r| {
            (r.get("character_id").unwrap(), r.get("other").unwrap())
        });
        assert!(mates.len() > 10, "{} teammate rows", mates.len());
        assert_eq!(got, mates, "stat_teammates differs from a recount");
        assert!(got.keys().any(|k| k.0 == SUMMON), "a summoned unit still has teammates");

        // Trait unit sets: unit Uu carries "Trait u%20" and "Trait 3u%20" (depth_static); trait Tt is named "Trait t".
        let carries = |u: &str, t: &str| -> bool {
            let (Some(u), Some(t)) = (u.strip_prefix('U'), t.strip_prefix('T')) else { return false };
            let (u, t): (i64, i64) = (u.parse().unwrap(), t.parse().unwrap());
            u % 20 == t || (u * 3) % 20 == t
        };
        let mut sets: Counts<(String, i64, String)> = BTreeMap::new();
        for (b, name, tier) in &traits {
            let Some(place) = boards.get(b) else { continue };
            let with: Vec<&str> = on[b].iter().filter(|u| carries(u, name)).map(String::as_str).collect();
            if !with.is_empty() {
                add(&mut sets, (name.clone(), *tier, with.join("|")), *place);
            }
        }
        sets.retain(|_, v| v.0 >= DEPTH_MIN_GAMES);
        let got = stored(&conn, "SELECT * FROM stat_trait_sets WHERE patch = '90.1' AND bucket = 'ALL'", |r| {
            (r.get("name").unwrap(), r.get("tier_current").unwrap(), r.get("units").unwrap())
        });
        assert!(sets.len() > 20, "{} trait set rows", sets.len());
        assert_eq!(got, sets, "stat_trait_sets differs from a recount");
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Teammates need 200 shared boards: 200 is kept, 199 is not.
    #[test]
    fn teammates_need_two_hundred_shared_boards() {
        let (dir, mut conn) = fresh_db("depth-200");
        for i in 0..200i64 {
            let m = format!("M{}", i / 8);
            conn.execute("INSERT OR IGNORE INTO matches VALUES (?, 'na1', '99.1', 99, ?, 2000, 8, NULL)", params![m, now_ms() - 3_600_000])
                .unwrap();
            conn.execute("INSERT INTO participants (match_id, puuid, placement, comp_key) VALUES (?, ?, ?, 'X|A')", params![m, format!("P{i}"), 1 + i % 8])
                .unwrap();
            let id = conn.last_insert_rowid();
            let mut us = vec!["A", "B"];
            if i > 0 {
                us.push("C");
            }
            for u in us {
                conn.execute("INSERT INTO units VALUES (?, ?, 2, 0, NULL, 1)", params![id, u]).unwrap();
            }
        }
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let rows: Vec<(String, String, i64)> = conn
            .prepare("SELECT character_id, other, games FROM stat_teammates WHERE bucket = 'ALL' ORDER BY 1, 2")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, [("A".to_string(), "B".to_string(), 200), ("B".to_string(), "A".to_string(), 200)]);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
