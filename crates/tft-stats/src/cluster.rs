//! Comps grouped by the units they share (PRD 2 C1), their core and flex units and variants (C2), and ids made
//! from the core (C13). "How clustering works" in docs/plan/prd-02-comps-engine.md, steps 1 to 7:
//!
//! 1. Signature: each board is a weighted set of units. Weight = cost, plus [`ITEM_BONUS2`] for a unit holding
//!    three items, plus [`STAR_BONUS2`] for a three-star unit. Weights are kept in half points, as integers, so
//!    sums do not depend on the order boards are added in (an incremental build must equal a full one).
//! 2. Bucket: boards go in a bucket by their two heaviest units (ties: unit id order). Boards with under four
//!    units are not a comp, as in the baseline rule.
//! 3. Merge: buckets with at least [`MIN_BUCKET`] boards are merged by weighted Jaccard of their mean signatures,
//!    the most similar pair first, while the best pair is at least [`MERGE_SIM`] alike. Two groups whose
//!    three-star units differ are never merged (the reroll comp and its fast-9 cousin, PRD edge case).
//!    A group is listed when it has enough games and a core of at least [`MIN_CORE`] units. Buckets left in an
//!    unlisted group (small ones included) join the most alike listed comp if at least [`ASSIGN_SIM`] alike, so
//!    a board with no three-item unit is placed by its units alone. Then any two listed comps whose cores
//!    overlap more than [`SEPARATION`] are folded together, the smaller into the bigger.
//! 4. Core: units on at least [`CORE_SHARE`] of a comp's boards. Flex: from [`FLEX_SHARE`] up to the core share.
//!    A core unit three-starred on at least half the boards is written `id:3`, so a reroll core differs from
//!    the same units played at two stars.
//! 5. Variants: see [`variants`](crate::variants).
//! 6. Name: worked out by the API from the comp's traits and carries (top trait by unit count, then the
//!    three-item units by frequency); your renames (C14) win.
//! 7. Identity: a comp id is a hash of its sorted core. Across patches, [`lineage`](crate::lineage) links it to
//!    last patch's comp with the highest core overlap above 60%.
//!
//! Today's rule ("biggest trait | main carry", look-alikes merged at 75%) stays the default. This method runs
//! only when the kv setting [`COMP_METHOD_KEY`] is `clusters`. Membership is not stored: participants.comp_key
//! keeps the baseline label, and the clusters are worked out again on every build from the boards (the same
//! boards always give the same comps), into the temporary table the stat queries read.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use tft_core::cluster::rarity_cost;

/// kv key for which comp method the stats use: `"baseline"` (default) or `"clusters"`.
pub const COMP_METHOD_KEY: &str = "comp_method";

/// Signature bonus, in half points, for a unit holding three items (3 points).
pub const ITEM_BONUS2: u32 = 6;
/// Signature bonus, in half points, for a three-star unit (1 point).
pub const STAR_BONUS2: u32 = 2;
/// Buckets this alike (weighted Jaccard of mean signatures) or more are merged.
pub const MERGE_SIM: f64 = 0.55;
/// A bucket outside every listed comp joins the most alike one if at least this alike.
pub const ASSIGN_SIM: f64 = 0.35;
/// Core units are on at least this share of a comp's boards.
pub const CORE_SHARE: f64 = 0.70;
/// Flex units are on at least this share (and under the core share).
pub const FLEX_SHARE: f64 = 0.20;
/// No two listed comps may share more than this much of their cores (Jaccard).
pub const SEPARATION: f64 = 0.75;
/// A unit three-starred on at least this share of a group's boards marks the group as built around it.
pub const STAR3_SHARE: f64 = 0.5;
/// A listed comp needs at least this many core units.
pub const MIN_CORE: usize = 3;
/// Buckets smaller than this many boards take no part in merging; they can still join a listed comp.
pub const MIN_BUCKET: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Baseline,
    Clusters,
}

