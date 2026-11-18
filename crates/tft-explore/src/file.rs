//! The index file: layout, writing and the memory-mapped reader.
//!
//! ```text
//! header   64 bytes   "HERTZIDX", format version, where the directory is, total length
//! rows     40 bytes per board (Row), 8-byte aligned
//! units    u16 per unit on a board:   unit id << 6 | star << 3 | items held   (see [`unit_entry`])
//! traits   u16 per trait on a board:  trait id << 4 | tier                    (see [`trait_entry`])
//! items    u32 per item held:         unit id << 16 | item id
//! bitmaps  Roaring bitmaps (the portable serialised format), end to end
//! table    (key, offset, length) as three u64 per bitmap, sorted by key
//! dir      JSON: patch, counts, the id dictionaries, where each section is
//! footer   24 bytes   "HERTZEND", where the directory is, total length
//! ```
//!
//! All numbers are little-endian. A file is only used when its header, footer and length agree, so a write
//! that stopped half way is never read.

use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Bump when the layout changes: files from another version are not read, and are rebuilt.
pub const FORMAT_VERSION: u32 = 1;

const MAGIC: &[u8; 8] = b"HERTZIDX";
const END: &[u8; 8] = b"HERTZEND";
const HEADER: u64 = 64;
const FOOTER: u64 = 24;

/// One board. `*_start` are offsets into the unit, trait and item arrays; a board's entries run to the next
/// board's start (or the end of the array for the last board).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// participants.id
    pub pid: i64,
    /// matches.game_datetime (ms)
    pub time: i64,
    pub unit_start: u32,
    pub trait_start: u32,
    pub item_start: u32,
    /// Index into the comp dictionary, or [`Row::NO_COMP`].
    pub comp: u32,
    pub placement: i16,
    /// [`Row::NO_LEVEL`] when the level is not known.
    pub level: i16,
    /// matches.rank: the rank of the player the match was found through.
    pub rank: i16,
    /// Index into the queue dictionary (`Dir::queues`).
    pub queue: u8,
    pub pad: u8,
}
const _: () = assert!(std::mem::size_of::<Row>() == 40 && std::mem::align_of::<Row>() == 8);

impl Row {
    pub const NO_COMP: u32 = u32::MAX;
    pub const NO_LEVEL: i16 = i16::MIN;

    fn write(&self, w: &mut impl Write) -> std::io::Result<()> {
        w.write_all(&self.pid.to_le_bytes())?;
        w.write_all(&self.time.to_le_bytes())?;
        w.write_all(&self.unit_start.to_le_bytes())?;
        w.write_all(&self.trait_start.to_le_bytes())?;
        w.write_all(&self.item_start.to_le_bytes())?;
        w.write_all(&self.comp.to_le_bytes())?;
        w.write_all(&self.placement.to_le_bytes())?;
        w.write_all(&self.level.to_le_bytes())?;
        w.write_all(&self.rank.to_le_bytes())?;
        w.write_all(&[self.queue, self.pad])
    }
}

/// What a bitmap is of. The key packs the kind, then two numbers (`a` up to 24 bits, `b` 32 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// a = unit
    Unit = 1,
    /// a = unit, b = star
    UnitStar = 2,
    /// a = unit, b = item (from unit_items: the unit held the item)
    UnitItem = 3,
    /// a = trait (any tier: the board has a row for it)
    Trait = 4,
    /// a = trait, b = tier
    TraitTier = 5,
    /// a = item, held by any unit
    Item = 6,
    /// b = level as i32 bits; [`Row::NO_LEVEL`] for unknown
    Level = 7,
    /// b = placement as i32 bits
    Placement = 8,
    /// b = rank as i32 bits
    Rank = 9,
    /// a = queue dictionary index
    Queue = 10,
    /// a = comp dictionary index
    Comp = 11,
}

/// Units per patch the index can hold (10 bits), and the largest star and item count (3 bits each).
pub const MAX_UNITS: usize = 1 << 10;
/// Traits per patch (12 bits), and the largest tier (4 bits).
pub const MAX_TRAITS: usize = 1 << 12;

pub fn unit_entry(unit: u16, star: u8, items: u8) -> Option<u16> {
    ((unit as usize) < MAX_UNITS && star < 8 && items < 8).then_some(unit << 6 | (star as u16) << 3 | items as u16)
}
/// (unit, star, items)
pub fn unit_parts(e: u16) -> (u16, u8, u8) {
    (e >> 6, ((e >> 3) & 7) as u8, (e & 7) as u8)
}
pub fn trait_entry(t: u16, tier: u8) -> Option<u16> {
    ((t as usize) < MAX_TRAITS && tier < 16).then_some(t << 4 | tier as u16)
}
/// (trait, tier)
pub fn trait_parts(e: u16) -> (u16, u8) {
    (e >> 4, (e & 15) as u8)
}

