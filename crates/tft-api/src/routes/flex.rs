//! Flex on the comp page (PRD 2 C8): `addAt9` and `swaps` on `/api/comps/:id`, from the tables `tft-stats`
//! writes in `flex.rs`. Place change is "average place with this, minus the baseline"; negative is better.
//!
//! - **Add at 9**: a unit outside the comp's level 8 core, on its boards of level 9 or more. `freq` is the share
//!   of those boards it is on; the baseline is the comp's other boards of level 9 or more (without the unit).
//! - **Swaps**: "replace `out` with `in`": boards missing the core unit `out` that play `in`. `freq` is the share
//!   of the boards missing `out` that have `in`; the baseline is the comp's boards that have `out`.
//!
//! Minimums: a row needs [`FLEX_MIN_GAMES`] boards, and so does its baseline, or its place change says nothing.

use super::{Obj, Scope, f, rows, s, with};
use crate::describe::{Kind, describe};
use crate::js::num;
use anyhow::Result;
use rusqlite::{Connection, ToSql};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Fewest boards for an add-at-9 or swap row, and for the baseline it is compared with.
pub const FLEX_MIN_GAMES: f64 = 10.0;
/// Rows shown: units added at 9, swaps in all, and swaps for any one core unit.
pub const ADDS_SHOWN: usize = 6;
pub const SWAPS_SHOWN: usize = 8;
pub const SWAPS_PER_UNIT: usize = 2;

/// The sections, as fields to add to the comp detail body.
pub fn sections(conn: &Connection, sc: &Scope, id: &str, comp: &Obj) -> Result<Vec<(&'static str, Value)>> {
    let k: [&dyn ToSql; 3] = [&sc.patch, &sc.key, &id];
    Ok(vec![("addAt9", adds(conn, &k)?), ("swaps", swaps(conn, &k, f(comp, "games"))?)])
}

fn adds(conn: &Connection, k: &[&dyn ToSql; 3]) -> Result<Value> {
    let nine = rows(
        conn,
        "SELECT COALESCE(SUM(games), 0) games, COALESCE(SUM(sum_place), 0) sum_place FROM stat_comp_levels
         WHERE patch = ? AND bucket = ? AND comp_key = ? AND level >= 9",
        k,
    )?;
    let (all, all_sum) = nine.first().map_or((0.0, 0.0), |r| (f(r, "games"), f(r, "sum_place")));
    let mut out = Vec::new();
    for r in rows(
        conn,
        "SELECT * FROM stat_comp_flex_adds WHERE patch = ? AND bucket = ? AND comp_key = ? AND games >= ?
         ORDER BY games DESC, character_id",
        &[k[0], k[1], k[2], &FLEX_MIN_GAMES],
    )? {
        let (games, sum) = (f(&r, "games"), f(&r, "sum_place"));
        let (rest, rest_sum) = (all - games, all_sum - sum);
        if rest < FLEX_MIN_GAMES {
            continue;
        }
        let avg = sum / games;
        out.push(json!({
            "unit": describe(conn, Kind::Unit, s(&r, "character_id"), false)?,
            "games": games as i64, "freq": num(games / all), "avgPlace": num(avg),
            "placeChange": num(avg - rest_sum / rest), "baseGames": rest as i64,
        }));
        if out.len() == ADDS_SHOWN {
            break;
        }
    }
    Ok(Value::Array(out))
}