impl Method {
    pub fn parse(s: &str) -> Option<Method> {
        match s.trim().trim_matches('"') {
            "baseline" => Some(Method::Baseline),
            "clusters" => Some(Method::Clusters),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Baseline => "baseline",
            Method::Clusters => "clusters",
        }
    }
}

/// The comp method the stats are built with. Anything unreadable counts as the baseline.
pub fn comp_method(db: &Connection) -> Result<Method> {
    let raw: Option<String> =
        db.query_row("SELECT value FROM kv WHERE key = ?", [COMP_METHOD_KEY], |r| r.get(0)).optional()?;
    Ok(raw.as_deref().and_then(Method::parse).unwrap_or(Method::Baseline))
}

/// Saves the comp method (as JSON, like every kv value). When it changes, every stats scope is marked as needing
/// a full rebuild, since the stored comps were made by the other method. Returns whether it changed.
pub fn set_comp_method(db: &Connection, method: Method) -> Result<bool> {
    let changed = comp_method(db)? != method;
    db.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![COMP_METHOD_KEY, format!("\"{}\"", method.as_str())],
    )?;
    if changed {
        db.execute("DELETE FROM stat_watermark", [])?;
    }
    Ok(changed)
}

// ---- Boards ----------------------------------------------------------------------

/// Boards in a compact form: per board its participant id and placement, and its units (each counted once).
/// Unit numbers follow the unit ids' sorted order, so ties break the same way on every build.
#[derive(Default)]
pub struct Boards {
    pub ids: Vec<i64>,
    pub place: Vec<u8>,
    /// Board i's units are `start[i]..start[i + 1]`.
    pub start: Vec<u32>,
    pub unit: Vec<u16>,
    pub cost: Vec<u8>,
    pub star: Vec<u8>,
    pub items: Vec<u8>,
    /// Unit number to unit id.
    pub names: Vec<String>,
}

/// Builds [`Boards`] from unit rows grouped by board.
#[derive(Default)]
pub struct BoardsBuilder {
    b: Boards,
    seen: HashMap<String, u16>,
}

impl BoardsBuilder {
    /// One unit of one board. Rows of a board must come together; a unit seen twice on a board keeps its
    /// highest star and item count, as the stat queries' MAX() does.
    pub fn push(&mut self, id: i64, place: i64, unit: &str, star: i64, items: i64, cost: i64) {
        let b = &mut self.b;
        if b.ids.last() != Some(&id) {
            b.ids.push(id);
            b.place.push(place.clamp(0, 255) as u8);
            b.start.push(b.unit.len() as u32);
        }
        let next = self.seen.len() as u16;
        let u = *self.seen.entry(unit.to_string()).or_insert_with(|| {
            b.names.push(unit.to_string());
            next
        });
        let from = *b.start.last().unwrap() as usize;
        if let Some(k) = (from..b.unit.len()).find(|&k| b.unit[k] == u) {
            b.star[k] = b.star[k].max(star.clamp(0, 255) as u8);
            b.items[k] = b.items[k].max(items.clamp(0, 255) as u8);
            return;
        }
        b.unit.push(u);
        b.cost.push(cost.clamp(0, 255) as u8);
        b.star.push(star.clamp(0, 255) as u8);
        b.items.push(items.clamp(0, 255) as u8);
    }

    pub fn finish(mut self) -> Boards {
        let b = &mut self.b;
        b.start.push(b.unit.len() as u32);
        let mut order: Vec<usize> = (0..b.names.len()).collect();
        order.sort_by(|x, y| b.names[*x].cmp(&b.names[*y]));
        let mut remap = vec![0u16; order.len()];
        for (new, old) in order.iter().enumerate() {
            remap[*old] = new as u16;
        }
        for u in &mut b.unit {
            *u = remap[*u as usize];
        }
        b.names = order.iter().map(|i| b.names[*i].clone()).collect();
        self.b
    }
}

