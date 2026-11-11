//! The comp page sections (PRD 2 "Comp page sections"): levelling (C5), carry builds and items (C6), item
//! holders (C7) and special items (C9). Tables from migration 4 in `tft_store::schema::MIGRATIONS`.
//!
//! Every value is a plain count or sum over the boards of a listed comp, so a full rebuild takes them from the
//! boards in `p` and an incremental build adds up its per-tag sums: the same SQL, run on either source.
//!
//! - **Levelling** (`stat_comp_level_bands`): boards by placement band and final level, with the last round
//!   reached. Bands: 1 is 1st–2nd, 2 is 3rd–4th, 3 is 5th–8th. Riot only reports the level a board ended on,
//!   so a levelling guide made from it is an estimate.
//! - **Carry boards**: boards with at least one unit holding three items. A board without one is still in its
//!   comp, but left out of every carry, build, item and holder count (PRD 2 edge cases).
//! - **Carry units** (`stat_comp_carry_units`): per unit on carry boards, games and place on the board at all
//!   (`games`), and with three items (`full_*`). The baselines for item and build place change.
//! - **Carry items** (`stat_comp_carry_items`) and **builds** (`stat_comp_builds`): per unit, on carry boards.
//! - **Holders** (`stat_comp_holders`): for a unit holding three items (the carry), each lower-cost unit on the
//!   same board (Riot's rarity, lower means cheaper). Which of them share a trait or role is set data, applied
//!   by the API.
//! - **Special items** (`stat_comp_special`): `stat_comp_items` rows whose item is an emblem, artifact or
//!   radiant, by the item type in the stored set data (or, for an item it does not list, by its id, as the API's
//!   `describe` does).

use anyhow::Result;
use rusqlite::{Connection, params};

/// The tables this module writes, per patch and scope.
pub const TABLES: [&str; 6] = [
    "stat_comp_level_bands", "stat_comp_carry_units", "stat_comp_carry_items", "stat_comp_builds", "stat_comp_holders",
    "stat_comp_special",
];

/// A board with at least one unit holding three items.
const CARRY_BOARD: &str = "EXISTS (SELECT 1 FROM units x WHERE x.participant_id = b.id AND x.n_items = 3)";

/// One summed table. `sql` makes its rows from boards `{src} b`; `{ti}` and `{to}` are where an incremental build
/// adds its tag columns (inside the board-level query and outside it). `keys` are the stat table's key columns
/// after comp_key, `inc_keys` the same columns in the scratch table (where `band` is taken by the rank tag).
struct Sum {
    table: &'static str,
    inc: &'static str,
    keys: &'static str,
    inc_keys: &'static str,
    vals: &'static str,
    sql: &'static str,
}

const SUMS: [Sum; 5] = [
    Sum {
        table: "stat_comp_level_bands",
        inc: "inc_cd_levels",
        keys: "band, level",
        inc_keys: "pband, level",
        vals: "games, sum_place, rounds, sum_round",
        sql: "SELECT {to} comp_key, pband, level, COUNT(*), SUM(placement), SUM(last_round IS NOT NULL), SUM(COALESCE(last_round, 0))
              FROM (SELECT {ti} b.comp_key, CASE WHEN b.placement <= 2 THEN 1 WHEN b.placement <= 4 THEN 2 ELSE 3 END pband,
                      COALESCE(b.level, 0) level, b.placement, pa.last_round
                    FROM {src} b JOIN participants pa ON pa.id = b.id)
              GROUP BY {to} comp_key, pband, level",
    },
    Sum {
        table: "stat_comp_carry_units",
        inc: "inc_cd_carry_units",
        keys: "character_id",
        inc_keys: "character_id",
        vals: "games, sum_place, full_games, full_sum_place, full_top4",
        sql: "SELECT {to} comp_key, character_id, COUNT(*), SUM(placement), SUM(n_items = 3),
                SUM(CASE WHEN n_items = 3 THEN placement ELSE 0 END), SUM(n_items = 3 AND placement <= 4)
              FROM (SELECT {ti} b.comp_key, u.character_id, MAX(u.n_items) n_items, b.placement
                    FROM {src} b JOIN units u ON u.participant_id = b.id WHERE {carry} GROUP BY b.id, u.character_id)
              GROUP BY {to} comp_key, character_id",
    },
    Sum {
        table: "stat_comp_carry_items",
        inc: "inc_cd_carry_items",
        keys: "character_id, item",
        inc_keys: "character_id, item",
        vals: "games, sum_place",
        sql: "SELECT {to} comp_key, character_id, item, COUNT(*), SUM(placement)
              FROM (SELECT DISTINCT {ti} b.id, b.comp_key, ui.character_id, ui.item, b.placement
                    FROM {src} b JOIN unit_items ui ON ui.participant_id = b.id WHERE {carry})
              GROUP BY {to} comp_key, character_id, item",
    },
    Sum {
        // A build is three items, so a board with one is a carry board.
        table: "stat_comp_builds",
        inc: "inc_cd_builds",
        keys: "character_id, build",
        inc_keys: "character_id, build",
        vals: "games, sum_place",
        sql: "SELECT {to} comp_key, character_id, build, COUNT(*), SUM(placement)
              FROM (SELECT DISTINCT {ti} b.id, b.comp_key, u.character_id, u.build, b.placement
                    FROM {src} b JOIN units u ON u.participant_id = b.id WHERE u.build IS NOT NULL)
              GROUP BY {to} comp_key, character_id, build",
    },
    Sum {
        table: "stat_comp_holders",
        inc: "inc_cd_holders",
        keys: "carry, holder",
        inc_keys: "carry, holder",
        vals: "games, sum_place, top4",
        sql: "SELECT {to} comp_key, carry, holder, COUNT(*), SUM(placement), SUM(placement <= 4)
              FROM (SELECT DISTINCT {ti} b.id, b.comp_key, c.character_id carry, h.character_id holder, b.placement
                    FROM {src} b JOIN units c ON c.participant_id = b.id AND c.n_items = 3
                    JOIN units h ON h.participant_id = b.id AND h.rarity < c.rarity)
              GROUP BY {to} comp_key, carry, holder",
    },
];

