//! Variants of a comp (PRD 2 C2, step 5 of "How clustering works"). A comp's boards are split by the flex units
//! they run in the 4- and 5-cost slots and by which units hold three items. Two boards with the same split are
//! the same variant, so any two variants differ in at least one 4- or 5-cost unit or in a three-item carry, as
//! the PRD asks. A variant needs [`VARIANT_MIN_GAMES`] games; a comp lists at most [`VARIANT_LIMIT`], most
//! games first.

use crate::cluster::{Boards, fnv};
use std::collections::HashMap;

pub const VARIANT_MIN_GAMES: i64 = 50;
pub const VARIANT_LIMIT: usize = 4;

pub struct Variant {
    pub id: String,
    /// The 4- and 5-cost flex units the variant runs, by unit id.
    pub units: Vec<String>,
    /// The units holding three items, by unit id.
    pub carries: Vec<String>,
    pub games: i64,
    pub sum_place: i64,
    pub wins: i64,
    pub top4: i64,
}

/// (4- and 5-cost flex units, three-item units) of a board.
type Key = (Vec<u16>, Vec<u16>);
/// games, sum of places, wins, top 4s.
type Sums = (i64, i64, i64, i64);

/// The variants of the comp made of `members` (board indexes), given its flex units.
pub fn split(b: &Boards, members: &[usize], flex: &[u16]) -> Vec<Variant> {
    let mut by_key: HashMap<Key, Sums> = HashMap::new();
    for &i in members {
        let mut units: Vec<u16> = Vec::new();
        let mut carries: Vec<u16> = Vec::new();
        for k in b.units(i) {
            if b.cost[k] >= 4 && flex.contains(&b.unit[k]) {
                units.push(b.unit[k]);
            }
            if b.items[k] >= 3 {
                carries.push(b.unit[k]);
            }
        }
        units.sort_unstable();
        carries.sort_unstable();
        let place = b.place[i] as i64;
        let e = by_key.entry((units, carries)).or_insert((0, 0, 0, 0));
        e.0 += 1;
        e.1 += place;
        e.2 += i64::from(place == 1);
        e.3 += i64::from(place <= 4);
    }
    let name = |xs: &[u16]| -> Vec<String> { xs.iter().map(|u| b.names[*u as usize].clone()).collect() };
    let mut out: Vec<Variant> = by_key
        .into_iter()
        .filter(|(_, s)| s.0 >= VARIANT_MIN_GAMES)
        .map(|((u, c), s)| {
            let (units, carries) = (name(&u), name(&c));
            let id = format!("v{:08x}", fnv(&format!("{}/{}", units.join(","), carries.join(","))) >> 32);
            Variant { id, units, carries, games: s.0, sum_place: s.1, wins: s.2, top4: s.3 }
        })
        .collect();
    out.sort_by(|a, b| b.games.cmp(&a.games).then(a.id.cmp(&b.id)));
    out.truncate(VARIANT_LIMIT);
    out
}