impl Boards {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn units(&self, i: usize) -> std::ops::Range<usize> {
        self.start[i] as usize..self.start[i + 1] as usize
    }
    /// Signature weight of one unit entry, in half points.
    pub fn weight(&self, k: usize) -> u32 {
        2 * self.cost[k] as u32
            + if self.items[k] >= 3 { ITEM_BONUS2 } else { 0 }
            + if self.star[k] >= 3 { STAR_BONUS2 } else { 0 }
    }

    /// The boards in the temporary table `table` (columns id, placement), with their units, in id order.
    pub fn load(db: &Connection, table: &str) -> Result<Boards> {
        let mut st = db.prepare(&format!(
            "SELECT p.id, p.placement, u.character_id, u.star, u.n_items, u.rarity
             FROM {table} p JOIN units u ON u.participant_id = p.id ORDER BY p.id"
        ))?;
        let mut rows = st.query([])?;
        let mut b = BoardsBuilder::default();
        while let Some(r) = rows.next()? {
            let rarity: Option<i64> = r.get(5)?;
            let unit: String = r.get(2)?;
            b.push(r.get(0)?, r.get(1)?, &unit, r.get(3)?, r.get(4)?, rarity_cost(rarity).unwrap_or(1));
        }
        Ok(b.finish())
    }

    /// Index of the board with this participant id.
    pub fn find(&self, id: i64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }
}

// ---- Groups ----------------------------------------------------------------------

/// Sums over a group of boards: count, signature weight per unit, boards with the unit, boards with it at 3 stars.
#[derive(Clone)]
pub(crate) struct Acc {
    pub n: u32,
    pub w: Vec<u64>,
    pub pres: Vec<u32>,
    pub star3: Vec<u32>,
}

impl Acc {
    pub fn new(d: usize) -> Acc {
        Acc { n: 0, w: vec![0; d], pres: vec![0; d], star3: vec![0; d] }
    }
    pub fn add_board(&mut self, b: &Boards, i: usize) {
        self.n += 1;
        for k in b.units(i) {
            let u = b.unit[k] as usize;
            self.w[u] += b.weight(k) as u64;
            self.pres[u] += 1;
            if b.star[k] >= 3 {
                self.star3[u] += 1;
            }
        }
    }
    pub fn absorb(&mut self, o: &Acc) {
        self.n += o.n;
        for u in 0..self.w.len() {
            self.w[u] += o.w[u];
            self.pres[u] += o.pres[u];
            self.star3[u] += o.star3[u];
        }
    }
    /// Units three-starred on at least [`STAR3_SHARE`] of the boards.
    pub fn star3_set(&self) -> Vec<u16> {
        let n = self.n.max(1) as f64;
        (0..self.w.len()).filter(|u| self.star3[*u] as f64 / n >= STAR3_SHARE).map(|u| u as u16).collect()
    }
    /// The core: units on at least [`CORE_SHARE`] of the boards, by unit number.
    pub fn core(&self) -> Vec<u16> {
        let n = self.n.max(1) as f64;
        (0..self.w.len()).filter(|u| self.pres[*u] as f64 / n >= CORE_SHARE).map(|u| u as u16).collect()
    }
    /// The core written out: unit ids in sorted order, `:3` after one three-starred on most boards.
    pub fn core_key(&self, names: &[String]) -> Vec<String> {
        let n = self.n.max(1) as f64;
        self.core()
            .into_iter()
            .map(|u| {
                let name = &names[u as usize];
                if self.star3[u as usize] as f64 / n >= STAR3_SHARE { format!("{name}:3") } else { name.clone() }
            })
            .collect()
    }
}

/// Weighted Jaccard of two groups' mean signatures: the sum of the smaller weight per unit over the sum of the
/// bigger one.
pub(crate) fn similarity(a: &Acc, b: &Acc) -> f64 {
    let (na, nb) = (a.n.max(1) as f64, b.n.max(1) as f64);
    let (mut lo, mut hi) = (0.0, 0.0);
    for u in 0..a.w.len() {
        if a.w[u] == 0 && b.w[u] == 0 {
            continue;
        }
        let (x, y) = (a.w[u] as f64 / na, b.w[u] as f64 / nb);
        lo += x.min(y);
        hi += x.max(y);
    }
    if hi == 0.0 { 0.0 } else { lo / hi }
}

