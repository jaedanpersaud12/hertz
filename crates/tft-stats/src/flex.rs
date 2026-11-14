//! Flex (PRD 2 C8, comp page "Flex: add at 9, swap options"): which units a listed comp adds on level 9, and
//! which unit it plays in place of a core one. Tables from migration 6 in `tft_store::schema::MIGRATIONS`.
//!
//! Both are counts over the boards of one comp in one scope, after look-alike labels are merged:
//! - **Core**: the units on at least half of the comp's boards (`stat_comp_units`), the rule the comp's board
//!   on the tier list uses.
//! - **Level 8 core**: the units on at least half of the comp's level 8 boards, when it has at least
//!   [`LEVEL8_MIN_GAMES`] of them. A comp with fewer (a reroll comp that rarely reaches 8, or one that always
//!   goes to 9) uses its core instead.
//! - **Added at 9** (`stat_comp_flex_adds`): per unit not in the level 8 core, the boards of level 9 or more
//!   that have it, with their place sum. The place change baseline (boards of level 9 or more without the
//!   unit) is `stat_comp_levels` minus this row, worked out by the API.
//! - **Swaps** (`stat_comp_swaps`): per core unit `out_unit` and non-core unit `in_unit`, the boards that do not
//!   have `out_unit` and do have `in_unit`. The baseline (boards that have `out_unit`) is its
//!   `stat_comp_units` row.
//!
//! Every row is kept, however few games it has; the API applies the minimums (see `routes/flex.rs` in
//! `tft-api`). The core depends on the whole scope, so these are not added up per tag: they are counted over the
//! scope's boards after `stat_comp_units` and `stat_comp_levels` are written. The SQL below is the definition
//! (the reference build runs it on `p`); [`write_fast`] counts the same rows in memory from the patch's boards,
//! because the SQL's self-joins took a quarter of a build at a million boards.

use crate::onepass::{NO_COMP, Names, PatchBoards};
use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};

/// The tables this module writes, per patch and scope.
pub const TABLES: [&str; 2] = ["stat_comp_flex_adds", "stat_comp_swaps"];

/// Fewest level 8 boards a comp needs for its level 8 core to come from them rather than from all its boards.
pub const LEVEL8_MIN_GAMES: i64 = 10;

/// The scope's boards of listed comps (`b`), one row per board and unit on it (`bu`), the core and the level 8
/// core. `{src}` is the board query: id, placement, level and merged comp_key.
const WITH: &str = "
WITH b AS ({src}),
bu AS (SELECT DISTINCT b.id, b.comp_key, b.placement, b.level, u.character_id FROM b JOIN units u ON u.participant_id = b.id),
core AS (SELECT cu.comp_key, cu.character_id FROM stat_comp_units cu
         JOIN stat_comps c ON c.patch = cu.patch AND c.bucket = cu.bucket AND c.comp_key = cu.comp_key
         WHERE cu.patch = ?1 AND cu.bucket = ?2 AND cu.games * 2 >= c.games),
n8 AS (SELECT comp_key, COUNT(*) n FROM b WHERE level = 8 GROUP BY comp_key HAVING COUNT(*) >= {min8}),
core8 AS (SELECT bu.comp_key, bu.character_id FROM bu JOIN n8 USING (comp_key) WHERE bu.level = 8
          GROUP BY bu.comp_key, bu.character_id HAVING COUNT(*) * 2 >= MAX(n8.n)
          UNION SELECT comp_key, character_id FROM core WHERE comp_key NOT IN (SELECT comp_key FROM n8))";

const ADDS: &str = "
INSERT INTO stat_comp_flex_adds (patch, bucket, comp_key, character_id, games, sum_place)
{with}
SELECT ?1, ?2, bu.comp_key, bu.character_id, COUNT(*), SUM(bu.placement) FROM bu
WHERE bu.level >= 9 AND NOT EXISTS (SELECT 1 FROM core8 k WHERE k.comp_key = bu.comp_key AND k.character_id = bu.character_id)
GROUP BY bu.comp_key, bu.character_id";

