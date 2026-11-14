//! One pass over a patch's boards in memory, for the stats that were slow as SQL self-joins: item pairs,
//! teammates and trait unit sets (`unit_depth`), and the flex tables (`flex`).
//!
//! [`load`] reads the boards tagged in `inc_b` once per patch, with their units, items and traits, and fills the
//! `inc_dep_*` tables with the same rows the SQL in `unit_depth::SUMS` gives when it is grouped by rank band and
//! time window. It returns the boards and the units on each, which [`crate::flex::write_fast`] counts per scope.
//!
//! Text is compared the way SQLite compares it in those queries: byte order for ids (the columns have no
//! collation), ASCII case folded for the set data lookups (NOCASE) and for LIKE.

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};

/// Strings to small numbers and back.
#[derive(Default)]
pub(crate) struct Names {
    ids: HashMap<String, u32>,
    pub names: Vec<String>,
}

impl Names {
    pub fn id(&mut self, s: &str) -> u32 {
        if let Some(i) = self.ids.get(s) {
            return *i;
        }
        let i = self.names.len() as u32;
        self.ids.insert(s.to_string(), i);
        self.names.push(s.to_string());
        i
    }
    pub fn get(&self, s: &str) -> Option<u32> {
        self.ids.get(s).copied()
    }
    pub fn name(&self, i: u32) -> &str {
        &self.names[i as usize]
    }
}

/// No comp label.
pub(crate) const NO_COMP: u32 = u32::MAX;

/// The boards of one patch that a build needs (the rows of `inc_b`), in id order, with the units on each.
pub(crate) struct PatchBoards {
    pub ids: Vec<i64>,
    pub band: Vec<u8>,
    pub w: Vec<u8>,
    pub place: Vec<i64>,
    pub level: Vec<Option<i64>>,
    /// The stored comp label, as an index into `comps`, or [`NO_COMP`].
    pub comp: Vec<u32>,
    pub comps: Names,
    pub units: Names,
    /// Board i's different units are `unit_ids[unit_off[i]..unit_off[i + 1]]`.
    unit_off: Vec<u32>,
    unit_ids: Vec<u32>,
}

impl PatchBoards {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn units_on(&self, i: usize) -> &[u32] {
        &self.unit_ids[self.unit_off[i] as usize..self.unit_off[i + 1] as usize]
    }
    /// The position of the board with this participant id.
    pub fn find(&self, id: i64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }
}

/// SQLite's LIKE without an escape character: `%` is any run of characters, `_` any one, ASCII case ignored.
pub(crate) fn like(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('%', rest)) => (0..=t.len()).any(|k| go(rest, &t[k..])),
            Some(('_', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first().is_some_and(|x| x.eq_ignore_ascii_case(c)) && go(rest, &t[1..]),
        }
    }
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    go(&p, &t)
}

/// `substr(s, 1, instr(s, '|') - 1)` and `substr(s, instr(s, '|') + 1)`: the text before and after the first
/// bar. Without a bar the first is empty and the second is all of `s`, as in SQLite.
fn split_bar(s: &str) -> (&str, &str) {
    s.split_once('|').unwrap_or(("", s))
}

type Count = (i64, i64); // games, sum of places

fn bump<K: std::hash::Hash + Eq>(m: &mut HashMap<K, Count>, k: K, place: i64) {
    let e = m.entry(k).or_insert((0, 0));
    e.0 += 1;
    e.1 += place;
}

/// Rows `(board, a, b)` of a child table, grouped per board: board i's rows are `rows[off[i]..off[i + 1]]`.
struct PerBoard<T> {
    off: Vec<u32>,
    rows: Vec<T>,
}

impl<T> PerBoard<T> {
    fn of(&self, i: usize) -> &[T] {
        &self.rows[self.off[i] as usize..self.off[i + 1] as usize]
    }
}