/// Jaccard of two cores (or any two sets of names). Two empty sets count as the same.
pub fn overlap(a: &[String], b: &[String]) -> f64 {
    let shared = a.iter().filter(|x| b.contains(x)).count();
    let union = a.len() + b.len() - shared;
    if union == 0 { 1.0 } else { shared as f64 / union as f64 }
}

/// FNV-1a, 64 bits: a hash that is the same on every machine and every Rust version.
pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A comp id from its core: "c" and 12 hex digits.
pub fn comp_id(core_key: &[String]) -> String {
    format!("c{:012x}", fnv(&core_key.join(",")) >> 16)
}

/// One listed comp.
pub struct Comp {
    pub id: String,
    pub core_key: Vec<String>,
    pub games: u32,
}

/// What clustering gives: per board the index of its comp in `comps` (or `None`), and the listed comps,
/// most games first.
pub struct Labels {
    pub boards: Boards,
    pub board_comp: Vec<Option<u32>>,
    pub comps: Vec<Comp>,
}

/// A unit's signature weight and number.
type Heavy = (u32, u16);

/// Clusters the boards (steps 1 to 4 and 7). A comp is listed with at least `min_games` boards; at most
/// `limit` are listed.
pub fn cluster(boards: Boards, min_games: u32, limit: usize) -> Labels {
    let d = boards.names.len();
    let n = boards.len();

    // 1-2. Two heaviest units per board.
    let mut key: Vec<u32> = vec![u32::MAX; n];
    for (i, slot) in key.iter_mut().enumerate() {
        let r = boards.units(i);
        if r.len() < 4 {
            continue;
        }
        let (mut a, mut b): (Option<Heavy>, Option<Heavy>) = (None, None);
        for k in r {
            let c = (boards.weight(k), boards.unit[k]);
            let better = |x: Heavy, y: Option<Heavy>| y.is_none_or(|y| x.0 > y.0 || (x.0 == y.0 && x.1 < y.1));
            if better(c, a) {
                b = a;
                a = Some(c);
            } else if better(c, b) {
                b = Some(c);
            }
        }
        let (x, y) = (a.unwrap().1, b.unwrap().1);
        *slot = ((x.min(y) as u32) << 16) | x.max(y) as u32;
    }
    let mut keys: Vec<u32> = key.iter().copied().filter(|k| *k != u32::MAX).collect();
    keys.sort_unstable();
    keys.dedup();
    let index: HashMap<u32, usize> = keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let board_bucket: Vec<Option<usize>> = key.iter().map(|k| index.get(k).copied()).collect();
    let mut buckets: Vec<Acc> = vec![Acc::new(d); keys.len()];
    for (i, b) in board_bucket.iter().enumerate() {
        if let Some(b) = b {
            buckets[*b].add_board(&boards, i);
        }
    }

    // 3. Merge the bigger buckets, most alike pair first.
    let min_bucket = MIN_BUCKET.max(min_games / 4);
    let big: Vec<usize> = (0..buckets.len()).filter(|b| buckets[*b].n >= min_bucket).collect();
    let mut group: Vec<Option<Acc>> = big.iter().map(|b| Some(buckets[*b].clone())).collect();
    let mut stars: Vec<Vec<u16>> = group.iter().map(|g| g.as_ref().unwrap().star3_set()).collect();
    let mut owner: Vec<usize> = (0..big.len()).collect(); // big bucket -> group
    let m = big.len();
    let pair_sim = |group: &[Option<Acc>], stars: &[Vec<u16>], i: usize, j: usize| -> f64 {
        match (&group[i], &group[j]) {
            (Some(a), Some(b)) if stars[i] == stars[j] => similarity(a, b),
            _ => -1.0,
        }
    };
    let row_best = |group: &[Option<Acc>], stars: &[Vec<u16>], i: usize| -> (f64, usize) {
        let mut best = (-1.0, usize::MAX);
        for j in 0..m {
            if j != i {
                let s = pair_sim(group, stars, i, j);
                if s > best.0 {
                    best = (s, j);
                }
            }
        }
        best
    };
    let mut best: Vec<(f64, usize)> = (0..m).map(|i| row_best(&group, &stars, i)).collect();
    loop {
        // The most alike pair; ties go to the pair with the lowest numbers.
        let mut top: Option<(f64, usize, usize)> = None;
        for (i, (s, j)) in best.iter().enumerate() {
            if group[i].is_none() || *j == usize::MAX || *s < MERGE_SIM {
                continue;
            }
            let (a, b) = (i.min(*j), i.max(*j));
            if top.is_none_or(|t| *s > t.0 || (*s == t.0 && (a, b) < (t.1, t.2))) {
                top = Some((*s, a, b));
            }
        }
        let Some((_, a, b)) = top else { break };
        let gb = group[b].take().unwrap();
        group[a].as_mut().unwrap().absorb(&gb);
        stars[a] = group[a].as_ref().unwrap().star3_set();
        for o in owner.iter_mut() {
            if *o == b {
                *o = a;
            }
        }
        best[b] = (-1.0, usize::MAX);
        best[a] = row_best(&group, &stars, a);
        for k in 0..m {
            if k == a || group[k].is_none() {
                continue;
            }
            if best[k].1 == a || best[k].1 == b {
                best[k] = row_best(&group, &stars, k);
            } else {
                let s = pair_sim(&group, &stars, k, a);
                if s > best[k].0 || (s == best[k].0 && a < best[k].1) {
                    best[k] = (s, a);
                }
            }
        }
    }

    // Listing: most boards first, enough games and a real core.
    let mut alive: Vec<usize> = (0..m).filter(|g| group[*g].is_some()).collect();
    alive.sort_by(|x, y| group[*y].as_ref().unwrap().n.cmp(&group[*x].as_ref().unwrap().n).then(x.cmp(y)));
    let listed: Vec<usize> = alive
        .into_iter()
        .filter(|g| {
            let a = group[*g].as_ref().unwrap();
            a.n >= min_games.max(1) && a.core().len() >= MIN_CORE
        })
        .take(limit)
        .collect();
    // comp slot per group; buckets of listed groups belong to them.
    let mut slot_of_group: HashMap<usize, usize> = HashMap::new();
    for (s, g) in listed.iter().enumerate() {
        slot_of_group.insert(*g, s);
    }
    let mut bucket_slot: Vec<Option<usize>> = vec![None; buckets.len()];
    for (bi, b) in big.iter().enumerate() {
        bucket_slot[*b] = slot_of_group.get(&owner[bi]).copied();
    }
    let mut comps: Vec<Acc> = listed.iter().map(|g| group[*g].clone().unwrap()).collect();

    // Every other bucket joins the most alike listed comp, judged against the comps before any joined.
    let snapshot: Vec<(Acc, Vec<u16>)> = comps.iter().map(|c| (c.clone(), c.star3_set())).collect();
    let mut joins: Vec<(usize, usize)> = Vec::new();
    for (bi, bucket) in buckets.iter().enumerate() {
        if bucket_slot[bi].is_some() {
            continue;
        }
        let stars = bucket.star3_set();
        let mut pick: Option<(f64, usize)> = None;
        for (s, (c, cs)) in snapshot.iter().enumerate() {
            if *cs != stars {
                continue;
            }
            let x = similarity(bucket, c);
            if x >= ASSIGN_SIM && pick.is_none_or(|p| x > p.0) {
                pick = Some((x, s));
            }
        }
        if let Some((_, s)) = pick {
            joins.push((bi, s));
        }
    }
    for (bi, s) in joins {
        bucket_slot[bi] = Some(s);
        comps[s].absorb(&buckets[bi]);
    }

    // Separation: fold comps whose cores overlap too much, the most overlapping pair first.
    let mut slot_alive: Vec<bool> = vec![true; comps.len()];
    let mut redirect: Vec<usize> = (0..comps.len()).collect();
    loop {
        let cores: Vec<Vec<String>> = comps.iter().map(|c| c.core_key(&boards.names)).collect();
        let mut top: Option<(f64, usize, usize)> = None;
        for i in 0..comps.len() {
            for j in i + 1..comps.len() {
                if !slot_alive[i] || !slot_alive[j] {
                    continue;
                }
                let o = overlap(&cores[i], &cores[j]);
                if o > SEPARATION && top.is_none_or(|t| o > t.0) {
                    top = Some((o, i, j));
                }
            }
        }
        let Some((_, i, j)) = top else { break };
        // The smaller folds into the bigger; on a tie the later into the earlier.
        let (keep, gone) = if comps[j].n > comps[i].n { (j, i) } else { (i, j) };
        let g = comps[gone].clone();
        comps[keep].absorb(&g);
        slot_alive[gone] = false;
        for r in redirect.iter_mut() {
            if *r == gone {
                *r = keep;
            }
        }
    }

    // 7. Ids, and the final order: most games first, then id.
    let mut out: Vec<(usize, Comp)> = Vec::new();
    for (s, c) in comps.iter().enumerate() {
        if !slot_alive[s] {
            continue;
        }
        let core_key = c.core_key(&boards.names);
        out.push((s, Comp { id: comp_id(&core_key), core_key, games: c.n }));
    }
    out.sort_by(|a, b| b.1.games.cmp(&a.1.games).then(a.1.id.cmp(&b.1.id)));
    let mut used: HashMap<String, u32> = HashMap::new();
    for (_, c) in out.iter_mut() {
        let n = used.entry(c.id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            c.id = format!("{}-{n}", c.id); // never seen: equal cores overlap fully and are folded together
        }
    }
    let mut final_of_slot: Vec<Option<u32>> = vec![None; comps.len()];
    for (pos, (s, _)) in out.iter().enumerate() {
        final_of_slot[*s] = Some(pos as u32);
    }
    let board_comp: Vec<Option<u32>> = board_bucket
        .iter()
        .map(|b| b.and_then(|b| bucket_slot[b]).and_then(|s| final_of_slot[redirect[s]]))
        .collect();
    Labels { boards, board_comp, comps: out.into_iter().map(|(_, c)| c).collect() }
}