fn fill(sql: &str, src: &str, tagged: bool) -> String {
    let (ti, to) = if tagged { ("b.band, b.w,", "band, w,") } else { ("", "") };
    sql.replace("{carry}", CARRY_BOARD).replace("{src}", src).replace("{ti}", ti).replace("{to}", to)
}

/// Removes one scope's rows (inside the caller's transaction).
pub fn clear(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    for table in TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), params![patch, scope])?;
    }
    Ok(())
}

/// A full rebuild: the listed comps' boards in `p` (labels already merged) and `topc`, after `stat_comp_items`.
pub fn build_full(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    let src = "(SELECT p.* FROM p JOIN topc USING (comp_key))";
    for s in &SUMS {
        db.execute(
            &format!(
                "INSERT INTO {} (patch, bucket, comp_key, {}, {}) SELECT ?1, ?2, * FROM ({})",
                s.table,
                s.keys,
                s.vals,
                fill(s.sql, src, false)
            ),
            params![patch, scope],
        )?;
    }
    special(db, patch, scope)
}

/// An incremental build, once per patch: the sums per rank band, time window and stored label, from `inc_b`.
pub fn inc_sums(db: &Connection) -> Result<()> {
    for s in &SUMS {
        db.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {} (band, w, comp_key, {}, {}); DELETE FROM {};",
            s.inc, s.inc_keys, s.vals, s.inc
        ))?;
        db.execute(
            &format!("INSERT INTO {} {}", s.inc, fill(s.sql, "(SELECT * FROM inc_b WHERE comp_key IS NOT NULL)", true)),
            [],
        )?;
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

/// An incremental build, per scope: the covered tags added up under their merged label (`inc_map`), listed comps
/// only (`inc_top`), after `stat_comp_items` is written.
pub fn write_inc(db: &Connection, patch: &str, scope: &str, band: i64, w: i64) -> Result<()> {
    for s in &SUMS {
        let sums: Vec<String> = s.vals.split(", ").map(|v| format!("SUM({v})")).collect();
        db.execute(
            &format!(
                "INSERT INTO {} (patch, bucket, comp_key, {}, {})
                 SELECT ?1, ?2, comp_key, {}, {}
                 FROM (SELECT COALESCE(m.dst, c.comp_key) comp_key, {}, {} FROM {} c LEFT JOIN inc_map m ON m.src = c.comp_key
                       WHERE c.band >= ?3 AND c.w >= ?4)
                 JOIN inc_top USING (comp_key) GROUP BY comp_key, {}",
                s.table,
                s.keys,
                s.vals,
                s.inc_keys,
                sums.join(", "),
                s.inc_keys,
                s.vals,
                s.inc,
                s.inc_keys
            ),
            params![patch, scope, band, w],
        )?;
    }
    special(db, patch, scope)
}

