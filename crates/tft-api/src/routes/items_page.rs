//! The item page's stats depth (PRD 3 U5, U6, U7), from the tables `tft-stats` writes in `items_depth.rs` and
//! `stat_unit_items`.
//!
//! - `GET /api/items/components`: per base component, the completed items it builds, best first by weight
//!   (place change on the S and A comps' units; negative is better). `basis` says what the weights are:
//!   `"top"` (the scope's S and A comps), `"all"` (no S or A comp listed: place change over all boards).
//! - `/api/items` rows gain `bestHolders`: the five units with the best place change holding that item, among
//!   those with the minimum games. `?type=` keeps one item type (the item page's type tabs).
//! - `/api/items/:id` gains `emblem` for an emblem: its trait, the level it is at on the boards holding it, and
//!   the breakpoint it most often completes on winning boards (1st place), or on top 4 boards when there are
//!   fewer wins than the minimum games (`basis` says which).
//!
//! An item removed from the set data mid-patch stays in old patches, and is hidden from the newest one's
//! components and type tabs. The full item list keeps it, as it always has: missing set data can also mean the
//! set data is older than the item.

use super::{Obj, Scope, f, g, rows, s, with};
use crate::describe::{Kind, describe};
use crate::js::num;
use anyhow::Result;
use rusqlite::{Connection, ToSql};
use serde_json::{Value, json};
use std::collections::HashMap;
use tft_core::order::locale_cmp;

/// Holders listed per item on the item list (U6).
pub const BEST_HOLDERS: usize = 5;

/// True when items missing from the set data should be hidden: this is the newest patch and set data is loaded.
pub fn hide_removed(conn: &Connection, sc: &Scope) -> Result<bool> {
    if sc.patches.first() != Some(&sc.patch) {
        return Ok(false);
    }
    Ok(conn.query_row("SELECT EXISTS (SELECT 1 FROM static_data WHERE kind = 'item')", [], |r| r.get(0))?)
}

/// True when the set data lists this item (letter case aside, as `describe` reads it).
pub fn in_set_data(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM static_data WHERE kind = 'item' AND id = ? COLLATE NOCASE)")?
        .query_row([id], |r| r.get(0))?)
}

/// The best holders per item: lowest place change first (then more games, then id), with the minimum games.
pub fn best_holders(conn: &Connection, sc: &Scope, min: &dyn ToSql) -> Result<HashMap<String, Vec<Value>>> {
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for h in rows(
        conn,
        "SELECT * FROM stat_unit_items WHERE patch = ? AND bucket = ? AND games >= ? AND place_change IS NOT NULL
         ORDER BY item, place_change, games DESC, character_id",
        &[&sc.patch, &sc.key, min],
    )? {
        let list = out.entry(s(&h, "item").to_string()).or_default();
        if list.len() < BEST_HOLDERS {
            let d = describe(conn, Kind::Unit, s(&h, "character_id"), false)?;
            list.push(Value::Object(with(d, &[("games", g(&h, "games")), ("placeChange", g(&h, "place_change"))])));
        }
    }
    Ok(out)
}

/// `GET /api/items/components` (U5).
pub fn components(conn: &Connection, sc: &Scope, public: Obj) -> Result<Obj> {
    let tiers = tft_stats::items_depth::top_tiers();
    let (t1, t2) = (tiers.first().cloned().unwrap_or_default(), tiers.get(1).cloned().unwrap_or_default());
    let top = rows(
        conn,
        "SELECT COUNT(*) n FROM stat_comps WHERE patch = ? AND bucket = ? AND tier IN (?, ?)",
        &[&sc.patch, &sc.key, &t1, &t2],
    )?;
    let basis = if top.first().is_some_and(|r| f(r, "n") > 0.0) { "top" } else { "all" };
    let hide = hide_removed(conn, sc)?;
    let mut by: Vec<(String, Vec<Obj>)> = Vec::new();
    for r in rows(
        conn,
        "SELECT * FROM stat_components WHERE patch = ? AND bucket = ? ORDER BY component, weight, games DESC, item",
        &[&sc.patch, &sc.key],
    )? {
        if hide && !in_set_data(conn, s(&r, "item"))? {
            continue;
        }
        let item = with(describe(conn, Kind::Item, s(&r, "item"), false)?, &[("weight", g(&r, "weight")), ("games", g(&r, "games"))]);
        match by.last_mut() {
            Some((c, items)) if c == s(&r, "component") => items.push(item),
            _ => by.push((s(&r, "component").to_string(), vec![item])),
        }
    }
    let mut out = Vec::new();
    for (c, items) in by {
        let best = items.first().map(|i| f(i, "weight")).unwrap_or(f64::INFINITY);
        let items: Vec<Value> = items.into_iter().map(Value::Object).collect();
        out.push((best, with(describe(conn, Kind::Item, &c, false)?, &[("best", num(best)), ("items", Value::Array(items))])));
    }
    // The component whose best build does best comes first; ties by name.
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| locale_cmp(s(&a.1, "name"), s(&b.1, "name"))));
    let rows: Vec<Value> = out.into_iter().map(|(_, o)| Value::Object(o)).collect();
    Ok(with(public, &[("basis", json!(basis)), ("tiers", json!(tiers)), ("rows", Value::Array(rows))]))
}