/// Clusters the boards in the temporary table p (id, placement, level, comp_key) and rewrites p with each
/// board's comp id in comp_key (NULL for boards in no listed comp). Inside the caller's transaction.
pub(crate) fn label(db: &Connection, min_games: i64, limit: i64) -> Result<Labels> {
    let rows: Vec<(i64, i64, Option<i64>)> = db
        .prepare("SELECT id, placement, level FROM p ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let boards = Boards::load(db, "p")?;
    let labels = cluster(boards, min_games.clamp(0, u32::MAX as i64) as u32, limit.max(0) as usize);
    db.execute("DELETE FROM p", [])?;
    let mut put = db.prepare("INSERT INTO p (id, placement, level, comp_key) VALUES (?, ?, ?, ?)")?;
    let mut k = 0;
    for (id, place, level) in rows {
        while k < labels.boards.len() && labels.boards.ids[k] < id {
            k += 1;
        }
        let comp = (k < labels.boards.len() && labels.boards.ids[k] == id)
            .then(|| labels.board_comp[k])
            .flatten()
            .map(|c| labels.comps[c as usize].id.as_str());
        put.execute(params![id, place, level, comp])?;
    }
    Ok(labels)
}

/// A JSON list of strings.
pub fn json_list<S: AsRef<str>>(xs: &[S]) -> String {
    let parts: Vec<String> = xs
        .iter()
        .map(|x| {
            let mut s = String::from("\"");
            for c in x.as_ref().chars() {
                match c {
                    '"' => s.push_str("\\\""),
                    '\\' => s.push_str("\\\\"),
                    c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
                    c => s.push(c),
                }
            }
            s.push('"');
            s
        })
        .collect();
    format!("[{}]", parts.join(","))
}

/// Reads a list written by [`json_list`].
pub fn parse_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (&mut cur, c) {
            (None, '"') => cur = Some(String::new()),
            (Some(_), '"') => out.push(cur.take().unwrap()),
            (Some(v), '\\') => match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        v.push(ch);
                    }
                }
                Some(e) => v.push(e),
                None => {}
            },
            (Some(v), c) => v.push(c),
            (None, _) => {}
        }
    }
    out
}

