//! Unit and trait page depth (PRD 3 U1, U2, U3, U4, U8), from `stat_unit_dist` and the tables `tft-stats` writes
//! in `unit_depth.rs`. Place change is "average place with this, minus the unit's own average" (as the unit page's
//! items and builds already are); negative is better.
//!
//! - `/api/units/:id` gains `grid` (U1), `teammates` (U4), `recent` (the last 3 days for the trend) and `holdsItems`.
//! - `/api/units/:id/builds?comp=` (U2): three-item builds, in one comp or on all boards.
//! - `/api/units/:id/pairs?have=a,b` (U3): the best item to add to the ones held.
//! - `/api/traits/:id` (U8): one row per active breakpoint, with the usual units.

use super::{Obj, Scope, f, g, min_param, public, rate, row, rows, s, scope, with};
use crate::describe::{Kind, describe};
use crate::js::{self, num};
use anyhow::Result;
use rusqlite::{Connection, ToSql};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use tft_core::config::scope_key;
use tft_stats::unit_depth::{TEAMMATE_MIN_GAMES, no_item_patterns};

/// Teammates shown (U4).
pub const TEAMMATES_SHOWN: usize = 10;
/// Fewest three-item boards for the unit in a comp before its builds are shown for that comp (U2); below it the
/// builds come from all boards, and the answer says so.
pub const COMP_BUILD_MIN_GAMES: f64 = 50.0;
/// Builds and item suggestions listed.
pub const BUILDS_SHOWN: usize = 40;
pub const PAIRS_SHOWN: usize = 30;

/// Whether the unit can hold items: not named in `no_item_units.json` (summons and the like).
pub fn holds_items(conn: &Connection, id: &str) -> Result<bool> {
    let excluded: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM json_each(?1) WHERE ?2 LIKE value)",
        rusqlite::params![no_item_patterns(), id],
        |r| r.get(0),
    )?;
    Ok(!excluded)
}

/// Items in the current set data, lowercased, when this is the newest patch and set data is stored. An item that
/// left the game mid-patch is hidden from the current patch's builds and pairs; older patches keep it.
fn current_items(conn: &Connection, sc: &Scope) -> Result<Option<HashSet<String>>> {
    if sc.patches.first() != Some(&sc.patch) {
        return Ok(None);
    }
    let ids: HashSet<String> = rows(conn, "SELECT id FROM static_data WHERE kind = 'item'", &[])?
        .iter()
        .map(|r| s(r, "id").to_ascii_lowercase())
        .collect();
    Ok((!ids.is_empty()).then_some(ids))
}

fn known(items: &Option<HashSet<String>>, ids: &[&str]) -> bool {
    items.as_ref().is_none_or(|set| ids.iter().all(|i| set.contains(&i.to_ascii_lowercase())))
}

/// The unit's own average place in this scope, if it was played.
fn unit_avg(conn: &Connection, sc: &Scope, id: &str) -> Result<Option<(f64, f64)>> {
    Ok(row(conn, "SELECT games, sum_place FROM stat_units WHERE patch = ? AND bucket = ? AND character_id = ?", &[&sc.patch, &sc.key, &id])?
        .map(|u| (f(&u, "games"), f(&u, "sum_place") / f(&u, "games"))))
}

fn by_change(a: &Value, b: &Value) -> std::cmp::Ordering {
    let pc = |v: &Value| v.get("placeChange").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    let games = |v: &Value| v.get("games").and_then(Value::as_f64).unwrap_or(0.0);
    pc(a).partial_cmp(&pc(b)).unwrap_or(std::cmp::Ordering::Equal).then(games(b).partial_cmp(&games(a)).unwrap_or(std::cmp::Ordering::Equal))
}

