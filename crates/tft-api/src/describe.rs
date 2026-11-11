//! `describe` from `src/server/describe.ts`: turns ids from match data into a name, an icon path and a few facts.

use crate::js::encode_component;
use anyhow::Result;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Unit,
    Trait,
    Item,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Unit => "unit",
            Kind::Trait => "trait",
            Kind::Item => "item",
        }
    }
}

/// "TFT18_KogMaw" -> "Kog Maw", for when Community Dragon data has not been downloaded yet.
pub fn prettify(id: &str) -> String {
    static SET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^TFT[0-9]*_?").unwrap());
    static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(Item|Augment)_").unwrap());
    static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z])([A-Z])").unwrap());
    let s = SET.replace(id, "");
    let s = PREFIX.replace(&s, "");
    let s = CAMEL.replace_all(&s, "$1 $2");
    s.replace('_', " ")
}

/// `itemType` from `src/static/fetch.ts`, with no composition known.
pub fn item_type(id: &str) -> &'static str {
    static RADIANT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)radiant").unwrap());
    static ARTIFACT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)artifact|ornn").unwrap());
    static EMBLEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)emblem").unwrap());
    static SUPPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)support").unwrap());
    static COMPONENTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^TFT_Item_(BFSword|RecurveBow|NeedlesslyLargeRod|TearOfTheGoddess|ChainVest|NegatronCloak|GiantsBelt|SparringGloves|Spatula|FryingPan)$").unwrap()
    });
    if RADIANT.is_match(id) {
        "radiant"
    } else if ARTIFACT.is_match(id) {
        "artifact"
    } else if EMBLEM.is_match(id) {
        "emblem"
    } else if SUPPORT.is_match(id) {
        "support"
    } else if COMPONENTS.is_match(id) {
        "component"
    } else {
        "other"
    }
}

/// `data.key ?? fallback`.
fn or(data: &Value, key: &str, fallback: Value) -> Value {
    match data.get(key) {
        Some(Value::Null) | None => fallback,
        Some(v) => v.clone(),
    }
}

pub fn describe(conn: &Connection, kind: Kind, id: &str, full: bool) -> Result<Map<String, Value>> {
    // Match data and set data sometimes differ in letter case.
    let row: Option<(String, Option<String>, String)> = conn
        .prepare_cached("SELECT name, icon, data FROM static_data WHERE kind = ? AND id = ? COLLATE NOCASE")?
        .query_row((kind.as_str(), id), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let data: Value = match &row {
        Some((_, _, d)) => serde_json::from_str(d)?,
        None => json!({}),
    };
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("name".into(), json!(row.as_ref().map(|r| r.0.clone()).unwrap_or_else(|| prettify(id))));
    let has_icon = row.as_ref().and_then(|r| r.1.as_deref()).is_some_and(|i| !i.is_empty());
    out.insert(
        "icon".into(),
        if has_icon { json!(format!("/api/icon/{}/{}", kind.as_str(), encode_component(id))) } else { Value::Null },
    );
    match kind {
        Kind::Unit => {
            out.insert("cost".into(), or(&data, "cost", Value::Null));
            out.insert("traits".into(), or(&data, "traits", json!([])));
            if full {
                for k in ["role", "ability", "stats"] {
                    out.insert(k.into(), or(&data, k, Value::Null));
                }
            }
        }
        Kind::Trait => {
            out.insert("breakpoints".into(), or(&data, "breakpoints", json!([])));
        }
        Kind::Item => {
            out.insert("type".into(), or(&data, "type", json!(item_type(id))));
            if full {
                out.insert("composition".into(), or(&data, "composition", json!([])));
                out.insert("text".into(), or(&data, "text", json!("")));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_ids() {
        assert_eq!(prettify("TFT18_KogMaw"), "Kog Maw");
        assert_eq!(prettify("TFT_Item_InfinityEdge"), "Infinity Edge");
        assert_eq!(prettify("tft99_augment_aBcDe"), "a Bc De");
        assert_eq!(prettify(""), "");
        assert_eq!(item_type("TFT5_Item_BloodthirsterRadiant"), "radiant");
        assert_eq!(item_type("TFT_Item_BFSword"), "component");
        assert_eq!(item_type("TFT_Item_X"), "other");
    }
}