pub fn key(kind: Kind, a: u32, b: u32) -> u64 {
    debug_assert!(a < 1 << 24);
    (kind as u64) << 56 | (a as u64) << 32 | b as u64
}

/// The directory: what is in the file and where.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Dir {
    pub version: u32,
    pub patch: String,
    pub rows: u64,
    /// The highest participant id in the file (0 when empty): a later build adds the boards above it.
    pub max_pid: i64,
    /// When the file was written (ms).
    pub built_at: i64,
    pub units: Vec<String>,
    pub traits: Vec<String>,
    pub items: Vec<String>,
    pub comps: Vec<String>,
    /// matches.queue_id values; None is a match stored before queues were recorded (ranked).
    pub queues: Vec<Option<i64>>,
    pub rows_at: u64,
    pub units_at: u64,
    pub units_n: u64,
    pub traits_at: u64,
    pub traits_n: u64,
    pub items_at: u64,
    pub items_n: u64,
    pub table_at: u64,
    pub table_n: u64,
}

/// Counts bytes written, and pads to a multiple of 8.
struct Out<W: Write> {
    w: W,
    at: u64,
}
impl<W: Write> Out<W> {
    fn put(&mut self, b: &[u8]) -> std::io::Result<()> {
        self.w.write_all(b)?;
        self.at += b.len() as u64;
        Ok(())
    }
    fn align(&mut self) -> std::io::Result<()> {
        while self.at % 8 != 0 {
            self.put(&[0])?;
        }
        Ok(())
    }
}

/// The parts of an index, in memory, ready to write.
pub struct Parts<'a> {
    pub dir: Dir,
    pub rows: &'a [Row],
    pub units: &'a [u16],
    pub traits: &'a [u16],
    pub items: &'a [u32],
    /// Sorted by key.
    pub bitmaps: Vec<(u64, RoaringBitmap)>,
}

/// Writes a complete file at `path` (a temporary name; the caller renames it into place).
pub fn write(path: &Path, mut p: Parts) -> Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut o = Out { w: BufWriter::with_capacity(1 << 20, file), at: 0 };
    o.put(&[0; HEADER as usize])?;

    p.dir.version = FORMAT_VERSION;
    p.dir.rows = p.rows.len() as u64;
    p.dir.rows_at = o.at;
    for r in p.rows {
        r.write(&mut o.w)?;
    }
    o.at += 40 * p.rows.len() as u64;
    let put_u16s = |o: &mut Out<_>, xs: &[u16]| -> std::io::Result<(u64, u64)> {
        o.align()?;
        let at = o.at;
        for x in xs {
            o.put(&x.to_le_bytes())?;
        }
        Ok((at, xs.len() as u64))
    };
    (p.dir.units_at, p.dir.units_n) = put_u16s(&mut o, p.units)?;
    (p.dir.traits_at, p.dir.traits_n) = put_u16s(&mut o, p.traits)?;
    o.align()?;
    p.dir.items_at = o.at;
    p.dir.items_n = p.items.len() as u64;
    for x in p.items {
        o.put(&x.to_le_bytes())?;
    }

    o.align()?;
    let mut table = Vec::with_capacity(p.bitmaps.len());
    for (k, b) in &mut p.bitmaps {
        b.optimize();
        let at = o.at;
        let mut buf = Vec::with_capacity(b.serialized_size());
        b.serialize_into(&mut buf)?;
        o.put(&buf)?;
        table.push([*k, at, buf.len() as u64]);
    }
    ensure!(table.windows(2).all(|w| w[0][0] < w[1][0]), "bitmap keys out of order");
    o.align()?;
    p.dir.table_at = o.at;
    p.dir.table_n = table.len() as u64;
    for t in &table {
        for x in t {
            o.put(&x.to_le_bytes())?;
        }
    }
    let dir_at = o.at;
    let dir = serde_json::to_vec(&p.dir)?;
    o.put(&dir)?;
    let total = o.at + FOOTER;
    o.put(END)?;
    o.put(&dir_at.to_le_bytes())?;
    o.put(&total.to_le_bytes())?;

    let mut file = o.w.into_inner().map_err(|e| e.into_error())?;
    let mut head = Vec::with_capacity(HEADER as usize);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    head.extend_from_slice(&0u32.to_le_bytes());
    head.extend_from_slice(&dir_at.to_le_bytes());
    head.extend_from_slice(&(dir.len() as u64).to_le_bytes());
    head.extend_from_slice(&total.to_le_bytes());
    head.resize(HEADER as usize, 0);
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)?;
    file.sync_all()?;
    Ok(())
}