/// The fields `/api/units/:id` gains, for a unit that was played (`avg` is its average place).
pub fn unit_sections(conn: &Connection, sc: &Scope, id: &str, avg: f64) -> Result<Vec<(&'static str, Value)>> {
    let k: [&dyn ToSql; 3] = [&sc.patch, &sc.key, &id];
    // U1: star 1 to 3 by 0 to 3 items. Every cell is listed, played or not.
    let mut cells = [[(0.0, 0.0, 0.0); 4]; 3];
    for d in rows(conn, "SELECT star, n_items, games, sum_place, top4 FROM stat_unit_dist WHERE patch = ? AND bucket = ? AND character_id = ?", &k)? {
        let star = (f(&d, "star") as usize).clamp(1, 3) - 1;
        let items = (f(&d, "n_items") as usize).min(3);
        let c = &mut cells[star][items];
        c.0 += f(&d, "games");
        c.1 += f(&d, "sum_place");
        c.2 += f(&d, "top4");
    }
    let mut grid = Vec::new();
    for (star, by_items) in cells.iter().enumerate() {
        for (items, (games, sum, top4)) in by_items.iter().enumerate() {
            let played = *games > 0.0;
            grid.push(json!({
                "star": star + 1, "items": items, "games": num(*games),
                "avgPlace": if played { num(sum / games) } else { Value::Null },
                "top4Rate": if played { num(top4 / games) } else { Value::Null },
            }));
        }
    }

    // U4: the 10 units it places best alongside, with 200 shared boards or more.
    let mut mates = Vec::new();
    for r in rows(
        conn,
        "SELECT other, games, sum_place FROM stat_teammates WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?
         ORDER BY sum_place * 1.0 / games, other LIMIT ?",
        &[&sc.patch, &sc.key, &id, &TEAMMATE_MIN_GAMES, &(TEAMMATES_SHOWN as i64)],
    )? {
        let place = f(&r, "sum_place") / f(&r, "games");
        mates.push(Value::Object(with(
            describe(conn, Kind::Unit, s(&r, "other"), false)?,
            &[("games", g(&r, "games")), ("avgPlace", num(place)), ("placeChange", num(place - avg))],
        )));
    }

    // Trend: the last 3 days of this patch, next to the whole patch.
    let recent = if sc.window.is_empty() {
        let d3 = scope_key(sc.bucket, "3d");
        row(
            conn,
            "SELECT su.games, su.sum_place, su.wins, su.top4, sm.comps FROM stat_units su
             JOIN stat_meta sm ON sm.patch = su.patch AND sm.bucket = su.bucket WHERE su.patch = ? AND su.bucket = ? AND su.character_id = ?",
            &[&sc.patch, &d3, &id],
        )?
        .map(|r| Value::Object(with(rate(&r, f(&r, "comps")), &[("window", json!("3d")), ("label", json!("Last 3 days"))])))
        .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    Ok(vec![
        ("grid", Value::Array(grid)),
        ("teammates", Value::Array(mates)),
        ("recent", recent),
        ("holdsItems", json!(holds_items(conn, id)?)),
    ])
}

fn items_of(conn: &Connection, build: &str) -> Result<Value> {
    let mut its = Vec::new();
    for i in build.split('|') {
        its.push(Value::Object(describe(conn, Kind::Item, i, false)?));
    }
    Ok(Value::Array(its))
}

/// GET `/api/units/:id/builds?comp=`: three-item builds by place change. With a comp, the builds on that comp's
/// boards, against the unit's average in that comp, when the unit held three items on 50 of them or more;
/// otherwise every board, with `fallback` set.
pub fn builds(conn: &Connection, q: Option<&str>, id: &str) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let comp = js::query(q, "comp").filter(|c| !c.is_empty());
    let holds = holds_items(conn, id)?;
    let mut body = with(
        public(&sc),
        &[
            ("unit", Value::Object(describe(conn, Kind::Unit, id, false)?)),
            ("comp", comp.as_ref().map_or(Value::Null, |c| json!(c))),
            ("minCompGames", num(COMP_BUILD_MIN_GAMES)),
            ("holdsItems", json!(holds)),
        ],
    );
    let Some((unit_games, avg)) = unit_avg(conn, &sc, id)?.filter(|_| holds) else {
        return Ok(with(body, &[("compGames", json!(0)), ("fallback", json!(false)), ("rows", json!([]))]));
    };
    let current = current_items(conn, &sc)?;
    let mp = min_param(&sc);

    let comp_games = match &comp {
        Some(c) => row(
            conn,
            "SELECT full_games FROM stat_comp_carry_units WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ?",
            &[&sc.patch, &sc.key, c, &id],
        )?
        .map_or(0.0, |r| f(&r, "full_games")),
        None => 0.0,
    };
    let in_comp = comp.is_some() && comp_games >= COMP_BUILD_MIN_GAMES;
    let mut out = Vec::new();
    if let (true, Some(c)) = (in_comp, &comp) {
        let base = row(
            conn,
            "SELECT games, sum_place FROM stat_comp_units WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ?",
            &[&sc.patch, &sc.key, c, &id],
        )?
        .map_or(avg, |r| f(&r, "sum_place") / f(&r, "games"));
        for r in rows(
            conn,
            "SELECT build, games, sum_place FROM stat_comp_builds WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ? AND games >= ?",
            &[&sc.patch, &sc.key, c, &id, &mp],
        )? {
            let build = s(&r, "build");
            if !known(&current, &build.split('|').collect::<Vec<_>>()) {
                continue;
            }
            let place = f(&r, "sum_place") / f(&r, "games");
            out.push(json!({ "id": build, "items": items_of(conn, build)?, "games": g(&r, "games"),
                "share": num(f(&r, "games") / comp_games), "avgPlace": num(place), "placeChange": num(place - base) }));
        }
    } else {
        for r in rows(
            conn,
            "SELECT build, games, sum_place, place_change FROM stat_unit_builds WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?",
            &[&sc.patch, &sc.key, &id, &mp],
        )? {
            let build = s(&r, "build");
            if !known(&current, &build.split('|').collect::<Vec<_>>()) {
                continue;
            }
            out.push(json!({ "id": build, "items": items_of(conn, build)?, "games": g(&r, "games"),
                "share": num(f(&r, "games") / unit_games), "avgPlace": num(f(&r, "sum_place") / f(&r, "games")),
                "placeChange": g(&r, "place_change") }));
        }
    }
    out.sort_by(by_change);
    out.truncate(BUILDS_SHOWN);
    body = with(
        body,
        &[("compGames", num(comp_games)), ("fallback", json!(comp.is_some() && !in_comp)), ("rows", Value::Array(out))],
    );
    Ok(body)
}

