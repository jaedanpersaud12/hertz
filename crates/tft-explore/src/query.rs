//! Answering a question from an index: intersect the condition bitmaps, then walk the matching rows.

use crate::file::{Index, Kind, entries, key, unit_parts};
use roaring::RoaringBitmap;
use std::sync::Arc;

/// Today's Explorer question (`/api/explore`): boards with every one of these units and traits, in one scope.
/// The conditions of X1 to X4 (stars, items, negation, level, placement) are bitmaps of the same index.
#[derive(Clone, Debug)]
pub struct Query<'a> {
    pub units: &'a [String],
    /// A board has a trait when it has a row for it, at any tier (as the SQL asks).
    pub traits: &'a [String],
    /// matches.rank at or above this.
    pub rank_min: i64,
    /// matches.game_datetime at or after this (ms); 0 for the whole patch.
    pub since: i64,
    /// Matches with this queue id, or none recorded, count as ranked.
    pub ranked_queue: i64,
    /// How many partner units and items on the first unit to list.
    pub partner_limit: usize,
    pub item_limit: usize,
}

/// Games, summed placement, firsts and top fours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub games: i64,
    pub sum_place: i64,
    pub wins: i64,
    pub top4: i64,
}
impl Counts {
    fn add(&mut self, placement: i64) {
        self.games += 1;
        self.sum_place += placement;
        self.wins += i64::from(placement == 1);
        self.top4 += i64::from(placement <= 4);
    }
    fn merge(&mut self, o: &Counts) {
        self.games += o.games;
        self.sum_place += o.sum_place;
        self.wins += o.wins;
        self.top4 += o.top4;
    }
}

/// The matching boards' totals, the units on them and the items the first unit holds, each counted once per
/// board, most games first, cut to the limits. Ties go highest id first (byte order), as SQLite orders them for
/// `GROUP BY id ORDER BY games DESC LIMIT n`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub summary: Counts,
    pub partners: Vec<(String, Counts)>,
    pub items: Vec<(String, Counts)>,
}

/// One worker's sums over its share of the rows.
struct Part {
    summary: Counts,
    units: Vec<Counts>,
    items: Vec<Counts>,
}

/// Below this many rows one thread is faster than starting more.
const PARALLEL_FROM: usize = 50_000;

impl Index {
    /// Rows of ranked matches at or above `rank_min`: a union of rank bitmaps and queue bitmaps, kept per scope.
    pub fn scope(&self, rank_min: i64, ranked_queue: i64) -> Arc<RoaringBitmap> {
        if let Some(b) = self.scopes.lock().unwrap().get(&(rank_min, ranked_queue)) {
            return b.clone();
        }
        let mut ranks = RoaringBitmap::new();
        for (_, b) in self.keys(Kind::Rank).collect::<Vec<_>>() {
            if (b as i32) as i64 >= rank_min {
                ranks |= &*self.bitmap(key(Kind::Rank, 0, b));
            }
        }
        let mut ranked = RoaringBitmap::new();
        for (a, _) in self.keys(Kind::Queue).collect::<Vec<_>>() {
            if self.dir.queues.get(a as usize).is_some_and(|q| q.is_none_or(|q| q == ranked_queue)) {
                ranked |= &*self.bitmap(key(Kind::Queue, a, 0));
            }
        }
        let b = Arc::new(ranks & ranked);
        self.scopes.lock().unwrap().insert((rank_min, ranked_queue), b.clone());
        b
    }

    /// The rows that meet every condition, before the time window. None when a unit or trait is not in the index
    /// (no board has it).
    pub fn matching(&self, q: &Query) -> Option<RoaringBitmap> {
        let mut sets = vec![self.scope(q.rank_min, q.ranked_queue)];
        for u in q.units {
            sets.push(self.bitmap(key(Kind::Unit, self.unit_id(u)? as u32, 0)));
        }
        for t in q.traits {
            sets.push(self.bitmap(key(Kind::Trait, self.trait_id(t)? as u32, 0)));
        }
        sets.sort_by_key(|b| b.len());
        let mut acc = (*sets[0]).clone();
        for s in &sets[1..] {
            if acc.is_empty() {
                break;
            }
            acc &= &**s;
        }
        Some(acc)
    }

    /// Answers `q`. The rows are walked in parallel when there are many.
    pub fn explore(&self, q: &Query) -> Answer {
        let Some(found) = self.matching(q) else { return Answer::default() };
        let first = q.units.first().and_then(|u| self.unit_id(u));
        let ids: Vec<u32> = found.iter().collect();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
        let parts: Vec<Part> = if ids.len() < PARALLEL_FROM || threads == 1 {
            vec![self.walk(&ids, q.since, first)]
        } else {
            let chunk = ids.len().div_ceil(threads);
            std::thread::scope(|s| {
                let handles: Vec<_> = ids.chunks(chunk).map(|c| s.spawn(move || self.walk(c, q.since, first))).collect();
                handles.into_iter().map(|h| h.join().expect("an Explorer worker panicked")).collect()
            })
        };
        let mut it = parts.into_iter();
        let mut total = it.next().unwrap_or(Part { summary: Counts::default(), units: Vec::new(), items: Vec::new() });
        for p in it {
            total.summary.merge(&p.summary);
            for (a, b) in total.units.iter_mut().zip(&p.units) {
                a.merge(b);
            }
            for (a, b) in total.items.iter_mut().zip(&p.items) {
                a.merge(b);
            }
        }
        let top = |counts: &[Counts], names: &[String], limit: usize| {
            let mut v: Vec<(String, Counts)> = counts
                .iter()
                .enumerate()
                .filter(|(_, c)| c.games > 0)
                .filter_map(|(i, c)| names.get(i).map(|n| (n.clone(), *c)))
                .collect();
            v.sort_by(|a, b| b.1.games.cmp(&a.1.games).then_with(|| b.0.as_bytes().cmp(a.0.as_bytes())));
            v.truncate(limit);
            v
        };
        Answer {
            summary: total.summary,
            partners: top(&total.units, &self.dir.units, q.partner_limit),
            items: top(&total.items, &self.dir.items, q.item_limit),
        }
    }

    /// Sums over `ids`: every board once, every unit on it once, every item the first unit holds once.
    fn walk(&self, ids: &[u32], since: i64, first: Option<u16>) -> Part {
        let (rows, units, items) = (self.rows(), self.units(), self.items());
        let (nu, ni) = (self.dir.units.len(), self.dir.items.len());
        let mut p = Part { summary: Counts::default(), units: vec![Counts::default(); nu], items: vec![Counts::default(); ni] };
        // The last row each unit or item was counted on, so a second copy on the same board is not counted again.
        let mut seen_unit = vec![u32::MAX; nu];
        let mut seen_item = vec![u32::MAX; ni];
        for &r in ids {
            let Some(row) = rows.get(r as usize) else { continue };
            if row.time < since {
                continue;
            }
            let place = row.placement as i64;
            p.summary.add(place);
            for &e in entries(rows, r as usize, units, |x| x.unit_start) {
                let u = unit_parts(e).0 as usize;
                if let Some(seen) = seen_unit.get_mut(u)
                    && *seen != r
                {
                    *seen = r;
                    p.units[u].add(place);
                }
            }
            let Some(first) = first else { continue };
            for &e in entries(rows, r as usize, items, |x| x.item_start) {
                if (e >> 16) as u16 != first {
                    continue;
                }
                let i = (e & 0xffff) as usize;
                if let Some(seen) = seen_item.get_mut(i)
                    && *seen != r
                {
                    *seen = r;
                    p.items[i].add(place);
                }
            }
        }
        p
    }
}
