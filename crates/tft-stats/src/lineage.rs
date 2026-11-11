//! A comp keeps its identity across patches (PRD 2 C13). A comp id is a hash of its core, so a comp whose core
//! changes with a patch gets a new id. Each comp is linked to last patch's comp with the highest core overlap,
//! when that overlap is above [`LINEAGE_MIN`], in comp_lineage. Along each link, your favourites, hidden comps,
//! renames and merges, and the pinned comp (when it is on the newest patch), carry over to the new id.
//!
//! Worked out after every stats build with the clusters method, from the stored cores, for every pair of
//! neighbouring patches that has them. Carrying over never undoes a choice you made on the new id: marks and
//! overrides already there are kept.

use crate::cluster::{overlap, parse_list};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

/// A comp continues last patch's comp when their cores overlap more than this.
pub const LINEAGE_MIN: f64 = 0.6;

/// "16.22" as (16, 22), for ordering patches; anything else sorts first.
fn version(p: &str) -> (i64, i64) {
    let mut it = p.split('.').map(|x| x.parse::<i64>().unwrap_or(-1));
    (it.next().unwrap_or(-1), it.next().unwrap_or(-1))
}

/// Each comp of a patch with its core and games, from the scope where it has the most games.
fn cores(db: &Connection, patch: &str) -> Result<Vec<(String, Vec<String>, i64)>> {
    let mut best: HashMap<String, (Vec<String>, i64)> = HashMap::new();
    let mut st = db.prepare("SELECT comp_key, core, games FROM stat_comps WHERE patch = ? AND core IS NOT NULL")?;
    let mut rows = st.query([patch])?;
    while let Some(r) = rows.next()? {
        let (key, core, games): (String, String, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let e = best.entry(key).or_insert((Vec::new(), -1));
        if games > e.1 {
            *e = (parse_list(&core), games);
        }
    }
    let mut out: Vec<(String, Vec<String>, i64)> = best.into_iter().map(|(k, (c, g))| (k, c, g)).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Rewrites comp_lineage for every patch with comp cores, and carries your marks and overrides along it.
pub fn update(db: &Connection) -> Result<()> {
    let mut patches: Vec<String> = db
        .prepare("SELECT DISTINCT patch FROM stat_comps WHERE core IS NOT NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    patches.sort_by(|a, b| version(b).cmp(&version(a)).then(b.cmp(a))); // newest first
    let newest = patches.first().cloned();
    let mut moved_overrides = false;
    for pair in patches.windows(2) {
        let (patch, prev) = (&pair[0], &pair[1]);
        let now = cores(db, patch)?;
        let before = cores(db, prev)?;
        db.execute("DELETE FROM comp_lineage WHERE patch = ?", [patch])?;
        let mut links: Vec<(String, String, f64, i64)> = Vec::new();
        for (id, core, games) in &now {
            let mut pick: Option<(f64, &str)> = None;
            for (pid, pcore, _) in &before {
                let o = overlap(core, pcore);
                if o > LINEAGE_MIN && pick.is_none_or(|p| o > p.0) {
                    pick = Some((o, pid));
                }
            }
            if let Some((o, pid)) = pick {
                db.execute(
                    "INSERT INTO comp_lineage (patch, comp_id, prev_patch, prev_comp_id, overlap) VALUES (?, ?, ?, ?, ?)",
                    params![patch, id, prev, pid, o],
                )?;
                links.push((id.clone(), pid.to_string(), o, *games));
            }
        }
        // Where last patch's comps went: the new comp that continues it best.
        let mut heir: HashMap<&str, (&str, f64, i64)> = HashMap::new();
        for (id, pid, o, g) in &links {
            let e = heir.entry(pid.as_str()).or_insert((id.as_str(), *o, *g));
            if (*o, *g) > (e.1, e.2) || ((*o, *g) == (e.1, e.2) && id.as_str() < e.0) {
                *e = (id.as_str(), *o, *g);
            }
        }
        for (id, pid, ..) in &links {
            if id == pid {
                continue;
            }
            db.execute(
                "INSERT OR IGNORE INTO comp_marks (comp_key, fav, hidden) SELECT ?, fav, hidden FROM comp_marks WHERE comp_key = ?",
                params![id, pid],
            )?;
            let old: Option<(Option<String>, Option<String>)> = db
                .query_row("SELECT name, merge_into FROM comp_overrides WHERE comp_id = ?", [pid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if let Some((name, into)) = old {
                let into = into.map(|t| heir.get(t.as_str()).map_or(t.clone(), |h| h.0.to_string()));
                let added = db.execute(
                    "INSERT OR IGNORE INTO comp_overrides (comp_id, name, merge_into) VALUES (?, ?, ?)",
                    params![id, name, into],
                )?;
                moved_overrides |= added > 0 && into.is_some();
            }
        }
        if Some(patch) == newest.as_ref() {
            let pinned: Option<String> =
                db.query_row("SELECT value FROM kv WHERE key = 'pinned'", [], |r| r.get(0)).optional()?;
            let pinned = pinned.map(|p| crate::cluster::parse_list(&format!("[{p}]")).pop().unwrap_or_default());
            if let Some((id, ..)) = pinned.as_deref().and_then(|p| heir.get(p))
                && Some(*id) != pinned.as_deref()
            {
                db.execute(
                    "UPDATE kv SET value = ? WHERE key = 'pinned'",
                    [crate::cluster::json_list(&[*id]).trim_start_matches('[').trim_end_matches(']')],
                )?;
            }
        }
    }
    if moved_overrides {
        crate::overrides::invalidate(db)?; // a merge carried to a new id is applied on the next build
    }
    Ok(())
}