/// GET `/api/units/:id/pairs?have=a,b`: with up to two items held, the best item to add, by place change against
/// the unit's average. Holding none lists single items; one lists the second; two lists the third. An item already
/// held is a real choice ("x2").
pub fn pairs(conn: &Connection, q: Option<&str>, id: &str) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let mut have: Vec<String> = js::query(q, "have")
        .unwrap_or_default()
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .take(2)
        .collect();
    have.sort();
    let holds = holds_items(conn, id)?;
    let mut held = Vec::new();
    for h in &have {
        held.push(Value::Object(describe(conn, Kind::Item, h, false)?));
    }
    let body = with(
        public(&sc),
        &[("unit", Value::Object(describe(conn, Kind::Unit, id, false)?)), ("have", Value::Array(held)), ("holdsItems", json!(holds))],
    );
    let Some((_, avg)) = unit_avg(conn, &sc, id)?.filter(|_| holds) else {
        return Ok(with(body, &[("base", Value::Null), ("rows", json!([]))]));
    };
    let current = current_items(conn, &sc)?;
    let mp = min_param(&sc);
    let (p, b) = (&sc.patch, &sc.key);
    // (candidate, games, sum_place) and the held items' own row.
    let (cands, base): (Vec<Cand>, Option<(f64, f64)>) = match have.as_slice() {
        [] => (
            rows(conn, "SELECT item c, games, sum_place FROM stat_unit_items WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?", &[p, b, &id, &mp])?
                .iter()
                .map(|r| (s(r, "c").to_string(), f(r, "games"), f(r, "sum_place")))
                .collect(),
            None,
        ),
        [a] => (
            rows(
                conn,
                "SELECT CASE WHEN item_a = ?4 THEN item_b ELSE item_a END c, games, sum_place FROM stat_unit_pairs
                 WHERE patch = ?1 AND bucket = ?2 AND character_id = ?3 AND item_c = '' AND (item_a = ?4 OR item_b = ?4) AND games >= ?5",
                &[p, b, &id, a, &mp],
            )?
            .iter()
            .map(|r| (s(r, "c").to_string(), f(r, "games"), f(r, "sum_place")))
            .collect(),
            row(conn, "SELECT games, sum_place FROM stat_unit_items WHERE patch = ? AND bucket = ? AND character_id = ? AND item = ?", &[p, b, &id, a])?
                .map(|r| (f(&r, "games"), f(&r, "sum_place"))),
        ),
        [a, c, ..] => (
            rows(
                conn,
                "SELECT item_c c, games, sum_place FROM stat_unit_pairs
                 WHERE patch = ? AND bucket = ? AND character_id = ? AND item_a = ? AND item_b = ? AND item_c != '' AND games >= ?",
                &[p, b, &id, a, c, &mp],
            )?
            .iter()
            .map(|r| (s(r, "c").to_string(), f(r, "games"), f(r, "sum_place")))
            .collect(),
            row(
                conn,
                "SELECT games, sum_place FROM stat_unit_pairs WHERE patch = ? AND bucket = ? AND character_id = ? AND item_a = ? AND item_b = ? AND item_c = ''",
                &[p, b, &id, a, c],
            )?
            .map(|r| (f(&r, "games"), f(&r, "sum_place"))),
        ),
    };
    let mut out = Vec::new();
    for (c, games, sum) in cands {
        if !known(&current, &[&c]) {
            continue;
        }
        let place = sum / games;
        let copies = 1 + have.iter().filter(|h| **h == c).count();
        out.push(Value::Object(with(
            describe(conn, Kind::Item, &c, false)?,
            &[("games", num(games)), ("avgPlace", num(place)), ("placeChange", num(place - avg)), ("copies", json!(copies))],
        )));
    }
    out.sort_by(by_change);
    out.truncate(PAIRS_SHOWN);
    let base = base.map_or(Value::Null, |(games, sum)| {
        json!({ "games": num(games), "avgPlace": num(sum / games), "placeChange": num(sum / games - avg) })
    });
    Ok(with(body, &[("base", base), ("rows", Value::Array(out))]))
}