fn u64_at(b: &[u8], at: u64) -> u64 {
    u64::from_le_bytes(b[at as usize..at as usize + 8].try_into().unwrap())
}

/// A slice of the mapped file as `T`s. The sections are written 8-byte aligned and the map starts on a page.
fn cast<T>(bytes: &[u8], at: u64, n: u64) -> Result<&[T]> {
    let size = std::mem::size_of::<T>() as u64;
    let end = n.checked_mul(size).and_then(|len| at.checked_add(len)).context("section size overflows")?;
    ensure!(end <= bytes.len() as u64, "section runs past the end of the file");
    let ptr = bytes[at as usize..end as usize].as_ptr();
    ensure!((ptr as usize) % std::mem::align_of::<T>() == 0, "section is not aligned");
    // SAFETY: in bounds and aligned (checked above); Row, u16 and u32 are plain data with no invalid bit patterns, the
    // file is little-endian (checked at open), and index files are never changed once written.
    Ok(unsafe { std::slice::from_raw_parts(ptr as *const T, n as usize) })
}

/// An open index file. Cheap to open: the file is mapped and only the directory is read. Bitmaps are read from the
/// map the first time a query needs them and kept.
pub struct Index {
    map: Mmap,
    pub dir: Dir,
    pub path: PathBuf,
    pub generation: u64,
    unit_ids: HashMap<String, u16>,
    trait_ids: HashMap<String, u16>,
    item_ids: HashMap<String, u16>,
    bitmaps: Mutex<HashMap<u64, Arc<RoaringBitmap>>>,
    pub(crate) scopes: Mutex<HashMap<(i64, i64), Arc<RoaringBitmap>>>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index").field("path", &self.path).field("rows", &self.dir.rows).finish()
    }
}

fn ids(names: &[String]) -> HashMap<String, u16> {
    names.iter().enumerate().map(|(i, n)| (n.clone(), i as u16)).collect()
}

impl Index {
    /// Maps and checks a file. Fails on anything not written whole by this format version.
    pub fn open(path: &Path) -> Result<Index> {
        ensure!(cfg!(target_endian = "little"), "index files are little-endian");
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        // SAFETY: index files are written under a temporary name and never changed after the rename; a newer
        // snapshot is always a new file.
        let map = unsafe { Mmap::map(&file) }.with_context(|| format!("mapping {}", path.display()))?;
        let b = &map[..];
        let len = b.len() as u64;
        ensure!(len >= HEADER + FOOTER, "too short to be an index file");
        ensure!(&b[..8] == MAGIC, "not an index file");
        let version = u32::from_le_bytes(b[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            bail!("index format {version}, this version reads {FORMAT_VERSION}");
        }
        let (dir_at, dir_len, total) = (u64_at(b, 16), u64_at(b, 24), u64_at(b, 32));
        ensure!(total == len, "cut short: {len} of {total} bytes");
        let foot = (len - FOOTER) as usize;
        ensure!(&b[foot..foot + 8] == END, "no end marker");
        ensure!(u64_at(b, len - 16) == dir_at && u64_at(b, len - 8) == total, "header and end marker disagree");
        ensure!(dir_at.checked_add(dir_len).is_some_and(|e| e == len - FOOTER), "directory out of place");
        let dir: Dir = serde_json::from_slice(&b[dir_at as usize..(dir_at + dir_len) as usize]).context("reading the directory")?;
        ensure!(dir.version == FORMAT_VERSION, "directory format {}", dir.version);
        ensure!(dir.units.len() <= 1 << 16 && dir.traits.len() <= 1 << 16 && dir.items.len() <= 1 << 16, "dictionary too large");
        ensure!(dir.queues.len() <= 256, "too many queues");
        let generation = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit_once('.'))
            .and_then(|(_, g)| g.parse().ok())
            .unwrap_or(0);
        let ix = Index {
            unit_ids: ids(&dir.units),
            trait_ids: ids(&dir.traits),
            item_ids: ids(&dir.items),
            map,
            dir,
            path: path.to_path_buf(),
            generation,
            bitmaps: Mutex::default(),
            scopes: Mutex::default(),
        };
        // Every section in bounds and aligned. The rows and arrays are not read here (that would read the whole
        // file): the query code never trusts an offset or id from them without a bounds check.
        ix.try_rows()?;
        ix.try_units()?;
        ix.try_traits()?;
        ix.try_items()?;
        let table = ix.try_table()?;
        ensure!(table.windows(2).all(|w| w[0][0] < w[1][0]), "bitmap table out of order");
        for e in table {
            ensure!(e[1].checked_add(e[2]).is_some_and(|end| end <= ix.dir.table_at), "bitmap out of place");
        }
        Ok(ix)
    }

