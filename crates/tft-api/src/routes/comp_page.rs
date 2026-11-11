//! The comp page sections of `/api/comps/:id` (PRD 2 C5, C6, C7, C9), from the tables `tft-stats` writes in
//! `comp_detail.rs`. Place change is "average place with this, minus the baseline"; negative is better.

use super::{Obj, Scope, f, g, rows, s, with};
use crate::describe::{Kind, describe};
use crate::js::num;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, ToSql};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Fewest games for a build to be listed.
pub const BUILD_MIN_GAMES: f64 = 5.0;
/// Builds shown per carry.
pub const BUILDS_SHOWN: usize = 3;
/// Fewest games for a single item to be ranked, and how many are shown.
pub const ITEM_MIN_GAMES: f64 = 10.0;
pub const ITEMS_SHOWN: usize = 5;
/// Fewest games a holder needs alongside the carry, and how many are shown per carry.
pub const HOLDER_MIN_GAMES: f64 = 5.0;
pub const HOLDERS_SHOWN: usize = 6;
/// Fewest games for a special item on a holder (PRD 2 C9), and how many are shown.
pub const SPECIAL_MIN_GAMES: f64 = 30.0;
pub const SPECIAL_SHOWN: usize = 12;

/// Placement bands of the levelling guide, as `stat_comp_level_bands.band` numbers them.
const BANDS: [(i64, &str); 3] = [(1, "1st–2nd"), (2, "3rd–4th"), (3, "5th–8th")];

/// The sections, as fields to add to the comp detail body.
pub fn sections(conn: &Connection, sc: &Scope, id: &str, comp: &Obj) -> Result<Vec<(&'static str, Value)>> {
    let k: [&dyn ToSql; 3] = [&sc.patch, &sc.key, &id];
    let carries: Vec<String> = comp
        .get("carries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    Ok(vec![
        ("levelling", levelling(conn, &k)?),
        ("carries", carry_items(conn, &k, &carries)?),
        ("holders", holders(conn, &k, &carries)?),
        ("special", special(conn, &k, f(comp, "games"), f(comp, "avgPlace"))?),
    ])
}

/// C5: per placement band, the final level split, the most common final level and the average last round.
/// Estimated: Riot reports the level a board ended on, not when it got there.
fn levelling(conn: &Connection, k: &[&dyn ToSql; 3]) -> Result<Value> {
    let all = rows(
        conn,
        "SELECT * FROM stat_comp_level_bands WHERE patch = ? AND bucket = ? AND comp_key = ? ORDER BY band, level",
        k,
    )?;
    let mut out = Vec::new();
    for (band, label) in BANDS {
        let mine: Vec<&Obj> = all.iter().filter(|r| f(r, "band") == band as f64).collect();
        let games: f64 = mine.iter().map(|r| f(r, "games")).sum();
        if games == 0.0 {
            continue;
        }
        let known: Vec<&&Obj> = mine.iter().filter(|r| f(r, "level") > 0.0).collect();
        let known_games: f64 = known.iter().map(|r| f(r, "games")).sum();
        let levels: Vec<Value> = known
            .iter()
            .map(|r| json!({ "level": g(r, "level"), "games": g(r, "games"), "share": num(f(r, "games") / known_games) }))
            .collect();
        // Most games; a tie goes to the lower level (the rows are in level order).
        let common = known.iter().fold(None::<&&Obj>, |best, r| match best {
            Some(b) if f(b, "games") >= f(r, "games") => Some(b),
            _ => Some(r),
        });
        let rounds: f64 = mine.iter().map(|r| f(r, "rounds")).sum();
        let sum_round: f64 = mine.iter().map(|r| f(r, "sum_round")).sum();
        let sum_place: f64 = mine.iter().map(|r| f(r, "sum_place")).sum();
        out.push(json!({
            "band": band, "label": label, "games": games as i64, "avgPlace": num(sum_place / games),
            "commonLevel": common.map(|r| g(r, "level")).unwrap_or(Value::Null),
            "avgRound": if rounds > 0.0 { num(sum_round / rounds) } else { Value::Null },
            "levels": levels,
        }));
    }
    Ok(Value::Array(out))
}