/// GET `/api/traits/:id` (U8): the trait overall, then one row per active breakpoint with avg place, pick rate and
/// the most common set of units carrying it at that count.
pub fn trait_detail(conn: &Connection, q: Option<&str>, id: &str) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let k: [&dyn ToSql; 3] = [&sc.patch, &sc.key, &id];
    let full = describe(conn, Kind::Trait, id, true)?;
    let set_data: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM static_data WHERE kind = 'unit')", [], |r| r.get(0))?;
    let rs = rows(conn, "SELECT * FROM stat_traits WHERE patch = ? AND bucket = ? AND name = ? ORDER BY tier_current", &k)?;
    let total = sc.comps as f64;
    let Some(overall) = rs.iter().find(|r| f(r, "tier_current") == 0.0) else {
        return Ok(with(public(&sc), &[("trait", Value::Object(full)), ("levels", json!([])), ("setData", json!(set_data))]));
    };
    let mut levels = Vec::new();
    for l in rs.iter().filter(|l| f(l, "tier_current") > 0.0) {
        let set = row(
            conn,
            "SELECT units, games, sum_place FROM stat_trait_sets WHERE patch = ? AND bucket = ? AND name = ? AND tier_current = ?
             ORDER BY games DESC, units LIMIT 1",
            &[&sc.patch, &sc.key, &id, &g(l, "tier_current").as_i64()],
        )?;
        let set = match set {
            Some(r) => {
                let mut units = Vec::new();
                for u in s(&r, "units").split('|') {
                    units.push(Value::Object(describe(conn, Kind::Unit, u, false)?));
                }
                json!({ "units": units, "games": g(&r, "games"), "share": num(f(&r, "games") / f(l, "games")),
                    "avgPlace": num(f(&r, "sum_place") / f(&r, "games")) })
            }
            None => Value::Null,
        };
        let mut o = Map::new();
        o.insert("id".into(), json!(format!("{id}|{}", f(l, "tier_current"))));
        o.insert("level".into(), g(l, "tier_current"));
        o.insert("units".into(), g(l, "num_units"));
        levels.push(Value::Object(with(merge_rate(o, rate(l, total)), &[("set", set)])));
    }
    let head = merge_rate(full, rate(overall, total));
    Ok(with(public(&sc), &[("trait", Value::Object(head)), ("levels", Value::Array(levels)), ("setData", json!(set_data))]))
}

/// An item to add: id, games, sum of places.
type Cand = (String, f64, f64);

