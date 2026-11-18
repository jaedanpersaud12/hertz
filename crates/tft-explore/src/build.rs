//! Building an index from the database, and refreshing one after new boards arrive.

use crate::file::{self, Dir, Index, Kind, Parts, Row, key, trait_entry, trait_parts, unit_entry, unit_parts};
use anyhow::{Context, Result, bail, ensure};
use roaring::RoaringBitmap;
use rusqlite::{Connection, types::ValueRef};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Names to small ids, in the order first seen.
#[derive(Clone, Default)]
struct Dict {
    names: Vec<String>,
    ids: HashMap<Vec<u8>, u16>,
}
impl Dict {
    fn from(names: &[String]) -> Dict {
        Dict { names: names.to_vec(), ids: names.iter().enumerate().map(|(i, n)| (n.as_bytes().to_vec(), i as u16)).collect() }
    }
    fn id(&mut self, bytes: &[u8], what: &str) -> Result<u16> {
        if let Some(&id) = self.ids.get(bytes) {
            return Ok(id);
        }
        let name = std::str::from_utf8(bytes).with_context(|| format!("a {what} id that is not UTF-8"))?;
        ensure!(self.names.len() < u16::MAX as usize, "more than {} {what}s", u16::MAX);
        let id = self.names.len() as u16;
        self.names.push(name.to_string());
        self.ids.insert(bytes.to_vec(), id);
        Ok(id)
    }
}

fn int(v: ValueRef, what: &str) -> Result<i64> {
    match v {
        ValueRef::Integer(i) => Ok(i),
        other => bail!("{what} is {:?}, not a whole number", other.data_type()),
    }
}
fn small<T: TryFrom<i64>>(v: i64, what: &str) -> Result<T> {
    T::try_from(v).map_err(|_| anyhow::anyhow!("{what} {v} does not fit the index"))
}
fn text<'a>(v: ValueRef<'a>, what: &str) -> Result<&'a [u8]> {
    match v {
        ValueRef::Text(t) => Ok(t),
        other => bail!("{what} is {:?}, not text", other.data_type()),
    }
}

/// An index being put together in memory. Rows are kept in participant id order.
#[derive(Clone, Default)]
pub struct Builder {
    patch: String,
    units: Dict,
    traits: Dict,
    items: Dict,
    comps: Dict,
    queues: Vec<Option<i64>>,
    rows: Vec<Row>,
    unit_e: Vec<u16>,
    trait_e: Vec<u16>,
    item_e: Vec<u32>,
    max_pid: i64,
}

impl Builder {
    pub fn new(patch: &str) -> Builder {
        Builder { patch: patch.to_string(), ..Default::default() }
    }