    fn try_rows(&self) -> Result<&[Row]> {
        cast(&self.map, self.dir.rows_at, self.dir.rows)
    }
    fn try_units(&self) -> Result<&[u16]> {
        cast(&self.map, self.dir.units_at, self.dir.units_n)
    }
    fn try_traits(&self) -> Result<&[u16]> {
        cast(&self.map, self.dir.traits_at, self.dir.traits_n)
    }
    fn try_items(&self) -> Result<&[u32]> {
        cast(&self.map, self.dir.items_at, self.dir.items_n)
    }
    fn try_table(&self) -> Result<&[[u64; 3]]> {
        cast(&self.map, self.dir.table_at, self.dir.table_n)
    }

    // Checked at open, so these cannot fail afterwards.
    pub fn rows(&self) -> &[Row] {
        self.try_rows().expect("checked at open")
    }
    pub fn units(&self) -> &[u16] {
        self.try_units().expect("checked at open")
    }
    pub fn traits(&self) -> &[u16] {
        self.try_traits().expect("checked at open")
    }
    pub fn items(&self) -> &[u32] {
        self.try_items().expect("checked at open")
    }
    fn table(&self) -> &[[u64; 3]] {
        self.try_table().expect("checked at open")
    }

    pub fn unit_id(&self, name: &str) -> Option<u16> {
        self.unit_ids.get(name).copied()
    }
    pub fn trait_id(&self, name: &str) -> Option<u16> {
        self.trait_ids.get(name).copied()
    }
    pub fn item_id(&self, name: &str) -> Option<u16> {
        self.item_ids.get(name).copied()
    }

    /// The size of the file (all of it is mapped; only the pages queries touch are read).
    pub fn bytes(&self) -> u64 {
        self.map.len() as u64
    }

    /// Heap held by the bitmaps read so far.
    pub fn heap_bytes(&self) -> u64 {
        self.bitmaps.lock().unwrap().values().map(|b| b.serialized_size() as u64).sum::<u64>()
            + self.scopes.lock().unwrap().values().map(|b| b.serialized_size() as u64).sum::<u64>()
    }

    /// The bitmap for `key`, empty when nothing has it.
    pub fn bitmap(&self, key: u64) -> Arc<RoaringBitmap> {
        if let Some(b) = self.bitmaps.lock().unwrap().get(&key) {
            return b.clone();
        }
        let table = self.table();
        let b = match table.binary_search_by_key(&key, |e| e[0]) {
            Ok(i) => {
                let [_, at, len] = table[i];
                RoaringBitmap::deserialize_from(&self.map[at as usize..(at + len) as usize]).unwrap_or_default()
            }
            Err(_) => RoaringBitmap::new(),
        };
        let b = Arc::new(b);
        self.bitmaps.lock().unwrap().insert(key, b.clone());
        b
    }

    /// The `(a, b)` of every bitmap of one kind.
    pub fn keys(&self, kind: Kind) -> impl Iterator<Item = (u32, u32)> + '_ {
        let table = self.table();
        let lo = table.partition_point(|e| (e[0] >> 56) < kind as u64);
        let hi = table.partition_point(|e| (e[0] >> 56) <= kind as u64);
        table[lo..hi].iter().map(|e| (((e[0] >> 32) & 0xff_ffff) as u32, e[0] as u32))
    }
}

/// Row `r`'s entries in `array`, given each row's start. Clamped, so a damaged file gives an empty range, not a panic.
pub(crate) fn entries<'a, T>(rows: &[Row], r: usize, array: &'a [T], start: impl Fn(&Row) -> u32) -> &'a [T] {
    let lo = (start(&rows[r]) as usize).min(array.len());
    let hi = rows.get(r + 1).map_or(array.len(), |n| (start(n) as usize).min(array.len())).max(lo);
    &array[lo..hi]
}