/// C6: per carry, its three most played builds and its best single items, on boards with a three-item unit.
/// Builds compare with the carry holding any three items; single items with the carry on the board at all.
fn carry_items(conn: &Connection, k: &[&dyn ToSql; 3], carries: &[String]) -> Result<Value> {
    let mut out = Vec::new();
    for carry in carries {
        let kc: [&dyn ToSql; 4] = [k[0], k[1], k[2], carry];
        let Some(base) = rows(
            conn,
            "SELECT * FROM stat_comp_carry_units WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ?",
            &kc,
        )?
        .into_iter()
        .next() else {
            continue;
        };
        let (full, full_avg) = (f(&base, "full_games"), f(&base, "full_sum_place") / f(&base, "full_games"));
        let (on, on_avg) = (f(&base, "games"), f(&base, "sum_place") / f(&base, "games"));
        let mut builds = Vec::new();
        for b in rows(
            conn,
            "SELECT * FROM stat_comp_builds WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ? AND games >= ?
             ORDER BY games DESC, sum_place, build LIMIT ?",
            &[k[0], k[1], k[2], carry, &BUILD_MIN_GAMES, &(BUILDS_SHOWN as i64)],
        )? {
            let mut items = Vec::new();
            for item in s(&b, "build").split('|') {
                items.push(Value::Object(describe(conn, Kind::Item, item, false)?));
            }
            let avg = f(&b, "sum_place") / f(&b, "games");
            builds.push(json!({
                "items": items, "games": g(&b, "games"), "freq": num(f(&b, "games") / full),
                "avgPlace": num(avg), "placeChange": num(avg - full_avg),
            }));
        }
        let mut items = Vec::new();
        for i in rows(
            conn,
            "SELECT * FROM stat_comp_carry_items WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ? AND games >= ?
             ORDER BY sum_place * 1.0 / games, games DESC, item LIMIT ?",
            &[k[0], k[1], k[2], carry, &ITEM_MIN_GAMES, &(ITEMS_SHOWN as i64)],
        )? {
            let avg = f(&i, "sum_place") / f(&i, "games");
            items.push(Value::Object(with(
                describe(conn, Kind::Item, s(&i, "item"), false)?,
                &[
                    ("games", g(&i, "games")),
                    ("freq", num(f(&i, "games") / on)),
                    ("avgPlace", num(avg)),
                    ("placeChange", num(avg - on_avg)),
                ],
            )));
        }
        out.push(json!({
            "unit": describe(conn, Kind::Unit, carry, false)?, "games": g(&base, "full_games"),
            "builds": builds, "items": items,
        }));
    }
    Ok(Value::Array(out))
}