/// The item types that count as special for C9.
pub const SPECIAL_TYPES: [&str; 3] = ["emblem", "artifact", "radiant"];

/// `stat_comp_special` from `stat_comp_items`: the same counts, for special items only. The type is the stored set
/// data's, or for an item it does not list, the one `item_type` in the API gives its id (the same patterns, in the
/// same order; LIKE ignores ASCII case as the patterns do).
fn special(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    let types: Vec<String> = SPECIAL_TYPES.iter().map(|t| format!("'{t}'")).collect();
    db.execute(
        &format!(
            "INSERT INTO stat_comp_special (patch, bucket, comp_key, item, holder, games, sum_place)
             SELECT patch, bucket, comp_key, item, character_id, games, sum_place FROM stat_comp_items ci
             WHERE patch = ?1 AND bucket = ?2 AND COALESCE(
               (SELECT json_extract(sd.data, '$.type') FROM static_data sd
                WHERE sd.kind = 'item' AND sd.id = ci.item COLLATE NOCASE LIMIT 1),
               CASE WHEN ci.item LIKE '%radiant%' THEN 'radiant'
                    WHEN ci.item LIKE '%artifact%' OR ci.item LIKE '%ornn%' THEN 'artifact'
                    WHEN ci.item LIKE '%emblem%' THEN 'emblem' ELSE 'other' END) IN ({})",
            types.join(", ")
        ),
        params![patch, scope],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_stats_with, now_ms};

    /// (id, rarity, items) for one unit on a board.
    type U = (&'static str, Option<i64>, Vec<&'static str>);
    /// (comp label, placement, level, last round, units).
    type Board = (&'static str, i64, Option<i64>, Option<i64>, Vec<U>);

    fn world(name: &str, boards: &[Board]) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-comp-detail-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        for (i, (comp, place, level, round, units)) in boards.iter().enumerate() {
            let m = format!("M{}", i / 8);
            conn.execute(
                "INSERT OR IGNORE INTO matches VALUES (?, 'na1', '99.1', 99, ?, 2000, 8, NULL)",
                params![m, now_ms() - 3_600_000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO participants (match_id, puuid, placement, level, last_round, comp_key) VALUES (?, ?, ?, ?, ?, ?)",
                params![m, format!("P{i}"), place, level, round, comp],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for (unit, rarity, items) in units {
                let mut sorted = items.clone();
                sorted.sort();
                let build = (items.len() == 3).then(|| sorted.join("|"));
                conn.execute("INSERT INTO units VALUES (?, ?, 2, ?, ?, ?)", params![id, unit, items.len() as i64, build, rarity])
                    .unwrap();
                for it in items {
                    conn.execute("INSERT INTO unit_items VALUES (?, ?, ?)", params![id, unit, it]).unwrap();
                }
            }
            conn.execute("INSERT INTO traits VALUES (?, 'T', 3, 1, 1)", [id]).unwrap();
        }
        (dir, conn)
    }

    const EMBLEM_BY_ID: &str = "TFT_Item_GuardianEmblem"; // no set data: an emblem by its id
    const RADIANT_BY_ID: &str = "TFT5_Item_RageRadiant";
    const ARTIFACT_BY_DATA: &str = "TFT_Item_Odd"; // set data says artifact
    const NOT_BY_DATA: &str = "TFT_Item_LooksLikeEmblem"; // set data says craftable, which wins over the id

    /// Two comps on separate units. Comp A's carry holds three items on three boards in four; on the fourth no unit
    /// holds three, so that board counts for levelling and nothing else. A3 is doubled on some boards.
    fn boards() -> Vec<Board> {
        let carry_items = [
            vec!["I1", "I2", EMBLEM_BY_ID],
            vec!["I1", RADIANT_BY_ID, "I3"],
            vec![ARTIFACT_BY_DATA, "I2", NOT_BY_DATA],
            vec!["I1", "I2"],
        ];
        let mut out: Vec<Board> = Vec::new();
        for i in 0..40usize {
            let a = i < 24;
            let place = 1 + (i % 8) as i64;
            let level = if i % 11 == 10 { None } else { Some(7 + (i % 3) as i64) };
            let round = if i % 5 == 0 { None } else { Some(25 + (i % 10) as i64) };
            let units: Vec<U> = if a {
                let mut u: Vec<U> = vec![
                    ("A1", Some(4), carry_items[i % 4].clone()),
                    ("A2", Some(2), if i % 2 == 0 { vec!["I4"] } else { vec![] }),
                    ("A3", Some(0), if i % 3 == 0 { vec![EMBLEM_BY_ID] } else { vec![] }),
                    ("A4", Some(6), vec![]), // costs more than the carry: never a holder for it
                    ("A6", Some(4), vec![]), // costs the same as the carry: not a holder either
                    ("A5", None, vec![]),    // no rarity: never a holder
                ];
                if i % 6 == 1 {
                    u.push(("A3", Some(0), vec!["I5"])); // a second copy
                }
                u
            } else {
                vec![
                    ("B1", Some(2), vec!["I1", "I1", "I3"]), // two copies of one item
                    ("B2", Some(0), vec!["I2", "I2", "I2"]),
                    ("B3", Some(1), vec![]),
                ]
            };
            out.push((if a { "A|A1" } else { "B|B1" }, place, level, round, units));
        }
        out
    }

    fn q(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// The rows of `table` in scope ALL equal `expected` (same columns after patch and bucket), both ways.
    fn same_rows(conn: &Connection, table: &str, expected: &str) -> i64 {
        let cols: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}') WHERE name NOT IN ('patch', 'bucket')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let got = format!("SELECT {} FROM {table} WHERE bucket = 'ALL'", cols.join(", "));
        let n = q(conn, &format!("SELECT COUNT(*) FROM ({got})"));
        assert_eq!(n, q(conn, &format!("SELECT COUNT(*) FROM ({expected})")), "{table}: row count");
        let extra = q(conn, &format!("SELECT COUNT(*) FROM ({got} EXCEPT {expected})"));
        let missing = q(conn, &format!("SELECT COUNT(*) FROM ({expected} EXCEPT {got})"));
        assert_eq!((extra, missing), (0, 0), "{table}: differs from a recount");
        n
    }

    /// Every comp page stat equals a direct count over its comp's boards, written without the build's SQL.
    #[test]
    fn every_comp_page_stat_equals_a_recount_over_member_boards() {
        let (dir, mut conn) = world("recount", &boards());
        conn.execute_batch(&format!(
            "INSERT INTO static_data VALUES ('item', '{ARTIFACT_BY_DATA}', 'Odd', NULL, '{{\"type\":\"artifact\"}}');
             INSERT INTO static_data VALUES ('item', '{NOT_BY_DATA}', 'Looks', NULL, '{{\"type\":\"craftable\"}}');"
        ))
        .unwrap();
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comps WHERE bucket = 'ALL'"), 2);

        // A board is a carry board when one of its units holds three items.
        let carry = "(SELECT COUNT(*) FROM units z WHERE z.participant_id = pa.id AND z.n_items = 3) > 0";
        let n = same_rows(
            &conn,
            "stat_comp_level_bands",
            "SELECT comp_key, (placement > 2) + (placement > 4) + 1, IFNULL(level, 0), COUNT(*), SUM(placement),
               COUNT(last_round), IFNULL(SUM(last_round), 0) FROM participants GROUP BY 1, 2, 3",
        );
        assert!(n > 10, "{n} level rows");
        same_rows(
            &conn,
            "stat_comp_carry_units",
            &format!(
                "SELECT pa.comp_key, c.character_id, COUNT(*), SUM(pa.placement), SUM(c.has3), SUM(c.has3 * pa.placement),
                   SUM(c.has3 * (pa.placement <= 4))
                 FROM participants pa
                 JOIN (SELECT participant_id, character_id, MAX(n_items = 3) has3 FROM units GROUP BY 1, 2) c ON c.participant_id = pa.id
                 WHERE {carry} GROUP BY 1, 2"
            ),
        );
        same_rows(
            &conn,
            "stat_comp_carry_items",
            &format!(
                "SELECT pa.comp_key, i.character_id, i.item, COUNT(*), SUM(pa.placement)
                 FROM participants pa JOIN (SELECT DISTINCT participant_id, character_id, item FROM unit_items) i ON i.participant_id = pa.id
                 WHERE {carry} GROUP BY 1, 2, 3"
            ),
        );
        same_rows(
            &conn,
            "stat_comp_builds",
            "SELECT pa.comp_key, u.character_id, u.build, COUNT(*), SUM(pa.placement)
             FROM participants pa JOIN (SELECT DISTINCT participant_id, character_id, build FROM units WHERE n_items = 3) u
               ON u.participant_id = pa.id GROUP BY 1, 2, 3",
        );
        same_rows(
            &conn,
            "stat_comp_holders",
            "SELECT pa.comp_key, c.character_id, h.character_id, COUNT(*), SUM(pa.placement), SUM(pa.placement <= 4)
             FROM participants pa
             JOIN (SELECT DISTINCT participant_id, character_id, rarity FROM units WHERE n_items = 3) c ON c.participant_id = pa.id
             JOIN (SELECT DISTINCT participant_id, character_id, rarity FROM units) h ON h.participant_id = pa.id AND h.rarity < c.rarity
             GROUP BY 1, 2, 3",
        );
        let special = same_rows(
            &conn,
            "stat_comp_special",
            &format!(
                "SELECT pa.comp_key, i.item, i.character_id, COUNT(*), SUM(pa.placement)
                 FROM participants pa JOIN (SELECT DISTINCT participant_id, character_id, item FROM unit_items) i ON i.participant_id = pa.id
                 WHERE i.item IN ('{EMBLEM_BY_ID}', '{RADIANT_BY_ID}', '{ARTIFACT_BY_DATA}') GROUP BY 1, 2, 3"
            ),
        );
        assert_eq!(special, 4, "emblem on A1 and A3, radiant, artifact; the craftable look-alike is left out");

        // Worked by hand: comp A has 24 boards, 6 of them with no three-item unit.
        let a1 = |col: &str| q(&conn, &format!("SELECT {col} FROM stat_comp_carry_units WHERE bucket = 'ALL' AND comp_key = 'A|A1' AND character_id = 'A1'"));
        assert_eq!((a1("games"), a1("full_games")), (18, 18));
        assert_eq!(q(&conn, "SELECT SUM(games) FROM stat_comp_level_bands WHERE bucket = 'ALL' AND comp_key = 'A|A1'"), 24);
        assert_eq!(
            q(&conn, "SELECT COUNT(*) FROM stat_comp_carry_items WHERE bucket = 'ALL' AND comp_key = 'A|A1' AND character_id = 'A1' AND item = 'I1'"),
            1
        );
        assert_eq!(
            q(&conn, "SELECT games FROM stat_comp_carry_items WHERE bucket = 'ALL' AND comp_key = 'A|A1' AND character_id = 'A1' AND item = 'I1'"),
            12,
            "I1 is on A1 on 18 boards; the 6 without a three-item unit are left out"
        );
        let holders: Vec<String> = conn
            .prepare("SELECT holder FROM stat_comp_holders WHERE bucket = 'ALL' AND carry = 'A1' ORDER BY holder")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(holders, ["A2", "A3"], "cheaper units only: not A4 (dearer), A6 (same cost) or A5 (no rarity)");
        // Comp B has two carries: B2 and B3 are cheaper than B1; nothing is cheaper than B2.
        assert_eq!(q(&conn, "SELECT games FROM stat_comp_holders WHERE bucket = 'ALL' AND carry = 'B1' AND holder = 'B2'"), 16);
        assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comp_holders WHERE bucket = 'ALL' AND carry = 'B2'"), 0);
        // Every scope gets rows, and a comp too small to be listed gets none.
        assert_eq!(q(&conn, "SELECT COUNT(DISTINCT bucket) FROM stat_comp_builds"), 8);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unlisted_comps_and_boards_without_a_three_item_unit_get_no_carry_rows() {
        let mut bs = boards();
        // Comp C: 3 boards, under the 5 a comp needs to be listed.
        for i in 0..3 {
            bs.push(("C|C1", 1 + i, Some(8), Some(30), vec![("C1", Some(6), vec!["I1", "I2", "I3"]), ("C2", Some(0), vec![])]));
        }
        // Comp D: listed, but no board has a three-item unit.
        for i in 0..8 {
            bs.push(("D|D1", 1 + i, Some(8), Some(30), vec![("D1", Some(6), vec!["I1", "I2"]), ("D2", Some(0), vec!["I3"])]));
        }
        let (dir, mut conn) = world("excluded", &bs);
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        for table in TABLES {
            assert_eq!(q(&conn, &format!("SELECT COUNT(*) FROM {table} WHERE comp_key = 'C|C1'")), 0, "{table}");
        }
        assert_eq!(q(&conn, "SELECT SUM(games) FROM stat_comp_level_bands WHERE bucket = 'ALL' AND comp_key = 'D|D1'"), 8);
        for table in ["stat_comp_carry_units", "stat_comp_carry_items", "stat_comp_builds", "stat_comp_holders"] {
            assert_eq!(q(&conn, &format!("SELECT COUNT(*) FROM {table} WHERE comp_key = 'D|D1'")), 0, "{table}");
        }
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
