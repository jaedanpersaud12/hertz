//! The tier list's additions to `/api/comps` (PRD 2 C3, C4, C11): tier letter and trend on each comp, the
//! filters (`unit`, `trait`, `item`, `playstyle`, `rising`), and the choices the filters offer.
//!
//! Trend fields on a comp:
//! - `prevPlace`, `d3Place`: average place last patch and in the last 3 days of this patch (null when there
//!   is nothing to compare with, and `d3Place` on the 3-day window itself);
//! - `patchDelta` = avgPlace - prevPlace, `d3Delta` = d3Place - avgPlace: negative is better;
//! - `rising`: either delta is at least `risingPlaces` (config/tier-list.json) better.

use crate::js::{self, num};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use tft_core::order::locale_cmp;

type Obj = Map<String, Value>;

fn real(o: &Obj, k: &str) -> Option<f64> {
    o.get(k).and_then(Value::as_f64).filter(|x| x.is_finite())
}

/// The tier and trend fields for one comp, from its stat_comps row and its average place.
pub fn fields(row: &Obj, avg_place: f64) -> Vec<(&'static str, Value)> {
    let prev = real(row, "prev_place");
    let d3 = real(row, "d3_place");
    let delta = |x: Option<f64>| x.map_or(Value::Null, num);
    vec![
        ("tier", row.get("tier").cloned().unwrap_or(Value::Null)),
        ("score", delta(real(row, "score"))),
        ("prevPlace", delta(prev)),
        ("d3Place", delta(d3)),
        ("patchDelta", delta(prev.map(|p| avg_place - p))),
        ("d3Delta", delta(d3.map(|d| d - avg_place))),
        ("rising", json!(tft_stats::tier::is_rising(avg_place, prev, d3, tft_stats::tier::config()))),
    ]
}

/// The filters asked for in the query string. An empty value is no filter.
#[derive(Debug, Default, PartialEq)]
pub struct Filters {
    pub unit: Option<String>,
    pub trait_id: Option<String>,
    pub item: Option<String>,
    pub playstyle: Option<String>,
    pub rising: bool,
}

impl Filters {
    pub fn from_query(q: Option<&str>) -> Self {
        let get = |k: &str| js::query(q, k).filter(|v| !v.is_empty());
        Filters {
            unit: get("unit"),
            trait_id: get("trait"),
            item: get("item"),
            playstyle: get("playstyle"),
            rising: get("rising").is_some_and(|v| v == "1" || v == "true"),
        }
    }

    /// Whether a comp passes every filter asked for.
    pub fn keep(&self, c: &Obj) -> bool {
        let list = |k: &str| c.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
        let id_is = |v: &Value, id: &str| v.get("id").and_then(Value::as_str) == Some(id);
        let unit = self.unit.as_deref().is_none_or(|id| list("units").iter().chain(list("carries")).any(|u| id_is(u, id)));
        let trait_ok = self
            .trait_id
            .as_deref()
            .is_none_or(|id| c.get("trait").is_some_and(|t| id_is(t, id)) || list("traits").iter().any(|t| id_is(t, id)));
        let item = self.item.as_deref().is_none_or(|id| {
            list("units").iter().any(|u| u.get("items").and_then(Value::as_array).is_some_and(|is| is.iter().any(|i| id_is(i, id))))
        });
        let play = self.playstyle.as_deref().is_none_or(|k| c.get("play").and_then(|p| p.get("kind")).and_then(Value::as_str) == Some(k));
        let rising = !self.rising || c.get("rising") == Some(&Value::Bool(true));
        unit && trait_ok && item && play && rising
    }
}

/// An entity reduced to what a filter choice needs (the keys that say what kind it is stay, for the stress switch).
fn choice(e: &Value, keep: &[&str]) -> Option<(String, Value)> {
    let o = e.as_object()?;
    let id = o.get("id")?.as_str()?.to_string();
    let mut out = Map::new();
    for k in ["id", "name", "icon"].iter().chain(keep) {
        out.insert((*k).into(), o.get(*k).cloned().unwrap_or(Value::Null));
    }
    Some((id, Value::Object(out)))
}

fn sorted(m: BTreeMap<String, Value>) -> Value {
    let mut xs: Vec<Value> = m.into_values().collect();
    let name = |v: &Value| v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    xs.sort_by(|a, b| locale_cmp(&name(a), &name(b)));
    Value::Array(xs)
}

