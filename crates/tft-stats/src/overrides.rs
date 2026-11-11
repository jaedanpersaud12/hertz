//! Your renames and merges (PRD 2 C14), kept in comp_overrides and never rebuilt. A rename is applied when the
//! comps are read (the API). A merge is applied on every stats build, after the comp rules: the boards of the
//! merged comp count for the comp it was merged into, whichever comp method is in use. Overrides beat rules, so
//! a merge may leave two comps with more core overlap than the rules allow.
//!
//! Changing a merge marks every stats scope as needing a full rebuild (the stored comp tables were made
//! without it); the next `tft stats`, or the engine's next build, applies it.

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub comp_id: String,
    pub name: Option<String>,
    pub merge_into: Option<String>,
}

/// The longest name kept, in characters.
pub const NAME_MAX: usize = 60;

pub fn list(db: &Connection) -> Result<Vec<Override>> {
    Ok(db
        .prepare("SELECT comp_id, name, merge_into FROM comp_overrides ORDER BY comp_id")?
        .query_map([], |r| Ok(Override { comp_id: r.get(0)?, name: r.get(1)?, merge_into: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn get(db: &Connection, comp_id: &str) -> Result<Option<Override>> {
    Ok(db
        .query_row("SELECT comp_id, name, merge_into FROM comp_overrides WHERE comp_id = ?", [comp_id], |r| {
            Ok(Override { comp_id: r.get(0)?, name: r.get(1)?, merge_into: r.get(2)? })
        })
        .optional()?)
}

/// Your name for a comp, if you gave it one.
pub fn name_of(db: &Connection, comp_id: &str) -> Result<Option<String>> {
    Ok(get(db, comp_id)?.and_then(|o| o.name))
}

fn save(db: &Connection, o: &Override) -> Result<()> {
    if o.name.is_none() && o.merge_into.is_none() {
        db.execute("DELETE FROM comp_overrides WHERE comp_id = ?", [&o.comp_id])?;
    } else {
        db.execute(
            "INSERT INTO comp_overrides (comp_id, name, merge_into) VALUES (?1, ?2, ?3)
             ON CONFLICT(comp_id) DO UPDATE SET name = excluded.name, merge_into = excluded.merge_into",
            params![o.comp_id, o.name, o.merge_into],
        )?;
    }
    Ok(())
}

/// Renames a comp. An empty name goes back to the name the rules give.
pub fn rename(db: &Connection, comp_id: &str, name: &str) -> Result<Override> {
    let name: String = name.trim().chars().take(NAME_MAX).collect();
    let mut o = get(db, comp_id)?.unwrap_or(Override { comp_id: comp_id.to_string(), name: None, merge_into: None });
    o.name = (!name.is_empty()).then_some(name);
    save(db, &o)?;
    Ok(o)
}

/// Merges a comp into another, or (with `None`) undoes the merge. Refuses a merge into itself or one that
/// would go round in a circle.
pub fn merge(db: &Connection, comp_id: &str, into: Option<&str>) -> Result<Override> {
    let into = into.map(str::trim).filter(|s| !s.is_empty());
    if let Some(target) = into {
        if target == comp_id {
            bail!("A comp cannot be merged into itself.");
        }
        let mut map: HashMap<String, String> = merges(db)?.into_iter().collect();
        map.insert(comp_id.to_string(), target.to_string());
        if resolve(&map, comp_id).is_none() {
            bail!("That merge would go round in a circle: the other comp is already merged into this one.");
        }
    }
    let mut o = get(db, comp_id)?.unwrap_or(Override { comp_id: comp_id.to_string(), name: None, merge_into: None });
    let changed = o.merge_into.as_deref() != into;
    o.merge_into = into.map(str::to_string);
    save(db, &o)?;
    if changed {
        invalidate(db)?;
    }
    Ok(o)
}

/// Forgets your rename and merge of a comp.
pub fn clear(db: &Connection, comp_id: &str) -> Result<()> {
    let had_merge = get(db, comp_id)?.is_some_and(|o| o.merge_into.is_some());
    db.execute("DELETE FROM comp_overrides WHERE comp_id = ?", [comp_id])?;
    if had_merge {
        invalidate(db)?;
    }
    Ok(())
}

/// Every stats scope gets a full rebuild next time, so a changed merge is applied.
pub fn invalidate(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM stat_watermark", [])?;
    Ok(())
}

fn merges(db: &Connection) -> Result<Vec<(String, String)>> {
    Ok(db
        .prepare("SELECT comp_id, merge_into FROM comp_overrides WHERE merge_into IS NOT NULL ORDER BY comp_id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Where a chain of merges ends; `None` for a circle.
fn resolve(map: &HashMap<String, String>, from: &str) -> Option<String> {
    let mut at = from.to_string();
    for _ in 0..=map.len() {
        match map.get(&at) {
            Some(next) => at = next.clone(),
            None => return Some(at),
        }
    }
    None
}

/// Each merged comp and the comp its boards end up in, chains followed (a circle, which [`merge`] refuses, is
/// left out).
pub(crate) fn merge_targets(db: &Connection) -> Result<Vec<(String, String)>> {
    let pairs = merges(db)?;
    let map: HashMap<String, String> = pairs.iter().cloned().collect();
    Ok(pairs.into_iter().filter_map(|(from, _)| resolve(&map, &from).map(|to| (from, to))).collect())
}

/// Applies your merges to the boards in the temporary table p (full rebuild).
pub(crate) fn apply_to_boards(db: &Connection) -> Result<()> {
    let mut st = db.prepare("UPDATE p SET comp_key = ? WHERE comp_key = ?")?;
    for (from, to) in merge_targets(db)? {
        st.execute(params![to, from])?;
    }
    Ok(())
}

/// Applies your merges to inc_map (stored label to the label it folds into) in an incremental baseline build,
/// with the same result as [`apply_to_boards`] after the look-alike merge.
pub(crate) fn apply_to_map(db: &Connection) -> Result<()> {
    for (from, to) in merge_targets(db)? {
        db.execute("UPDATE inc_map SET dst = ? WHERE dst = ?", params![to, from])?;
        db.execute("INSERT OR IGNORE INTO inc_map VALUES (?, ?)", params![from, to])?;
    }
    Ok(())
}