/// Runs `sql` (first column the board id, ordered by it) and groups what `row` makes of each line per board.
fn per_board<T>(
    db: &Connection,
    ids: &[i64],
    sql: &str,
    mut row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Result<PerBoard<T>> {
    let mut off = Vec::with_capacity(ids.len() + 1);
    let mut rows = Vec::new();
    let mut st = db.prepare(sql)?;
    let mut it = st.query([])?;
    let mut i = 0usize;
    off.push(0);
    while let Some(r) = it.next()? {
        let id: i64 = r.get(0)?;
        while i < ids.len() && ids[i] < id {
            i += 1;
            off.push(rows.len() as u32);
        }
        anyhow::ensure!(i < ids.len() && ids[i] == id, "board {id} is not in inc_b, or the rows are out of order");
        rows.push(row(r)?);
    }
    while off.len() < ids.len() + 1 {
        off.push(rows.len() as u32);
    }
    Ok(PerBoard { off, rows })
}

/// Reads the boards in `inc_b` and writes `inc_dep_pairs`, `inc_dep_teammates` and `inc_dep_trait_sets` (sums per
/// rank band and time window). Needs `dep_no_items` and `dep_unit_traits` (`unit_depth::prepare`).
pub(crate) fn load(db: &Connection) -> Result<PatchBoards> {
    let mut pb = PatchBoards {
        ids: Vec::new(),
        band: Vec::new(),
        w: Vec::new(),
        place: Vec::new(),
        level: Vec::new(),
        comp: Vec::new(),
        comps: Names::default(),
        units: Names::default(),
        unit_off: vec![0],
        unit_ids: Vec::new(),
    };
    {
        let mut st = db.prepare("SELECT id, placement, level, comp_key, band, w FROM inc_b ORDER BY id")?;
        let mut it = st.query([])?;
        while let Some(r) = it.next()? {
            pb.ids.push(r.get(0)?);
            pb.place.push(r.get(1)?);
            pb.level.push(r.get(2)?);
            let comp = match r.get_ref(3)? {
                rusqlite::types::ValueRef::Null => NO_COMP,
                v => pb.comps.id(v.as_str()?),
            };
            pb.comp.push(comp);
            pb.band.push(r.get(4)?);
            pb.w.push(r.get(5)?);
        }
    }

    // The child rows, as numbers. Item id 0 is '', the "any third item" of a pair row.
    let mut items = Names::default();
    items.id("");
    let mut builds = Names::default();
    let mut traits = Names::default();
    const NO_BUILD: u32 = u32::MAX;
    let units = {
        let names = &mut pb.units;
        per_board(
            db,
            &pb.ids,
            "SELECT b.id, u.character_id, u.build FROM inc_b b CROSS JOIN units u ON u.participant_id = b.id ORDER BY b.id",
            |r| {
                let unit = names.id(r.get_ref(1)?.as_str()?);
                let build = match r.get_ref(2)? {
                    rusqlite::types::ValueRef::Null => NO_BUILD,
                    v => builds.id(v.as_str()?),
                };
                Ok((unit, build))
            },
        )?
    };
    let held = {
        let names = &mut pb.units;
        per_board(
            db,
            &pb.ids,
            "SELECT b.id, ui.character_id, ui.item FROM inc_b b CROSS JOIN unit_items ui ON ui.participant_id = b.id ORDER BY b.id",
            |r| Ok((names.id(r.get_ref(1)?.as_str()?), items.id(r.get_ref(2)?.as_str()?))),
        )?
    };
    let active = per_board(
        db,
        &pb.ids,
        "SELECT b.id, t.name, t.tier_current FROM inc_b b CROSS JOIN traits t ON t.participant_id = b.id ORDER BY b.id",
        |r| Ok((traits.id(r.get_ref(1)?.as_str()?), r.get::<_, i64>(2)?)),
    )?;

    // Lookups: which units hold items, each build's three items, which units carry which trait.
    let patterns: Vec<String> =
        db.prepare("SELECT pat FROM dep_no_items")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let holds: Vec<bool> = pb.units.names.iter().map(|u| !patterns.iter().any(|p| like(p, u))).collect();
    let build_items: Vec<[u32; 3]> = builds
        .names
        .iter()
        .map(|b| {
            let (i1, rest) = split_bar(b);
            let (i2, i3) = split_bar(rest);
            [items.id(i1), items.id(i2), items.id(i3)]
        })
        .collect();
    let carried: HashSet<(String, String)> = db
        .prepare("SELECT unit, trait FROM dep_unit_traits")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?.to_ascii_lowercase(), r.get::<_, String>(1)?.to_ascii_lowercase())))?
        .collect::<rusqlite::Result<_>>()?;
    let lower_units: Vec<String> = pb.units.names.iter().map(|s| s.to_ascii_lowercase()).collect();
    let lower_traits: Vec<String> = traits.names.iter().map(|s| s.to_ascii_lowercase()).collect();
    let mut carries: HashMap<(u32, u32), bool> = HashMap::new();

    // (tag, ...) keys, where the tag is band and window in one number.
    let mut pairs: HashMap<(u8, u32, u32, u32, u32), Count> = HashMap::new();
    let mut mates: HashMap<(u8, u32, u32), Count> = HashMap::new();
    let mut sets: HashMap<(u8, u32, i64, u32), Count> = HashMap::new();
    let mut set_ids: HashMap<Vec<u32>, u32> = HashMap::new();
    let mut set_list: Vec<Vec<u32>> = Vec::new();
    let mut keys: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut on: Vec<u32> = Vec::new();
    let mut levels: Vec<(u32, i64)> = Vec::new();

    for i in 0..pb.len() {
        let (tag, place) = (pb.band[i] * 16 + pb.w[i], pb.place[i]);
        on.clear();
        on.extend(units.of(i).iter().map(|u| u.0));
        on.sort_unstable();
        on.dedup();

        // Teammates: each pair of different units once.
        for (k, a) in on.iter().enumerate() {
            for b in &on[k + 1..] {
                bump(&mut mates, (tag, *a, *b), place);
            }
        }

        // Pairs with any third item: two item rows of the same unit, items pooled across its copies.
        keys.clear();
        let mine = held.of(i);
        for (k, (unit, x)) in mine.iter().enumerate() {
            if !holds[*unit as usize] {
                continue;
            }
            for (other, y) in &mine[k + 1..] {
                if other == unit {
                    let (a, b) = if items.name(*x) <= items.name(*y) { (*x, *y) } else { (*y, *x) };
                    keys.push((*unit, a, b, 0));
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        for k in &keys {
            bump(&mut pairs, (tag, k.0, k.1, k.2, k.3), place);
        }
        // Pairs with their third item: each copy's own three-item build, each item in turn as the third.
        keys.clear();
        for (unit, build) in units.of(i) {
            if *build == NO_BUILD || !holds[*unit as usize] {
                continue;
            }
            let [i1, i2, i3] = build_items[*build as usize];
            keys.extend([(*unit, i2, i3, i1), (*unit, i1, i3, i2), (*unit, i1, i2, i3)]);
        }
        keys.sort_unstable();
        keys.dedup();
        for k in &keys {
            bump(&mut pairs, (tag, k.0, k.1, k.2, k.3), place);
        }

        // Trait unit sets: per active trait level, the board's units that carry the trait.
        levels.clear();
        levels.extend_from_slice(active.of(i));
        levels.sort_unstable();
        levels.dedup();
        for (t, tier) in &levels {
            let with: Vec<u32> = on
                .iter()
                .copied()
                .filter(|u| {
                    *carries.entry((*u, *t)).or_insert_with(|| {
                        carried.contains(&(lower_units[*u as usize].clone(), lower_traits[*t as usize].clone()))
                    })
                })
                .collect();
            if with.is_empty() {
                continue;
            }
            let set = match set_ids.get(&with) {
                Some(s) => *s,
                None => {
                    let s = set_list.len() as u32;
                    set_ids.insert(with.clone(), s);
                    set_list.push(with);
                    s
                }
            };
            bump(&mut sets, (tag, *t, *tier, set), place);
        }

        pb.unit_ids.extend_from_slice(&on);
        pb.unit_off.push(pb.unit_ids.len() as u32);
    }

    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS inc_dep_pairs (band, w, character_id, item_a, item_b, item_c, games, sum_place);
         CREATE TEMP TABLE IF NOT EXISTS inc_dep_teammates (band, w, character_id, other, games, sum_place);
         CREATE TEMP TABLE IF NOT EXISTS inc_dep_trait_sets (band, w, name, tier_current, units, games, sum_place);
         DELETE FROM inc_dep_pairs; DELETE FROM inc_dep_teammates; DELETE FROM inc_dep_trait_sets;",
    )?;
    let untag = |tag: u8| ((tag / 16) as i64, (tag % 16) as i64);
    {
        let mut put = db.prepare("INSERT INTO inc_dep_pairs VALUES (?, ?, ?, ?, ?, ?, ?, ?)")?;
        for ((tag, unit, a, b, c), (games, sum)) in &pairs {
            let (band, w) = untag(*tag);
            put.execute(params![band, w, pb.units.name(*unit), items.name(*a), items.name(*b), items.name(*c), games, sum])?;
        }
        // Each pair was counted once; it is written both ways.
        let mut put = db.prepare("INSERT INTO inc_dep_teammates VALUES (?, ?, ?, ?, ?, ?)")?;
        for ((tag, a, b), (games, sum)) in &mates {
            let (band, w) = untag(*tag);
            let (a, b) = (pb.units.name(*a), pb.units.name(*b));
            put.execute(params![band, w, a, b, games, sum])?;
            put.execute(params![band, w, b, a, games, sum])?;
        }
        let mut put = db.prepare("INSERT INTO inc_dep_trait_sets VALUES (?, ?, ?, ?, ?, ?, ?)")?;
        let set_text: Vec<String> = set_list
            .iter()
            .map(|set| {
                let mut names: Vec<&str> = set.iter().map(|u| pb.units.name(*u)).collect();
                names.sort_unstable();
                names.join("|")
            })
            .collect();
        for ((tag, t, tier, set), (games, sum)) in &sets {
            let (band, w) = untag(*tag);
            put.execute(params![band, w, traits.name(*t), tier, set_text[*set as usize], games, sum])?;
        }
    }
    Ok(pb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_matches_as_sqlite_does() {
        let conn = Connection::open_in_memory().unwrap();
        for (p, t) in [
            ("%summon%", "TFT90_SummonU07"),
            ("%summon%", "TFT90_Sumon"),
            ("%trainingdummy%", "TFT_TrainingDummy"),
            ("a_c", "AbC"),
            ("a_c", "ac"),
            ("%", ""),
            ("_%", ""),
            ("abc", "abcd"),
            ("%b%d", "abcd"),
            ("%é_", "caféx"),
        ] {
            let want: bool = conn.query_row("SELECT ?2 LIKE ?1", [p, t], |r| r.get(0)).unwrap();
            assert_eq!(like(p, t), want, "{t:?} LIKE {p:?}");
        }
    }

    #[test]
    fn a_build_splits_at_its_first_two_bars() {
        let conn = Connection::open_in_memory().unwrap();
        for b in ["A|B|C", "A|B", "A", "", "A|B|C|D", "|B|"] {
            let want: (String, String, String) = conn
                .query_row(
                    "SELECT substr(?1, 1, instr(?1, '|') - 1), substr(rest, 1, instr(rest, '|') - 1), substr(rest, instr(rest, '|') + 1)
                     FROM (SELECT substr(?1, instr(?1, '|') + 1) rest)",
                    [b],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            let (i1, rest) = split_bar(b);
            let (i2, i3) = split_bar(rest);
            assert_eq!((i1, i2, i3), (want.0.as_str(), want.1.as_str(), want.2.as_str()), "{b:?}");
        }
    }
}