/// What the filters can pick from: every unit, trait, item and playstyle on the comps listed before filtering.
pub fn options(comps: &[&Obj]) -> Value {
    let (mut units, mut traits, mut items) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let mut plays: Vec<String> = Vec::new();
    let list = |c: &Obj, k: &str| c.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    for c in comps {
        for u in list(c, "units").iter().chain(&list(c, "carries")) {
            units.extend(choice(u, &["cost"]));
            for i in u.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                items.extend(choice(i, &["type"]));
            }
        }
        for t in list(c, "traits") {
            traits.extend(choice(&t, &["style"]));
        }
        if let Some(t) = c.get("trait").filter(|t| t.is_object()) {
            traits.entry(t["id"].as_str().unwrap_or("").to_string()).or_insert_with(|| choice(t, &["style"]).unwrap().1);
        }
        if let Some(k) = c.get("play").and_then(|p| p.get("kind")).and_then(Value::as_str)
            && !plays.iter().any(|p| p == k)
        {
            plays.push(k.to_string());
        }
    }
    plays.sort();
    json!({ "units": sorted(units), "traits": sorted(traits), "items": sorted(items), "playstyles": plays })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(id: &str, units: &[(&str, &[&str])], traits: &[&str], play: &str, rising: bool) -> Obj {
        let v = json!({
            "id": id,
            "trait": { "id": traits[0], "name": traits[0], "style": 1 },
            "carries": [{ "id": units[0].0, "name": units[0].0, "cost": 4 }],
            "units": units.iter().map(|(u, is)| json!({ "id": u, "name": u, "cost": 1,
                "items": is.iter().map(|i| json!({ "id": i, "name": i, "type": "craftable", "freq": 0.5 })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
            "traits": traits.iter().map(|t| json!({ "id": t, "name": t, "style": 2, "units": 3 })).collect::<Vec<_>>(),
            "play": { "kind": play, "label": play },
            "rising": rising,
        });
        v.as_object().unwrap().clone()
    }

    fn set() -> Vec<Obj> {
        vec![
            comp("A", &[("Jinx", &["IE", "LW"]), ("Vi", &[])], &["Rebel", "Sniper"], "fast", true),
            comp("B", &[("Jinx", &["Rageblade"]), ("Ekko", &[])], &["Rebel"], "reroll", false),
            comp("C", &[("Ahri", &["IE"])], &["Mage"], "fast", false),
        ]
    }

    fn pick(q: &str) -> Vec<String> {
        let f = Filters::from_query(Some(q));
        set().iter().filter(|c| f.keep(c)).map(|c| c["id"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn reads_the_filters_from_the_query() {
        assert_eq!(Filters::from_query(None), Filters::default());
        assert_eq!(Filters::from_query(Some("unit=&trait=&rising=0&playstyle=")), Filters::default());
        let f = Filters::from_query(Some("unit=TFT_A%7CB&trait=T&item=I&playstyle=fast&rising=1"));
        assert_eq!(
            f,
            Filters { unit: Some("TFT_A|B".into()), trait_id: Some("T".into()), item: Some("I".into()), playstyle: Some("fast".into()), rising: true }
        );
        assert!(Filters::from_query(Some("rising=true")).rising);
    }

    #[test]
    fn each_filter_narrows_and_they_combine() {
        assert_eq!(pick(""), ["A", "B", "C"]);
        assert_eq!(pick("unit=Jinx"), ["A", "B"]);
        assert_eq!(pick("unit=Vi"), ["A"]);
        assert_eq!(pick("trait=Rebel"), ["A", "B"]);
        assert_eq!(pick("trait=Sniper"), ["A"], "a trait that is not the main one counts");
        assert_eq!(pick("item=IE"), ["A", "C"]);
        assert_eq!(pick("playstyle=fast"), ["A", "C"]);
        assert_eq!(pick("rising=1"), ["A"]);
        // Combined: every filter must pass.
        assert_eq!(pick("unit=Jinx&playstyle=fast"), ["A"]);
        assert_eq!(pick("item=IE&trait=Mage"), ["C"]);
        assert_eq!(pick("unit=Jinx&item=Rageblade&playstyle=reroll"), ["B"]);
        assert_eq!(pick("unit=Jinx&trait=Mage"), Vec::<String>::new());
        assert_eq!(pick("unit=Nobody"), Vec::<String>::new());
    }

    #[test]
    fn options_list_every_choice_once_by_name() {
        let comps = set();
        let o = options(&comps.iter().collect::<Vec<_>>());
        let ids = |k: &str| -> Vec<&str> { o[k].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect() };
        assert_eq!(ids("units"), ["Ahri", "Ekko", "Jinx", "Vi"]);
        assert_eq!(ids("traits"), ["Mage", "Rebel", "Sniper"]);
        assert_eq!(ids("items"), ["IE", "LW", "Rageblade"]);
        assert_eq!(o["playstyles"], json!(["fast", "reroll"]));
        assert_eq!(o["items"][0], json!({ "id": "IE", "name": "IE", "icon": null, "type": "craftable" }), "no per-comp numbers");
    }

    #[test]
    fn trend_fields_from_a_stat_row() {
        let row = json!({ "tier": "S", "score": 0.5, "prev_place": 4.5, "d3_place": 4.4 }).as_object().unwrap().clone();
        let f: Map<String, Value> = fields(&row, 4.2).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(f["tier"], json!("S"));
        assert!((f["patchDelta"].as_f64().unwrap() + 0.3).abs() < 1e-12);
        assert!((f["d3Delta"].as_f64().unwrap() - 0.2).abs() < 1e-12);
        assert_eq!(f["rising"], json!(true), "0.3 better than last patch");
        let bare = json!({ "tier": null, "score": null, "prev_place": null, "d3_place": null }).as_object().unwrap().clone();
        let f: Map<String, Value> = fields(&bare, 4.2).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(
            Value::Object(f),
            json!({ "tier": null, "score": null, "prevPlace": null, "d3Place": null, "patchDelta": null, "d3Delta": null, "rising": false })
        );
    }
}