/// The `emblem` field of `/api/items/:id` (U7), or None when the item is not an emblem.
pub fn emblem(conn: &Connection, sc: &Scope, item: &Obj) -> Result<Option<Value>> {
    if item.get("type").and_then(Value::as_str) != Some("emblem") {
        return Ok(None);
    }
    let id = s(item, "id").to_string();
    let levels = rows(
        conn,
        "SELECT * FROM stat_emblem_tiers WHERE patch = ? AND bucket = ? AND item = ? ORDER BY trait, tier_current",
        &[&sc.patch, &sc.key, &id],
    )?;
    let trait_id: Option<String> = match levels.first() {
        Some(r) => Some(s(r, "trait").to_string()),
        None => conn
            .prepare_cached("SELECT json_extract(data, '$.trait') FROM static_data WHERE kind = 'item' AND id = ? COLLATE NOCASE")?
            .query_row([&id], |r| r.get(0))
            .ok()
            .flatten(),
    };
    let Some(trait_id) = trait_id else {
        return Ok(Some(json!({ "trait": null, "basis": "wins", "breakpoint": null, "levels": [] })));
    };
    let tr = describe(conn, Kind::Trait, &trait_id, false)?;
    let breakpoints: Vec<Value> = tr.get("breakpoints").and_then(Value::as_array).cloned().unwrap_or_default();
    let levels: Vec<&Obj> = levels.iter().filter(|r| s(r, "trait") == trait_id).collect();
    let units = |level: i64| -> Result<Value> {
        if level < 1 {
            return Ok(Value::Null);
        }
        if let Some(u) = breakpoints.get(level as usize - 1).and_then(|b| b.get("units")).filter(|u| u.is_number()) {
            return Ok(u.clone());
        }
        let r = rows(
            conn,
            "SELECT num_units FROM stat_traits WHERE patch = ? AND bucket = ? AND name = ? AND tier_current = ?",
            &[&sc.patch, &sc.key, &trait_id, &level],
        )?;
        Ok(r.first().map(|r| g(r, "num_units")).unwrap_or(Value::Null))
    };
    let mut out = Vec::new();
    for r in &levels {
        let games = f(r, "games");
        out.push(json!({
            "level": g(r, "tier_current"), "units": units(f(r, "tier_current") as i64)?, "games": g(r, "games"),
            "wins": g(r, "wins"), "top4": g(r, "top4"), "avgPlace": num(f(r, "sum_place") / games),
            "winRate": num(f(r, "wins") / games), "top4Rate": num(f(r, "top4") / games),
        }));
    }
    let active: Vec<&Value> = out.iter().filter(|l| l["level"].as_f64().unwrap_or(0.0) >= 1.0).collect();
    let wins: f64 = active.iter().map(|l| l["wins"].as_f64().unwrap_or(0.0)).sum();
    let need = if sc.min.is_nan() { 1.0 } else { sc.min.max(1.0) };
    let basis = if wins >= need { "wins" } else { "top4" };
    let mut best: Option<&Value> = None;
    for l in &active {
        let key = |v: &Value| (v[basis].as_f64().unwrap_or(0.0), v["games"].as_f64().unwrap_or(0.0));
        if key(l).0 <= 0.0 {
            continue;
        }
        // Most boards of that kind; then more games; then the lower level (levels come in order).
        if best.is_none_or(|b| key(l).0 > key(b).0 || (key(l).0 == key(b).0 && key(l).1 > key(b).1)) {
            best = Some(l);
        }
    }
    Ok(Some(json!({
        "trait": Value::Object(tr),
        "basis": basis,
        "breakpoint": best.cloned().unwrap_or(Value::Null),
        "levels": out,
    })))
}