fn merge_rate(mut a: Obj, b: Obj) -> Obj {
    a.extend(b);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(name: &str) -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("tft-api-unit-depth-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO stat_meta VALUES ('99.1', 'ALL', 50, 400, 1), ('99.1', 'ALL@3d', 10, 80, 1);
             INSERT INTO stat_units VALUES ('99.1', 'ALL', 'C', 200, 800, 20, 100, 2.0), ('99.1', 'ALL@3d', 'C', 40, 120, 8, 30, 2.0);
             INSERT INTO stat_unit_builds VALUES ('99.1', 'ALL', 'C', 'A|B|D', 30, 90, 5, 20, -1.0), ('99.1', 'ALL', 'C', 'A|B|E', 20, 100, 1, 8, 1.0);
             INSERT INTO stat_comp_units VALUES ('99.1', 'ALL', 'X|C', 'C', 100, 200, 290, 90, 300), ('99.1', 'ALL', 'Y|C', 'C', 60, 120, 150, 49, 240);
             INSERT INTO stat_comp_carry_units VALUES ('99.1', 'ALL', 'X|C', 'C', 95, 300, 50, 150, 40), ('99.1', 'ALL', 'Y|C', 'C', 60, 240, 49, 196, 30);
             INSERT INTO stat_comp_builds VALUES ('99.1', 'ALL', 'X|C', 'C', 'A|B|F', 20, 40), ('99.1', 'ALL', 'Y|C', 'C', 'A|B|G', 20, 20);",
        )
        .unwrap();
        (dir, conn)
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
    }

    /// U2: a comp where the unit held three items on 50 boards gets its own builds; 49 falls back to all boards.
    #[test]
    fn builds_by_comp_fall_back_to_all_boards_under_fifty_games() {
        let (dir, conn) = db("builds");
        let all = builds(&conn, Some("min=1"), "C").unwrap();
        assert_eq!(ids(&all["rows"]), ["A|B|D", "A|B|E"], "best place change first");
        assert_eq!(all["fallback"], json!(false));

        let x = builds(&conn, Some("min=1&comp=X%7CC"), "C").unwrap();
        assert_eq!(x["compGames"], json!(50.0));
        assert_eq!(x["fallback"], json!(false));
        assert_eq!(ids(&x["rows"]), ["A|B|F"]);
        // Against the unit's average in that comp (300 / 100 = 3.0): 2.0 - 3.0.
        assert_eq!(x["rows"][0]["placeChange"], json!(-1.0));

        let y = builds(&conn, Some("min=1&comp=Y%7CC"), "C").unwrap();
        assert_eq!(y["compGames"], json!(49.0));
        assert_eq!(y["fallback"], json!(true), "49 games in the comp is under 50");
        assert_eq!(ids(&y["rows"]), ["A|B|D", "A|B|E"]);
        let none = builds(&conn, Some("min=1&comp=Z%7CC"), "C").unwrap();
        assert_eq!((none["fallback"].clone(), none["compGames"].clone()), (json!(true), json!(0.0)));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn summons_get_no_builds_or_pairs() {
        let (dir, conn) = db("summons");
        conn.execute_batch(
            "INSERT INTO stat_units VALUES ('99.1', 'ALL', 'TFT99_SummonBear', 200, 800, 20, 100, 1.0);
             INSERT INTO stat_unit_builds VALUES ('99.1', 'ALL', 'TFT99_SummonBear', 'A|B|D', 30, 90, 5, 20, -1.0);",
        )
        .unwrap();
        assert!(!holds_items(&conn, "TFT99_SummonBear").unwrap() && !holds_items(&conn, "tft99_summonbear").unwrap());
        assert!(holds_items(&conn, "C").unwrap());
        let b = builds(&conn, Some("min=1"), "TFT99_SummonBear").unwrap();
        assert_eq!((b["holdsItems"].clone(), b["rows"].clone()), (json!(false), json!([])));
        let p = pairs(&conn, Some("min=1"), "TFT99_SummonBear").unwrap();
        assert_eq!(p["rows"], json!([]));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// U3: none held lists single items, one held lists pairs (a duplicate counts), two held lists third items.
    #[test]
    fn item_completer_lists_what_to_add() {
        let (dir, conn) = db("pairs");
        conn.execute_batch(
            "INSERT INTO stat_unit_items VALUES ('99.1', 'ALL', 'C', 'A', 100, 350, 0, 0, -0.5), ('99.1', 'ALL', 'C', 'B', 50, 250, 0, 0, 1.0);
             INSERT INTO stat_unit_pairs VALUES
               ('99.1', 'ALL', 'C', 'A', 'B', '', 40, 120), ('99.1', 'ALL', 'C', 'A', 'A', '', 10, 20),
               ('99.1', 'ALL', 'C', 'B', 'Z', '', 10, 60),
               ('99.1', 'ALL', 'C', 'A', 'B', 'D', 30, 90), ('99.1', 'ALL', 'C', 'A', 'B', 'B', 10, 10),
               ('99.1', 'ALL', 'C', 'A', 'B', 'E', 5, 35);",
        )
        .unwrap();
        let none = pairs(&conn, Some("min=1"), "C").unwrap();
        assert_eq!(ids(&none["rows"]), ["A", "B"]);
        let one = pairs(&conn, Some("min=1&have=A"), "C").unwrap();
        assert_eq!(ids(&one["rows"]), ["A", "B"], "A twice (2.0) beats A with B (3.0)");
        assert_eq!(one["rows"][0]["copies"], json!(2));
        assert_eq!(one["rows"][0]["placeChange"], json!(2.0 - 4.0));
        assert_eq!(one["base"]["avgPlace"], json!(3.5));
        let two = pairs(&conn, Some("min=1&have=B,A"), "C").unwrap();
        assert_eq!(ids(&two["rows"]), ["B", "D", "E"]);
        assert_eq!((two["rows"][0]["copies"].clone(), two["rows"][1]["copies"].clone()), (json!(2), json!(1)));
        assert_eq!(two["base"]["games"], json!(40.0));
        let min = pairs(&conn, Some("min=6&have=A,B"), "C").unwrap();
        assert_eq!(ids(&min["rows"]), ["B", "D"], "under the minimum games is left out");
        // Set data that no longer lists E hides it from the newest patch.
        conn.execute_batch("INSERT INTO static_data VALUES ('item', 'A', 'A', NULL, '{}'), ('item', 'B', 'B', NULL, '{}'), ('item', 'D', 'D', NULL, '{}')")
            .unwrap();
        let cur = pairs(&conn, Some("min=1&have=A,B"), "C").unwrap();
        assert_eq!(ids(&cur["rows"]), ["B", "D"]);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn grid_teammates_and_trait_breakpoints() {
        let (dir, conn) = db("grid");
        conn.execute_batch(
            "INSERT INTO stat_unit_dist VALUES ('99.1', 'ALL', 'C', 1, 0, 50, 250, 0, 10), ('99.1', 'ALL', 'C', 2, 3, 100, 300, 10, 70),
               ('99.1', 'ALL', 'C', 3, 4, 50, 100, 10, 50);
             INSERT INTO stat_teammates VALUES ('99.1', 'ALL', 'C', 'M1', 200, 600), ('99.1', 'ALL', 'C', 'M2', 300, 1200),
               ('99.1', 'ALL', 'C', 'M3', 199, 199);
             INSERT INTO stat_traits VALUES ('99.1', 'ALL', 'T', 0, 2, 120, 480, 10, 60), ('99.1', 'ALL', 'T', 1, 2, 100, 420, 8, 50),
               ('99.1', 'ALL', 'T', 2, 4, 20, 60, 2, 10);
             INSERT INTO stat_trait_sets VALUES ('99.1', 'ALL', 'T', 1, 'M1|M2', 60, 240), ('99.1', 'ALL', 'T', 1, 'C|M1', 30, 90);",
        )
        .unwrap();
        let sc = scope(&conn, None).unwrap();
        let out: Map<String, Value> = unit_sections(&conn, &sc, "C", 4.0).unwrap().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let grid = out["grid"].as_array().unwrap();
        assert_eq!(grid.len(), 12);
        assert_eq!(grid[0], json!({ "star": 1, "items": 0, "games": 50.0, "avgPlace": 5.0, "top4Rate": 0.2 }));
        assert_eq!(grid[7]["games"], json!(100.0));
        assert_eq!(grid[11]["games"], json!(50.0), "four items count as three");
        assert_eq!(grid[1]["avgPlace"], Value::Null);
        assert_eq!(ids(&out["teammates"]), ["M1", "M2"], "199 shared boards is under the 200 minimum");
        assert_eq!(out["teammates"][0]["placeChange"], json!(-1.0));
        assert_eq!(out["recent"]["games"], json!(40));
        assert_eq!(out["recent"]["frequency"], json!(0.5));

        let t = trait_detail(&conn, None, "T").unwrap();
        let levels = t["levels"].as_array().unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0]["frequency"], json!(0.25), "pick rate: 100 of 400 boards");
        assert_eq!(levels[0]["set"]["games"], json!(60));
        assert_eq!(ids(&levels[0]["set"]["units"]), ["M1", "M2"]);
        assert_eq!(levels[1]["set"], Value::Null);
        assert_eq!(trait_detail(&conn, None, "nope").unwrap()["levels"], json!([]));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