    /// Starts from an existing file: its boards are kept as they are, and new ones go after them.
    pub fn from_index(ix: &Index) -> Builder {
        let d = &ix.dir;
        Builder {
            patch: d.patch.clone(),
            units: Dict::from(&d.units),
            traits: Dict::from(&d.traits),
            items: Dict::from(&d.items),
            comps: Dict::from(&d.comps),
            queues: d.queues.clone(),
            rows: ix.rows().to_vec(),
            unit_e: ix.units().to_vec(),
            trait_e: ix.traits().to_vec(),
            item_e: ix.items().to_vec(),
            max_pid: d.max_pid,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    /// Adds every board of the patch with a participant id above `after`, with its units, traits and items.
    /// Fails on a value the index cannot hold exactly (the app then answers from SQL); the builder is then half
    /// filled and must be dropped.
    pub fn add_from_db(&mut self, conn: &Connection, after: i64) -> Result<usize> {
        ensure!(after >= self.max_pid || self.rows.is_empty(), "boards must be added in id order");
        let base = self.rows.len();
        let mut pids = Vec::new();
        {
            let mut q = conn.prepare(
                "SELECT pa.id, pa.placement, pa.level, pa.comp_key, m.game_datetime, m.rank, m.queue_id
                 FROM matches m JOIN participants pa ON pa.match_id = m.match_id
                 WHERE m.patch = ?1 AND pa.id > ?2 ORDER BY pa.id",
            )?;
            let mut rs = q.query(rusqlite::params![self.patch, after])?;
            while let Some(r) = rs.next()? {
                let pid = int(r.get_ref(0)?, "participants.id")?;
                let placement = small(int(r.get_ref(1)?, "placement")?, "placement")?;
                let level = match r.get_ref(2)? {
                    ValueRef::Null => Row::NO_LEVEL,
                    v => small::<i16>(int(v, "level")?, "level").and_then(|l| {
                        ensure!(l != Row::NO_LEVEL, "level {l} does not fit the index");
                        Ok(l)
                    })?,
                };
                let comp = match r.get_ref(3)? {
                    ValueRef::Null => Row::NO_COMP,
                    v => self.comps.id(text(v, "comp_key")?, "comp")? as u32,
                };
                let time = int(r.get_ref(4)?, "game_datetime")?;
                let rank = small(int(r.get_ref(5)?, "rank")?, "rank")?;
                let queue = match r.get_ref(6)? {
                    ValueRef::Null => None,
                    v => Some(int(v, "queue_id")?),
                };
                let queue = match self.queues.iter().position(|q| *q == queue) {
                    Some(i) => i,
                    None => {
                        ensure!(self.queues.len() < 256, "more than 256 queues");
                        self.queues.push(queue);
                        self.queues.len() - 1
                    }
                } as u8;
                pids.push(pid);
                self.rows.push(Row { pid, time, placement, level, rank, queue, comp, ..Row::default() });
            }
        }
        let (Some(&lo), Some(&hi)) = (pids.first(), pids.last()) else { return Ok(0) };
        self.max_pid = hi;

        // Each table is read in participant id order through its participant index and matched to the boards.
        let units = &mut self.units;
        let starts = fill(
            conn,
            "SELECT participant_id, character_id, star, n_items FROM units WHERE participant_id BETWEEN ?1 AND ?2 ORDER BY participant_id",
            lo,
            hi,
            &pids,
            &mut self.unit_e,
            &mut |r| {
                let u = units.id(text(r.get_ref(1)?, "character_id")?, "unit")?;
                let star: u8 = small(int(r.get_ref(2)?, "star")?, "star")?;
                let n: u8 = small(int(r.get_ref(3)?, "n_items")?, "n_items")?;
                unit_entry(u, star, n).with_context(|| format!("unit {u} at {star} stars with {n} items does not fit the index"))
            },
        )?;
        for (k, s) in starts.into_iter().enumerate() {
            self.rows[base + k].unit_start = s;
        }
        let traits = &mut self.traits;
        let starts = fill(
            conn,
            "SELECT participant_id, name, tier_current FROM traits WHERE participant_id BETWEEN ?1 AND ?2 ORDER BY participant_id",
            lo,
            hi,
            &pids,
            &mut self.trait_e,
            &mut |r| {
                let t = traits.id(text(r.get_ref(1)?, "trait name")?, "trait")?;
                let tier: u8 = small(int(r.get_ref(2)?, "tier_current")?, "tier_current")?;
                trait_entry(t, tier).with_context(|| format!("trait {t} at tier {tier} does not fit the index"))
            },
        )?;
        for (k, s) in starts.into_iter().enumerate() {
            self.rows[base + k].trait_start = s;
        }
        let (units, items) = (&mut self.units, &mut self.items);
        let starts = fill(
            conn,
            "SELECT participant_id, character_id, item FROM unit_items WHERE participant_id BETWEEN ?1 AND ?2 ORDER BY participant_id",
            lo,
            hi,
            &pids,
            &mut self.item_e,
            &mut |r| {
                let u = units.id(text(r.get_ref(1)?, "character_id")?, "unit")?;
                let i = items.id(text(r.get_ref(2)?, "item")?, "item")?;
                Ok((u as u32) << 16 | i as u32)
            },
        )?;
        for (k, s) in starts.into_iter().enumerate() {
            self.rows[base + k].item_start = s;
        }
        ensure!(self.rows.len() < u32::MAX as usize, "too many boards for one index");
        Ok(pids.len())
    }

    /// Every bitmap, sorted by key.
    fn bitmaps(&self) -> Vec<(u64, RoaringBitmap)> {
        let mut m: HashMap<u64, RoaringBitmap> = HashMap::new();
        let mut put = |k: u64, r: u32| {
            // Rows are visited in order, so push (append) is enough; a repeat on the same board is a no-op.
            let _ = m.entry(k).or_default().try_push(r);
        };
        for (r, row) in self.rows.iter().enumerate() {
            let rn = r as u32;
            for &e in file::entries(&self.rows, r, &self.unit_e, |x| x.unit_start) {
                let (u, star, _) = unit_parts(e);
                put(key(Kind::Unit, u as u32, 0), rn);
                put(key(Kind::UnitStar, u as u32, star as u32), rn);
            }
            for &e in file::entries(&self.rows, r, &self.trait_e, |x| x.trait_start) {
                let (t, tier) = trait_parts(e);
                put(key(Kind::Trait, t as u32, 0), rn);
                put(key(Kind::TraitTier, t as u32, tier as u32), rn);
            }
            for &e in file::entries(&self.rows, r, &self.item_e, |x| x.item_start) {
                put(key(Kind::UnitItem, e >> 16, e & 0xffff), rn);
                put(key(Kind::Item, e & 0xffff, 0), rn);
            }
            put(key(Kind::Level, 0, row.level as i32 as u32), rn);
            put(key(Kind::Placement, 0, row.placement as i32 as u32), rn);
            put(key(Kind::Rank, 0, row.rank as i32 as u32), rn);
            put(key(Kind::Queue, row.queue as u32, 0), rn);
            if row.comp != Row::NO_COMP {
                put(key(Kind::Comp, row.comp, 0), rn);
            }
        }
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by_key(|(k, _)| *k);
        v
    }

    /// Writes the index as the next generation for its patch in `dir`, then removes the older ones it can.
    pub fn write(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let generation = crate::newest(dir, &self.patch).map_or(1, |(g, _)| g + 1);
        let path = dir.join(crate::file_name(&self.patch, generation));
        let tmp = dir.join(format!("{}.tmp-{}", crate::file_name(&self.patch, generation), std::process::id()));
        let parts = Parts {
            dir: Dir {
                patch: self.patch.clone(),
                max_pid: self.max_pid,
                built_at: now_ms(),
                units: self.units.names.clone(),
                traits: self.traits.names.clone(),
                items: self.items.names.clone(),
                comps: self.comps.names.clone(),
                queues: self.queues.clone(),
                ..Dir::default()
            },
            rows: &self.rows,
            units: &self.unit_e,
            traits: &self.trait_e,
            items: &self.item_e,
            bitmaps: self.bitmaps(),
        };
        let written = file::write(&tmp, parts).and_then(|()| {
            std::fs::rename(&tmp, &path).with_context(|| format!("moving the new index into place at {}", path.display()))
        });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        // Older snapshots: a reader that still has one mapped keeps it (on Windows the delete fails and is retried
        // at the next write).
        for (g, old) in crate::generations(dir, &self.patch) {
            if g < generation {
                let _ = std::fs::remove_file(old);
            }
        }
        Ok(path)
    }
}

/// Streams one board table in participant id order and appends each row's entry to `out`, for the boards in
/// `pids` (sorted). Returns where each board's entries start.
fn fill<T>(
    conn: &Connection,
    sql: &str,
    lo: i64,
    hi: i64,
    pids: &[i64],
    out: &mut Vec<T>,
    entry: &mut dyn FnMut(&rusqlite::Row) -> Result<T>,
) -> Result<Vec<u32>> {
    let mut starts = vec![0u32; pids.len()];
    let mut next = 0;
    let mut q = conn.prepare(sql)?;
    let mut rs = q.query([lo, hi])?;
    while let Some(r) = rs.next()? {
        let pid = int(r.get_ref(0)?, "participant_id")?;
        while next < pids.len() && pids[next] <= pid {
            starts[next] = out.len() as u32;
            next += 1;
        }
        if next > 0 && pids[next - 1] == pid {
            ensure!(out.len() < u32::MAX as usize, "too many entries for one index");
            out.push(entry(r)?);
        }
    }
    for s in &mut starts[next..] {
        *s = out.len() as u32;
    }
    Ok(starts)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshKind {
    /// No boards for the patch: nothing written (older files removed).
    Empty,
    /// The newest file already holds every board.
    Current,
    /// The newest file's boards were kept and the newer ones added.
    Appended,
    /// Built from every board.
    Full,
}

#[derive(Debug)]
pub struct Refreshed {
    pub kind: RefreshKind,
    pub rows: u64,
    pub added: u64,
    /// The file that now holds the patch (None when empty).
    pub path: Option<PathBuf>,
}

/// Brings the index for `patch` in `dir` up to date with the database. When the newest file still holds exactly
/// the boards stored up to its highest id, only the boards after it are read; anything else (no file, a file from
/// another format, boards deleted or moved into the patch) is a full build.
pub fn refresh(conn: &Connection, dir: &Path, patch: &str) -> Result<Refreshed> {
    let existing = crate::newest(dir, patch).and_then(|(_, p)| Index::open(&p).ok());
    let after = existing.as_ref().map_or(i64::MIN, |ix| ix.dir.max_pid);
    let (count, above): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(pa.id > ?2), 0) FROM matches m JOIN participants pa ON pa.match_id = m.match_id
         WHERE m.patch = ?1",
        rusqlite::params![patch, after],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count == 0 {
        for (_, p) in crate::generations(dir, patch) {
            let _ = std::fs::remove_file(p);
        }
        return Ok(Refreshed { kind: RefreshKind::Empty, rows: 0, added: 0, path: None });
    }
    if let Some(ix) = &existing
        && (count - above) as u64 == ix.dir.rows
        && ix.dir.patch == patch
    {
        if above == 0 {
            return Ok(Refreshed { kind: RefreshKind::Current, rows: ix.dir.rows, added: 0, path: Some(ix.path.clone()) });
        }
        let mut b = Builder::from_index(ix);
        let added = b.add_from_db(conn, ix.dir.max_pid)?;
        let path = b.write(dir)?;
        return Ok(Refreshed { kind: RefreshKind::Appended, rows: b.rows() as u64, added: added as u64, path: Some(path) });
    }
    let mut b = Builder::new(patch);
    let added = b.add_from_db(conn, i64::MIN)?;
    let path = b.write(dir)?;
    Ok(Refreshed { kind: RefreshKind::Full, rows: b.rows() as u64, added: added as u64, path: Some(path) })
}