/// After the stat rows are written: core and flex on each listed comp, and its variants (step 5), from the
/// boards each comp ended up with (your merges included). Reads p and topc; inside the caller's transaction.
pub(crate) fn write_shape(db: &Connection, patch: &str, scope: &str, labels: &Labels) -> Result<()> {
    let b = &labels.boards;
    let d = b.names.len();
    let mut members: HashMap<String, Vec<usize>> = HashMap::new();
    {
        let mut st = db.prepare("SELECT p.id, p.comp_key FROM p JOIN topc USING (comp_key) ORDER BY p.id")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let (id, key): (i64, String) = (r.get(0)?, r.get(1)?);
            if let Some(i) = b.find(id) {
                members.entry(key).or_default().push(i);
            }
        }
    }
    let mut keys: Vec<&String> = members.keys().collect();
    keys.sort();
    let mut set = db.prepare("UPDATE stat_comps SET core = ?, flex = ? WHERE patch = ? AND bucket = ? AND comp_key = ?")?;
    let mut add = db.prepare(
        "INSERT INTO stat_comp_variants (patch, bucket, comp_key, variant_id, units, carries, games, sum_place, wins, top4)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    for key in keys {
        let boards = &members[key];
        let mut acc = Acc::new(d);
        for i in boards {
            acc.add_board(b, *i);
        }
        let n = acc.n.max(1) as f64;
        let core = acc.core_key(&b.names);
        let mut flex: Vec<u16> = (0..d as u16)
            .filter(|u| {
                let s = acc.pres[*u as usize] as f64 / n;
                (FLEX_SHARE..CORE_SHARE).contains(&s)
            })
            .collect();
        flex.sort_by(|x, y| acc.pres[*y as usize].cmp(&acc.pres[*x as usize]).then(x.cmp(y)));
        let flex_names: Vec<&str> = flex.iter().map(|u| b.names[*u as usize].as_str()).collect();
        set.execute(params![json_list(&core), json_list(&flex_names), patch, scope, key])?;
        for v in crate::variants::split(b, boards, &flex) {
            add.execute(params![
                patch,
                scope,
                key,
                v.id,
                json_list(&v.units),
                json_list(&v.carries),
                v.games,
                v.sum_place,
                v.wins,
                v.top4
            ])?;
        }
    }
    Ok(())
}

/// The comp tables of one scope in an incremental build, from the tagged boards in inc_b: the boards are put
/// in p and go through the same steps as in a full rebuild, so the result is the same.
pub(crate) fn comps_from_inc(
    db: &Connection,
    patch: &str,
    scope: &str,
    band: i64,
    w: i64,
    comps: i64,
    boards: &crate::onepass::PatchBoards,
) -> Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT);
         CREATE TEMP TABLE IF NOT EXISTS topc (comp_key TEXT PRIMARY KEY) WITHOUT ROWID;
         DELETE FROM p; DELETE FROM topc;",
    )?;
    db.execute(
        "INSERT INTO p SELECT id, placement, level, comp_key FROM inc_b WHERE band >= ? AND w >= ? ORDER BY id",
        params![band, w],
    )?;
    super::build_comps(db, patch, scope, comps, Some(boards))
}