const SWAPS: &str = "
INSERT INTO stat_comp_swaps (patch, bucket, comp_key, out_unit, in_unit, games, sum_place)
{with}
SELECT ?1, ?2, b.comp_key, core.character_id, bu.character_id, COUNT(*), SUM(b.placement)
FROM b JOIN core ON core.comp_key = b.comp_key JOIN bu ON bu.id = b.id
WHERE NOT EXISTS (SELECT 1 FROM units u WHERE u.participant_id = b.id AND u.character_id = core.character_id)
  AND NOT EXISTS (SELECT 1 FROM core k WHERE k.comp_key = b.comp_key AND k.character_id = bu.character_id)
GROUP BY b.comp_key, core.character_id, bu.character_id";

fn write(db: &Connection, src: &str, p: &[&dyn rusqlite::ToSql]) -> Result<()> {
    let with = WITH.replace("{src}", src).replace("{min8}", &LEVEL8_MIN_GAMES.to_string());
    for sql in [ADDS, SWAPS] {
        db.execute(&sql.replace("{with}", &with), p)?;
    }
    Ok(())
}

/// Removes one scope's rows (inside the caller's transaction).
pub fn clear(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    for table in TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE patch = ? AND bucket = ?"), params![patch, scope])?;
    }
    Ok(())
}

/// A full rebuild: the listed comps' boards in `p` (labels already merged) and `topc`.
pub fn build_full(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    write(db, "SELECT p.id, p.placement, p.level, p.comp_key FROM p JOIN topc USING (comp_key)", params![patch, scope])
}

/// Which boards a scope's flex rows count: (position in the patch's boards, comp) pairs and the comps' labels.
pub(crate) struct Members {
    pub labels: Vec<String>,
    pub boards: Vec<(usize, u32)>,
}

impl Members {
    /// An incremental build with the baseline comp method: the scope's boards (band and window tags), under
    /// their merged label (`inc_map`), listed comps only (`inc_top`).
    pub fn from_tags(db: &Connection, pb: &PatchBoards, band: i64, w: i64) -> Result<Members> {
        let top: Vec<String> = db.prepare("SELECT comp_key FROM inc_top")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let slot: HashMap<&str, u32> = top.iter().enumerate().map(|(i, k)| (k.as_str(), i as u32)).collect();
        let map: HashMap<String, String> = db
            .prepare("SELECT src, dst FROM inc_map")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        // Stored label to listed comp, once per label.
        let of: Vec<Option<u32>> =
            pb.comps.names.iter().map(|k| slot.get(map.get(k).unwrap_or(k).as_str()).copied()).collect();
        let boards = (0..pb.len())
            .filter(|i| pb.comp[*i] != NO_COMP && pb.band[*i] as i64 >= band && pb.w[*i] as i64 >= w)
            .filter_map(|i| of[pb.comp[i] as usize].map(|s| (i, s)))
            .collect();
        drop(slot);
        Ok(Members { labels: top, boards })
    }

    /// The listed comps' boards in `p` (labels already merged) and `topc`, as [`build_full`] reads them.
    pub fn from_p(db: &Connection, pb: &PatchBoards) -> Result<Members> {
        let mut labels = Names::default();
        let mut boards = Vec::new();
        let mut st = db.prepare("SELECT p.id, p.comp_key FROM p JOIN topc USING (comp_key)")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let Some(i) = pb.find(id) else { anyhow::bail!("board {id} in p is not one of the patch's boards") };
            boards.push((i, labels.id(r.get_ref(1)?.as_str()?)));
        }
        Ok(Members { labels: labels.names, boards })
    }
}