/// A unit's role from the set data (e.g. "APCaster"), which says what class of items it uses.
fn role(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached("SELECT json_extract(data, '$.role') FROM static_data WHERE kind = 'unit' AND id = ? COLLATE NOCASE")?
        .query_row([id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
        .filter(|r| !r.is_empty()))
}

fn traits_of(u: &Obj) -> Vec<String> {
    u.get("traits").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// C7: per carry, cheaper units that share a trait or a role with it, ranked by how often they were on the
/// carry's top 4 boards. Without set data for the carry nothing can be compared, so every cheaper unit is listed.
fn holders(conn: &Connection, k: &[&dyn ToSql; 3], carries: &[String]) -> Result<Value> {
    let mut out = Vec::new();
    for carry in carries {
        let kc: [&dyn ToSql; 4] = [k[0], k[1], k[2], carry];
        let Some(base) = rows(
            conn,
            "SELECT * FROM stat_comp_carry_units WHERE patch = ? AND bucket = ? AND comp_key = ? AND character_id = ?",
            &kc,
        )?
        .into_iter()
        .next() else {
            continue;
        };
        let top4 = f(&base, "full_top4");
        let me = describe(conn, Kind::Unit, carry, false)?;
        let my_traits: HashSet<String> = traits_of(&me).into_iter().collect();
        let my_role = role(conn, carry)?;
        let known = !my_traits.is_empty() || my_role.is_some();
        let mut units = Vec::new();
        for h in rows(
            conn,
            "SELECT * FROM stat_comp_holders WHERE patch = ? AND bucket = ? AND comp_key = ? AND carry = ? AND games >= ?
             ORDER BY top4 DESC, games DESC, holder",
            &[k[0], k[1], k[2], carry, &HOLDER_MIN_GAMES],
        )? {
            let d = describe(conn, Kind::Unit, s(&h, "holder"), false)?;
            let shared: Vec<String> = traits_of(&d).into_iter().filter(|t| my_traits.contains(t)).collect();
            let same_role = my_role.is_some() && role(conn, s(&h, "holder"))? == my_role;
            if known && shared.is_empty() && !same_role {
                continue;
            }
            let avg = f(&h, "sum_place") / f(&h, "games");
            units.push(Value::Object(with(
                d,
                &[
                    ("games", g(&h, "games")),
                    ("top4", g(&h, "top4")),
                    ("top4Share", if top4 > 0.0 { num(f(&h, "top4") / top4) } else { json!(0) }),
                    ("avgPlace", num(avg)),
                    ("sharedTraits", json!(shared)),
                    ("sameRole", json!(same_role)),
                ],
            )));
            if units.len() == HOLDERS_SHOWN {
                break;
            }
        }
        out.push(json!({ "carry": me, "games": g(&base, "full_games"), "top4": g(&base, "full_top4"), "units": units }));
    }
    Ok(Value::Array(out))
}

/// C9: emblems, artifacts and radiant items on this comp, per holder, with at least [`SPECIAL_MIN_GAMES`] games,
/// best place change first. Place change compares with the comp's average place.
fn special(conn: &Connection, k: &[&dyn ToSql; 3], comp_games: f64, comp_avg: f64) -> Result<Value> {
    let mut out = Vec::new();
    for r in rows(
        conn,
        "SELECT * FROM stat_comp_special WHERE patch = ? AND bucket = ? AND comp_key = ? AND games >= ?
         ORDER BY sum_place * 1.0 / games, games DESC, item, holder LIMIT ?",
        &[k[0], k[1], k[2], &SPECIAL_MIN_GAMES, &(SPECIAL_SHOWN as i64)],
    )? {
        let avg = f(&r, "sum_place") / f(&r, "games");
        let item = describe(conn, Kind::Item, s(&r, "item"), false)?;
        let holder = describe(conn, Kind::Unit, s(&r, "holder"), false)?;
        out.push(json!({
            "item": item, "holder": holder, "games": g(&r, "games"), "freq": num(f(&r, "games") / comp_games),
            "avgPlace": num(avg), "placeChange": num(avg - comp_avg),
        }));
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::super::{CompCache, comp};
    use serde_json::{Value, json};

    fn db(name: &str) -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("tft-api-comp-page-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO stat_meta VALUES ('99.1', 'ALL', 13, 100, 1);
            INSERT INTO stat_comps (patch, bucket, comp_key, games, sum_place, wins, top4, sum_level) VALUES ('99.1', 'ALL', 'X|C', 100, 450, 10, 50, 800);
            INSERT INTO stat_comp_units VALUES ('99.1', 'ALL', 'X|C', 'C', 100, 200, 290, 90, 450),
              ('99.1', 'ALL', 'X|C', 'H1', 100, 200, 0, 0, 450), ('99.1', 'ALL', 'X|C', 'H2', 100, 200, 0, 0, 450),
              ('99.1', 'ALL', 'X|C', 'H3', 100, 200, 0, 0, 450), ('99.1', 'ALL', 'X|C', 'H4', 100, 200, 0, 0, 450);
            INSERT INTO static_data VALUES ('unit', 'C', 'Carry', NULL, '{"cost":4,"traits":["Alpha"],"role":"ADCarry"}'),
              ('unit', 'H1', 'Shares a trait', NULL, '{"cost":1,"traits":["Alpha"],"role":"Tank"}'),
              ('unit', 'H2', 'Shares a role', NULL, '{"cost":2,"traits":["Beta"],"role":"ADCarry"}'),
              ('unit', 'H3', 'Shares nothing', NULL, '{"cost":1,"traits":["Beta"],"role":"Tank"}'),
              ('unit', 'H4', 'Too few games', NULL, '{"cost":1,"traits":["Alpha"]}');
            INSERT INTO stat_comp_level_bands VALUES
              ('99.1', 'ALL', 'X|C', 1, 8, 10, 15, 10, 300), ('99.1', 'ALL', 'X|C', 1, 9, 10, 15, 5, 160),
              ('99.1', 'ALL', 'X|C', 3, 0, 5, 30, 0, 0), ('99.1', 'ALL', 'X|C', 3, 7, 3, 18, 3, 75);
            INSERT INTO stat_comp_carry_units VALUES ('99.1', 'ALL', 'X|C', 'C', 95, 400, 90, 360, 60);
            INSERT INTO stat_comp_builds VALUES ('99.1', 'ALL', 'X|C', 'C', 'A|B|C', 20, 60),
              ('99.1', 'ALL', 'X|C', 'C', 'A|B|D', 10, 50), ('99.1', 'ALL', 'X|C', 'C', 'A|B|E', 5, 10),
              ('99.1', 'ALL', 'X|C', 'C', 'A|B|F', 4, 4), ('99.1', 'ALL', 'X|C', 'C', 'A|B|G', 6, 30);
            INSERT INTO stat_comp_carry_items VALUES ('99.1', 'ALL', 'X|C', 'C', 'I1', 10, 30),
              ('99.1', 'ALL', 'X|C', 'C', 'I2', 9, 9), ('99.1', 'ALL', 'X|C', 'C', 'I3', 50, 225);
            INSERT INTO stat_comp_holders VALUES ('99.1', 'ALL', 'X|C', 'C', 'H1', 40, 160, 20),
              ('99.1', 'ALL', 'X|C', 'C', 'H2', 50, 200, 30), ('99.1', 'ALL', 'X|C', 'C', 'H3', 60, 240, 40),
              ('99.1', 'ALL', 'X|C', 'C', 'H4', 4, 4, 4);
            INSERT INTO stat_comp_special VALUES ('99.1', 'ALL', 'X|C', 'TFT_Item_AlphaEmblem', 'C', 30, 90),
              ('99.1', 'ALL', 'X|C', 'TFT_Item_BetaEmblem', 'C', 29, 29), ('99.1', 'ALL', 'X|C', 'TFT_Item_Radiant', 'H1', 40, 200);
            "#,
        )
        .unwrap();
        (dir, conn)
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn sections_apply_their_thresholds_and_baselines() {
        let (dir, conn) = db("sections");
        let body = Value::Object(comp(&conn, Some("patch=99.1"), "X|C", &CompCache::default()).unwrap());

        // C5: bands with games only; a tie in levels goes to the lower one; unknown levels count for games only.
        let lv = &body["levelling"];
        assert_eq!(lv.as_array().unwrap().len(), 2);
        assert_eq!((lv[0]["label"].clone(), lv[0]["games"].clone(), lv[0]["commonLevel"].clone()), (json!("1st–2nd"), json!(20), json!(8)));
        assert_eq!(lv[0]["avgRound"], json!(460.0 / 15.0));
        assert_eq!(lv[1]["band"], json!(3));
        assert_eq!(lv[1]["games"], json!(8));
        assert_eq!(lv[1]["levels"], json!([{ "level": 7, "games": 3, "share": 1.0 }]));
        assert_eq!(lv[1]["commonLevel"], json!(7));

        // C6: top 3 builds by games, 5 games or more; placeChange against the carry with any three items (4.0).
        let c = &body["carries"][0];
        assert_eq!(c["unit"]["id"], json!("C"));
        let builds: Vec<String> = c["builds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| ids(&b["items"]).join("|"))
            .collect();
        assert_eq!(builds, ["A|B|C", "A|B|D", "A|B|G"]);
        assert_eq!(c["builds"][0]["placeChange"], json!(3.0 - 4.0));
        assert_eq!(c["builds"][0]["freq"], json!(20.0 / 90.0));
        // Single items: 10 games or more, best place change first, against the carry on the board (400 / 95).
        assert_eq!(ids(&c["items"]), ["I1", "I3"]);
        assert_eq!(c["items"][0]["placeChange"], json!(3.0 - 400.0 / 95.0));

        // C7: cheaper units sharing a trait or role, most top 4s first; H3 shares nothing, H4 has too few games.
        let h = &body["holders"][0];
        assert_eq!(ids(&h["units"]), ["H2", "H1"]);
        assert_eq!(h["units"][0]["top4Share"], json!(0.5));
        assert_eq!((h["units"][0]["sameRole"].clone(), h["units"][1]["sharedTraits"].clone()), (json!(true), json!(["Alpha"])));

        // C9: 30 games or more; place change against the comp's average place (4.5).
        let sp = body["special"].as_array().unwrap();
        let got: Vec<(&str, &str)> = sp.iter().map(|r| (r["item"]["id"].as_str().unwrap(), r["holder"]["id"].as_str().unwrap())).collect();
        assert_eq!(got, [("TFT_Item_AlphaEmblem", "C"), ("TFT_Item_Radiant", "H1")]);
        assert_eq!(sp[0]["placeChange"], json!(3.0 - 4.5));
        assert_eq!((sp[0]["item"]["type"].clone(), sp[1]["item"]["type"].clone()), (json!("emblem"), json!("radiant")));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_carry_without_set_data_lists_every_cheaper_unit_and_a_comp_without_rows_gets_empty_sections() {
        let (dir, conn) = db("bare");
        conn.execute_batch("DELETE FROM static_data WHERE id = 'C'").unwrap();
        let body = Value::Object(comp(&conn, Some("patch=99.1"), "X|C", &CompCache::default()).unwrap());
        assert_eq!(ids(&body["holders"][0]["units"]), ["H3", "H2", "H1"]);

        for t in ["stat_comp_level_bands", "stat_comp_carry_units", "stat_comp_builds", "stat_comp_carry_items", "stat_comp_holders", "stat_comp_special"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        let body = Value::Object(comp(&conn, Some("patch=99.1"), "X|C", &CompCache::default()).unwrap());
        for k in ["levelling", "carries", "holders", "special"] {
            assert_eq!(body[k], json!([]), "{k}");
        }
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