#[cfg(test)]
mod tests {
    use super::super::{item, items, scope};
    use serde_json::{Value, json};

    fn db(name: &str) -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("tft-api-items-page-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO stat_meta VALUES ('99.1', 'ALL', 13, 100, 1), ('99.0', 'ALL', 13, 100, 1);
            INSERT INTO stat_comps (patch, bucket, comp_key, games, sum_place, wins, top4, sum_level, tier)
              VALUES ('99.1', 'ALL', 'X|C', 100, 450, 10, 50, 800, 'A');
            INSERT INTO stat_items VALUES ('99.1', 'ALL', 'I1', 50, 200, 5, 25, -0.5), ('99.1', 'ALL', 'Gone', 50, 200, 5, 25, -0.5),
              ('99.0', 'ALL', 'Gone', 50, 200, 5, 25, -0.5), ('99.1', 'ALL', 'E', 40, 160, 4, 20, 0);
            INSERT INTO stat_unit_items VALUES
              ('99.1', 'ALL', 'U1', 'I1', 20, 80, 2, 10, -1.0), ('99.1', 'ALL', 'U2', 'I1', 20, 80, 2, 10, -2.0),
              ('99.1', 'ALL', 'U3', 'I1', 20, 80, 2, 10, 0.5), ('99.1', 'ALL', 'U4', 'I1', 20, 80, 2, 10, -1.0),
              ('99.1', 'ALL', 'U5', 'I1', 30, 80, 2, 10, -1.0), ('99.1', 'ALL', 'U6', 'I1', 20, 80, 2, 10, 1.5),
              ('99.1', 'ALL', 'U7', 'I1', 3, 3, 3, 3, -3.0);
            INSERT INTO stat_components VALUES ('99.1', 'ALL', 'C1', 'I1', -0.4, 30), ('99.1', 'ALL', 'C1', 'I2', 0.2, 30),
              ('99.1', 'ALL', 'C2', 'I1', -0.4, 30), ('99.1', 'ALL', 'C2', 'Gone', -0.9, 30), ('99.1', 'ALL', 'C3', 'I3', -0.6, 9);
            INSERT INTO stat_emblem_tiers VALUES ('99.1', 'ALL', 'E', 'Tr', 0, 10, 60, 0, 2),
              ('99.1', 'ALL', 'E', 'Tr', 1, 20, 80, 3, 12), ('99.1', 'ALL', 'E', 'Tr', 2, 16, 50, 3, 15), ('99.1', 'ALL', 'E', 'Tr', 3, 5, 10, 2, 5);
            INSERT INTO stat_traits VALUES ('99.1', 'ALL', 'Tr', 3, 7, 5, 10, 2, 5);
            INSERT INTO static_data VALUES ('item', 'I1', 'One', NULL, '{"type":"craftable","composition":["C1","C2"]}'),
              ('item', 'I2', 'Two', NULL, '{"type":"craftable","composition":["C1","C1"]}'),
              ('item', 'I3', 'Three', NULL, '{"type":"craftable","composition":["C3","C1"]}'),
              ('item', 'C1', 'Sword', NULL, '{"type":"component"}'), ('item', 'C2', 'Bow', NULL, '{"type":"component"}'),
              ('item', 'C3', 'Rod', NULL, '{"type":"component"}'),
              ('item', 'E', 'Tr Emblem', NULL, '{"type":"emblem","trait":"Tr"}'),
              ('trait', 'Tr', 'Tr', NULL, '{"breakpoints":[{"units":2,"style":1},{"units":4,"style":2}]}');
            "#,
        )
        .unwrap();
        (dir, conn)
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn components_list_their_items_best_first_and_hide_removed_items_on_the_newest_patch() {
        let (dir, conn) = db("components");
        let sc = scope(&conn, Some("patch=99.1")).unwrap();
        let body = Value::Object(super::components(&conn, &sc, super::super::public(&sc)).unwrap());
        assert_eq!(body["basis"], json!("top"));
        assert_eq!(body["tiers"], json!(["S", "A"]));
        // C3's best build (-0.6) beats C1's and C2's (-0.4; then by name: Bow before Sword).
        assert_eq!(ids(&body["rows"]), ["C3", "C2", "C1"]);
        assert_eq!(ids(&body["rows"][2]["items"]), ["I1", "I2"]);
        assert_eq!(ids(&body["rows"][1]["items"]), ["I1"], "Gone is not in the set data any more");
        assert_eq!(body["rows"][2]["items"][0]["weight"], json!(-0.4));
        // No S or A comp: the weights are over all boards, and the answer says so.
        conn.execute("UPDATE stat_comps SET tier = 'B'", []).unwrap();
        let body = Value::Object(super::components(&conn, &sc, super::super::public(&sc)).unwrap());
        assert_eq!(body["basis"], json!("all"));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn items_list_their_five_best_holders_and_filter_by_type() {
        let (dir, conn) = db("holders");
        let body = Value::Object(items(&conn, Some("patch=99.1&min=10")).unwrap());
        let i1 = body["rows"].as_array().unwrap().iter().find(|r| r["id"] == "I1").unwrap().clone();
        // Best place change first; U4 and U5 tie on -1.0 and U5 has more games; U7 has too few games.
        assert_eq!(ids(&i1["bestHolders"]), ["U2", "U5", "U1", "U4", "U3"]);
        assert_eq!(i1["bestHolders"][0]["placeChange"], json!(-2.0));
        assert_eq!(ids(&body["rows"]), ["E", "Gone", "I1"], "the full list keeps every item, as before");
        // Gone is not in the set data: its type comes from its id ("other"). A type tab hides it on the newest patch.
        let body = Value::Object(items(&conn, Some("patch=99.1&min=10&type=other")).unwrap());
        assert_eq!(ids(&body["rows"]), Vec::<String>::new(), "Gone is hidden from the newest patch");
        let body = Value::Object(items(&conn, Some("patch=99.0&min=10&type=other")).unwrap());
        assert_eq!(ids(&body["rows"]), ["Gone"], "and stays in the old one");
        let body = Value::Object(items(&conn, Some("patch=99.1&min=10&type=emblem")).unwrap());
        assert_eq!(ids(&body["rows"]), ["E"]);
        let body = Value::Object(items(&conn, Some("patch=99.1&min=10&type=")).unwrap());
        assert_eq!(body["rows"].as_array().unwrap().len(), 3, "an empty type keeps every type");
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn emblems_give_the_breakpoint_they_complete_on_winning_boards() {
        let (dir, conn) = db("emblem");
        // 8 wins at levels 1 to 3, at least the minimum of 5: by wins. Levels 1 and 2 tie on 3 wins; level 1 has
        // more games. Level 3's units come from the trait stats (the set data lists two breakpoints).
        let body = Value::Object(item(&conn, Some("patch=99.1&min=5"), "E").unwrap());
        let e = &body["emblem"];
        assert_eq!(e["trait"]["id"], json!("Tr"));
        assert_eq!(e["basis"], json!("wins"));
        assert_eq!((e["breakpoint"]["level"].clone(), e["breakpoint"]["units"].clone()), (json!(1), json!(2)));
        let units: Vec<Value> = e["levels"].as_array().unwrap().iter().map(|l| l["units"].clone()).collect();
        assert_eq!(units, [Value::Null, json!(2), json!(4), json!(7)]);
        // Fewer wins than the minimum: by top 4 boards instead, where level 2 leads.
        let body = Value::Object(item(&conn, Some("patch=99.1&min=10"), "E").unwrap());
        assert_eq!(body["emblem"]["basis"], json!("top4"));
        assert_eq!(body["emblem"]["breakpoint"]["level"], json!(2));
        // Not an emblem: no field at all.
        assert!(Value::Object(item(&conn, Some("patch=99.1"), "I1").unwrap()).get("emblem").is_none());
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