/// [`build_full`] counted in memory: the same rows as [`ADDS`] and [`SWAPS`] give for the boards in `members`,
/// after `stat_comp_units` and `stat_comps` are written for the scope.
pub(crate) fn write_fast(db: &Connection, patch: &str, scope: &str, pb: &PatchBoards, members: &Members) -> Result<()> {
    let slots = members.labels.len();
    let slot: HashMap<&str, usize> = members.labels.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    // Unit ids: the patch's, plus any core unit that is on none of its boards (numbered after them).
    let mut extra: Vec<String> = Vec::new();
    let mut core: Vec<HashSet<u32>> = vec![HashSet::new(); slots];
    {
        let mut st = db.prepare(
            "SELECT cu.comp_key, cu.character_id FROM stat_comp_units cu
             JOIN stat_comps c ON c.patch = cu.patch AND c.bucket = cu.bucket AND c.comp_key = cu.comp_key
             WHERE cu.patch = ?1 AND cu.bucket = ?2 AND cu.games * 2 >= c.games",
        )?;
        let mut rows = st.query(params![patch, scope])?;
        while let Some(r) = rows.next()? {
            let Some(s) = slot.get(r.get_ref(0)?.as_str()?) else { continue };
            let name = r.get_ref(1)?.as_str()?;
            let unit = pb.units.get(name).unwrap_or_else(|| {
                let at = extra.iter().position(|x| x == name).unwrap_or_else(|| {
                    extra.push(name.to_string());
                    extra.len() - 1
                });
                (pb.units.names.len() + at) as u32
            });
            core[*s].insert(unit);
        }
    }
    let unit_name = |u: u32| -> &str {
        let known = pb.units.names.len();
        if (u as usize) < known { pb.units.name(u) } else { &extra[u as usize - known] }
    };

    // The level 8 core: units on at least half of a comp's level 8 boards, when it has enough of them.
    let mut n8 = vec![0i64; slots];
    let mut on8: Vec<HashMap<u32, i64>> = vec![HashMap::new(); slots];
    for (i, s) in &members.boards {
        if pb.level[*i] == Some(8) {
            n8[*s as usize] += 1;
            for u in pb.units_on(*i) {
                *on8[*s as usize].entry(*u).or_insert(0) += 1;
            }
        }
    }
    let core8: Vec<HashSet<u32>> = (0..slots)
        .map(|s| {
            if n8[s] >= LEVEL8_MIN_GAMES {
                on8[s].iter().filter(|(_, n)| **n * 2 >= n8[s]).map(|(u, _)| *u).collect()
            } else {
                core[s].clone()
            }
        })
        .collect();

    let mut adds: HashMap<(u32, u32), (i64, i64)> = HashMap::new();
    let mut swaps: HashMap<(u32, u32, u32), (i64, i64)> = HashMap::new();
    let mut out: Vec<u32> = Vec::new();
    for (i, s) in &members.boards {
        let (on, place) = (pb.units_on(*i), pb.place[*i]);
        let (core, core8) = (&core[*s as usize], &core8[*s as usize]);
        if pb.level[*i].is_some_and(|l| l >= 9) {
            for u in on.iter().filter(|u| !core8.contains(u)) {
                let e = adds.entry((*s, *u)).or_insert((0, 0));
                e.0 += 1;
                e.1 += place;
            }
        }
        out.clear();
        out.extend(core.iter().filter(|c| !on.contains(c)));
        if out.is_empty() {
            continue;
        }
        for u in on.iter().filter(|u| !core.contains(u)) {
            for o in &out {
                let e = swaps.entry((*s, *o, *u)).or_insert((0, 0));
                e.0 += 1;
                e.1 += place;
            }
        }
    }

    let mut put = db.prepare(
        "INSERT INTO stat_comp_flex_adds (patch, bucket, comp_key, character_id, games, sum_place) VALUES (?, ?, ?, ?, ?, ?)",
    )?;
    for ((s, u), (games, sum)) in &adds {
        put.execute(params![patch, scope, members.labels[*s as usize], unit_name(*u), games, sum])?;
    }
    let mut put = db.prepare(
        "INSERT INTO stat_comp_swaps (patch, bucket, comp_key, out_unit, in_unit, games, sum_place) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )?;
    for ((s, o, u), (games, sum)) in &swaps {
        put.execute(params![patch, scope, members.labels[*s as usize], unit_name(*o), unit_name(*u), games, sum])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_stats_with, now_ms};

    /// (comp label, placement, level, units).
    type Board = (&'static str, i64, i64, Vec<&'static str>);

    fn world(name: &str, boards: &[Board]) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-flex-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        for (i, (comp, place, level, units)) in boards.iter().enumerate() {
            let m = format!("M{}", i / 8);
            conn.execute(
                "INSERT OR IGNORE INTO matches VALUES (?, 'na1', '99.1', 99, ?, 2000, 8, NULL)",
                params![m, now_ms() - 3_600_000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, ?, ?)",
                params![m, format!("P{i}"), place, level, comp],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for u in units {
                conn.execute("INSERT INTO units VALUES (?, ?, 2, 0, NULL, 1)", params![id, u]).unwrap();
            }
            conn.execute("INSERT INTO traits VALUES (?, 'T', 3, 1, 1)", [id]).unwrap();
        }
        (dir, conn)
    }

    /// Comp A: 40 boards. Core A1 to A4 (on every board but the swaps). Level 8 boards (18) also all have A5, so
    /// A5 is in the level 8 core but not the core (18 of 40). Level 9 boards (18) add A6 on half; level 10 boards (4) add A7.
    /// On 6 boards A4 is missing and A8 is there instead; on 2 of those A3 is missing too and A9 is there.
    /// Comp R: 30 boards, all level 7 (no level 8 core), with 6 at level 9 that add R5.
    fn boards() -> Vec<Board> {
        let mut out: Vec<Board> = Vec::new();
        for i in 0..40usize {
            let level = if i < 18 { 8 } else if i < 36 { 9 } else { 10 };
            let mut units = vec!["A1", "A2", "A3", "A4"];
            if level == 8 {
                units.push("A5");
            }
            if level == 9 && i % 2 == 0 {
                units.push("A6");
            }
            if level == 10 {
                units.push("A7");
            }
            if i % 7 == 3 {
                units.retain(|u| *u != "A4");
                units.push("A8");
                if i % 2 == 0 {
                    units.retain(|u| *u != "A3");
                    units.push("A9");
                }
            }
            out.push(("A|A1", 1 + (i % 8) as i64, level, units));
        }
        for i in 0..30usize {
            let level = if i < 24 { 7 } else { 9 };
            let mut units = vec!["R1", "R2", "R3", "R4"];
            if level == 9 {
                units.push("R5");
            }
            out.push(("R|R1", 1 + ((i * 3) % 8) as i64, level, units));
        }
        out
    }

    fn q(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn strings(conn: &Connection, sql: &str) -> Vec<String> {
        conn.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    /// The rows of `table` in scope ALL equal `expected`, both ways. Returns the row count.
    fn same_rows(conn: &Connection, table: &str, cols: &str, expected: &str) -> i64 {
        let got = format!("SELECT {cols} FROM {table} WHERE bucket = 'ALL'");
        let n = q(conn, &format!("SELECT COUNT(*) FROM ({got})"));
        assert_eq!(n, q(conn, &format!("SELECT COUNT(*) FROM ({expected})")), "{table}: row count");
        let extra = q(conn, &format!("SELECT COUNT(*) FROM ({got} EXCEPT {expected})"));
        let missing = q(conn, &format!("SELECT COUNT(*) FROM ({expected} EXCEPT {got})"));
        assert_eq!((extra, missing), (0, 0), "{table}: differs from a recount");
        n
    }

    /// Every flex and swap row equals a direct count over the comp's boards, with the cores written out by hand.
    #[test]
    fn flex_and_swap_rows_equal_a_recount_over_member_boards() {
        let (dir, mut conn) = world("recount", &boards());
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(strings(&conn, "SELECT comp_key FROM stat_comps WHERE bucket = 'ALL' ORDER BY comp_key"), ["A|A1", "R|R1"]);

        // Cores by hand: A1-A4 for A (A4 is on 34 of 40, A3 on 37), R1-R4 for R. Level 8 core: A1-A5 for A
        // (A4 on 15 of 18 level 8 boards); R has no level 8 boards, so its core.
        conn.execute_batch(
            "CREATE TEMP TABLE hand_core (comp_key, character_id);
             INSERT INTO hand_core VALUES ('A|A1','A1'),('A|A1','A2'),('A|A1','A3'),('A|A1','A4'),
               ('R|R1','R1'),('R|R1','R2'),('R|R1','R3'),('R|R1','R4');
             CREATE TEMP TABLE hand_core8 (comp_key, character_id);
             INSERT INTO hand_core8 SELECT * FROM hand_core; INSERT INTO hand_core8 VALUES ('A|A1','A5');",
        )
        .unwrap();
        let adds = same_rows(
            &conn,
            "stat_comp_flex_adds",
            "comp_key, character_id, games, sum_place",
            "SELECT pa.comp_key, u.character_id, COUNT(*), SUM(pa.placement) FROM participants pa
             JOIN units u ON u.participant_id = pa.id
             WHERE pa.level >= 9 AND (pa.comp_key, u.character_id) NOT IN (SELECT * FROM hand_core8) GROUP BY 1, 2",
        );
        assert_eq!(adds, 5, "A6, A7, A8 and A9 on comp A's level 9+ boards, R5 on R's");
        let swaps = same_rows(
            &conn,
            "stat_comp_swaps",
            "comp_key, out_unit, in_unit, games, sum_place",
            "SELECT pa.comp_key, k.character_id, u.character_id, COUNT(*), SUM(pa.placement) FROM participants pa
             JOIN hand_core k ON k.comp_key = pa.comp_key JOIN units u ON u.participant_id = pa.id
             WHERE k.character_id NOT IN (SELECT character_id FROM units x WHERE x.participant_id = pa.id)
               AND (pa.comp_key, u.character_id) NOT IN (SELECT * FROM hand_core)
             GROUP BY 1, 2, 3",
        );
        assert!(swaps >= 4, "{swaps} swap rows");

        // Worked by hand. A6 is on the 9 even level 9 boards; A7 on the 4 level 10 boards (9 or more counts).
        let add = |u: &str| q(&conn, &format!("SELECT games FROM stat_comp_flex_adds WHERE bucket = 'ALL' AND character_id = '{u}'"));
        assert_eq!((add("A6"), add("A7"), add("R5")), (9, 4, 6));
        assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comp_flex_adds WHERE character_id = 'A5'"), 0, "in the level 8 core");
        // A4 is missing on 6 boards (i = 3, 10, 17, 24, 31, 38), each with A8; A3 also on the even ones (10, 24, 38).
        let swap = |o: &str, i: &str| {
            q(&conn, &format!("SELECT COALESCE(SUM(games), 0) FROM stat_comp_swaps WHERE bucket = 'ALL' AND out_unit = '{o}' AND in_unit = '{i}'"))
        };
        assert_eq!((swap("A4", "A8"), swap("A3", "A9"), swap("A3", "A8"), swap("A4", "A9")), (6, 3, 3, 3));
        assert_eq!(swap("A4", "A3"), 0, "a core unit is never the one played instead");
        // Every scope that lists the comps has rows.
        assert_eq!(q(&conn, "SELECT COUNT(DISTINCT bucket) FROM stat_comp_swaps"), 8);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The level 8 core needs LEVEL8_MIN_GAMES level 8 boards; with fewer, the comp's core is the baseline, so a
    /// unit on every level 8 board but under half of all boards counts as added at 9.
    #[test]
    fn a_comp_with_few_level_8_boards_uses_its_core() {
        let make = |n8: usize| -> Vec<Board> {
            (0..30)
                .map(|i| {
                    let level = if i < n8 { 8 } else { 9 };
                    let mut units = vec!["C1", "C2", "C3"];
                    if level == 8 || i % 5 == 0 {
                        units.push("C4"); // on every level 8 board, and on some level 9 ones
                    }
                    ("C|C1", 1 + (i % 8) as i64, level, units)
                })
                .collect()
        };
        let c4 = |n8: usize| {
            let (dir, mut conn) = world(&format!("min8-{n8}"), &make(n8));
            build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
            let n = q(&conn, "SELECT COUNT(*) FROM stat_comp_flex_adds WHERE bucket = 'ALL' AND character_id = 'C4'");
            drop(conn);
            std::fs::remove_dir_all(&dir).unwrap();
            n
        };
        assert_eq!(LEVEL8_MIN_GAMES, 10);
        assert_eq!(c4(9), 1, "9 level 8 boards: under the minimum, C4 (on 13 of 30 boards) is not core, so it is added");
        assert_eq!(c4(10), 0, "10 level 8 boards: C4 is in the level 8 core");
    }
}