fn swaps(conn: &Connection, k: &[&dyn ToSql; 3], comp_games: f64) -> Result<Value> {
    let base: HashMap<String, (f64, f64)> =
        rows(conn, "SELECT character_id, games, sum_place FROM stat_comp_units WHERE patch = ? AND bucket = ? AND comp_key = ?", k)?
            .iter()
            .map(|u| (s(u, "character_id").to_string(), (f(u, "games"), f(u, "sum_place"))))
            .collect();
    let mut per: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for r in rows(
        conn,
        "SELECT * FROM stat_comp_swaps WHERE patch = ? AND bucket = ? AND comp_key = ? AND games >= ?
         ORDER BY games DESC, out_unit, in_unit",
        &[k[0], k[1], k[2], &FLEX_MIN_GAMES],
    )? {
        let out_unit = s(&r, "out_unit");
        let Some(&(with_out, with_sum)) = base.get(out_unit) else { continue };
        let n = per.entry(out_unit.to_string()).or_default();
        if with_out < FLEX_MIN_GAMES || *n == SWAPS_PER_UNIT {
            continue;
        }
        *n += 1;
        let (games, sum) = (f(&r, "games"), f(&r, "sum_place"));
        let avg = sum / games;
        let missing = comp_games - with_out;
        out.push(Value::Object(with(
            Obj::new(),
            &[
                ("out", Value::Object(describe(conn, Kind::Unit, out_unit, false)?)),
                ("in", Value::Object(describe(conn, Kind::Unit, s(&r, "in_unit"), false)?)),
                ("games", json!(games as i64)),
                ("freq", if missing > 0.0 { num(games / missing) } else { json!(0) }),
                ("avgPlace", num(avg)),
                ("placeChange", num(avg - with_sum / with_out)),
                ("baseGames", json!(with_out as i64)),
            ],
        )));
        if out.len() == SWAPS_SHOWN {
            break;
        }
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::super::{CompCache, comp};
    use super::*;

    fn db(name: &str) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-api-flex-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO stat_meta VALUES ('99.1', 'ALL', 13, 100, 1);
            INSERT INTO stat_comps (patch, bucket, comp_key, games, sum_place, wins, top4, sum_level) VALUES ('99.1', 'ALL', 'X|C', 100, 450, 10, 50, 850);
            INSERT INTO stat_comp_units VALUES ('99.1', 'ALL', 'X|C', 'C', 100, 200, 290, 90, 450),
              ('99.1', 'ALL', 'X|C', 'K1', 80, 160, 0, 0, 320), ('99.1', 'ALL', 'X|C', 'K2', 95, 190, 0, 0, 380),
              ('99.1', 'ALL', 'X|C', 'K3', 88, 176, 0, 0, 400), ('99.1', 'ALL', 'X|C', 'K4', 60, 120, 0, 0, 240);
            INSERT INTO stat_comp_levels VALUES ('99.1', 'ALL', 'X|C', 8, 50, 250), ('99.1', 'ALL', 'X|C', 9, 40, 160),
              ('99.1', 'ALL', 'X|C', 10, 10, 40);
            INSERT INTO stat_comp_flex_adds VALUES ('99.1', 'ALL', 'X|C', 'F1', 30, 90), ('99.1', 'ALL', 'X|C', 'F2', 12, 60),
              ('99.1', 'ALL', 'X|C', 'F3', 9, 9), ('99.1', 'ALL', 'X|C', 'F4', 45, 180);
            INSERT INTO stat_comp_swaps VALUES ('99.1', 'ALL', 'X|C', 'K1', 'S1', 15, 45), ('99.1', 'ALL', 'X|C', 'K1', 'S2', 12, 60),
              ('99.1', 'ALL', 'X|C', 'K1', 'S3', 11, 11), ('99.1', 'ALL', 'X|C', 'K2', 'S1', 9, 9),
              ('99.1', 'ALL', 'X|C', 'K3', 'S4', 10, 50), ('99.1', 'ALL', 'X|C', 'NOPE', 'S4', 30, 90);
            "#,
        )
        .unwrap();
        (dir, conn)
    }

    fn close(a: &Value, b: f64) {
        assert!((a.as_f64().unwrap() - b).abs() < 1e-12, "{a} vs {b}");
    }

    #[test]
    fn add_at_9_and_swaps_apply_their_minimums_and_baselines() {
        let (dir, conn) = db("sections");
        let body = Value::Object(comp(&conn, Some("patch=99.1"), "X|C", &CompCache::default()).unwrap());
        assert_eq!(FLEX_MIN_GAMES, 10.0);

        // Add at 9: 50 boards of level 9 or more, place sum 200. F4 (45) leaves 5 without it: under the minimum.
        // F3 has 9 boards: under it too. F1: 3.0 against (200 - 90) / 20 = 5.5. F2: 5.0 against 140 / 38.
        let adds = body["addAt9"].as_array().unwrap();
        let ids: Vec<&str> = adds.iter().map(|a| a["unit"]["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["F1", "F2"]);
        close(&adds[0]["freq"], 30.0 / 50.0);
        close(&adds[0]["placeChange"], 3.0 - 5.5);
        assert_eq!(adds[0]["baseGames"], json!(20));
        close(&adds[1]["placeChange"], 5.0 - 140.0 / 38.0);

        // Swaps: K1 is on 80 of 100 boards (avg 4.0), so 20 miss it. Two per core unit: S3 (11) is cut. K2's only
        // swap has 9 boards. K3's has 10 (a minimum is inclusive). NOPE is not a unit of this comp.
        let sw = body["swaps"].as_array().unwrap();
        let pairs: Vec<(&str, &str)> = sw.iter().map(|x| (x["out"]["id"].as_str().unwrap(), x["in"]["id"].as_str().unwrap())).collect();
        assert_eq!(pairs, [("K1", "S1"), ("K1", "S2"), ("K3", "S4")]);
        close(&sw[0]["freq"], 15.0 / 20.0);
        close(&sw[0]["placeChange"], 3.0 - 4.0);
        assert_eq!(sw[0]["baseGames"], json!(80));
        close(&sw[2]["placeChange"], 5.0 - 400.0 / 88.0);
        close(&sw[2]["freq"], 10.0 / 12.0);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_comp_with_no_rows_gets_empty_lists() {
        let (dir, conn) = db("empty");
        conn.execute_batch("DELETE FROM stat_comp_flex_adds; DELETE FROM stat_comp_swaps; DELETE FROM stat_comp_levels;").unwrap();
        let body = Value::Object(comp(&conn, Some("patch=99.1"), "X|C", &CompCache::default()).unwrap());
        assert_eq!((body["addAt9"].clone(), body["swaps"].clone()), (json!([]), json!([])));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
