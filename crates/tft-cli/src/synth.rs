//! `tft synth`: a synthetic database at real-data scale, for timing the stats build (no Riot API, no real players).
//!
//! The shape follows a real set: about 60 champions with two or three traits each, 28 traits, 9 components,
//! 45 craftable items, radiants, artifacts, support items and emblems; boards of 7 to 10 units built around
//! one of 40 archetypes (a few are popular), carries holding 3 items picked from a per-unit favourite list with a
//! long tail. Everything is deterministic for a given seed.

use anyhow::Result;
use rusqlite::{Connection, params};
use tft_core::patches::{Calendar, PatchStart};
use tft_core::riot::{Info, Metadata, Participant, RiotMatch, Trait, Unit};
use tft_store::Ingest;

const UNITS: usize = 60;
const TRAITS: usize = 28;
const COMPONENTS: usize = 9;
const ARCHETYPES: usize = 40;
const DAY: i64 = 86_400_000;

/// A small deterministic random source (xorshift).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// Skewed towards 0: low numbers are more common.
    fn skewed(&mut self, n: usize) -> usize {
        (self.below(n) * self.below(n)) / n
    }
}

struct World {
    items: Vec<String>,       // every holdable item
    craftable: Vec<String>,   // the 45 craftables, in recipe order
    unit_traits: Vec<Vec<usize>>,
    /// Favourite items per unit, best first.
    favourites: Vec<Vec<usize>>,
    /// Per archetype: the units on its usual board, carry first.
    archetypes: Vec<Vec<usize>>,
}

fn unit_id(u: usize) -> String {
    format!("TFT77_Champ{u:02}")
}
fn trait_id(t: usize) -> String {
    format!("TFT77_Trait{t:02}")
}

fn world() -> World {
    let mut rng = Rng(0x0005_eed0_f7f7);
    let craftable: Vec<String> = (0..45).map(|i| format!("TFT_Item_Craft{i:02}")).collect();
    let radiant: Vec<String> = (0..45).map(|i| format!("TFT5_Item_Craft{i:02}Radiant")).collect();
    let artifact: Vec<String> = (0..25).map(|i| format!("TFT_Item_Artifact_{i:02}")).collect();
    let support: Vec<String> = (0..10).map(|i| format!("TFT_Item_Support_{i:02}")).collect();
    let emblem: Vec<String> = (0..20).map(|i| format!("TFT77_Item_Trait{i:02}EmblemItem")).collect();
    let items: Vec<String> = [&craftable[..], &radiant[..], &artifact[..], &support[..], &emblem[..]].concat();
    let unit_traits: Vec<Vec<usize>> =
        (0..UNITS).map(|u| (0..2 + u % 2).map(|j| (u * 5 + j * 11) % TRAITS).collect()).collect();
    let favourites: Vec<Vec<usize>> = (0..UNITS)
        .map(|_| {
            let mut fav: Vec<usize> = (0..14).map(|_| rng.below(45)).collect(); // mostly craftables
            fav.extend((0..4).map(|_| 45 + rng.below(items.len() - 45))); // a few radiants and the rest
            fav
        })
        .collect();
    let archetypes: Vec<Vec<usize>> = (0..ARCHETYPES)
        .map(|a| {
            let mut units: Vec<usize> = Vec::new();
            let mut u = (a * 13) % UNITS;
            while units.len() < 10 {
                if !units.contains(&u) {
                    units.push(u);
                }
                u = (u + 1 + rng.below(9)) % UNITS;
            }
            units
        })
        .collect();
    World { items, craftable, unit_traits, favourites, archetypes }
}

fn lobby(w: &World, rng: &mut Rng, id: &str, at: i64) -> RiotMatch {
    let mut places: Vec<i64> = (1..=8).collect();
    for i in (1..8).rev() {
        places.swap(i, rng.below(i + 1));
    }
    let participants = places
        .iter()
        .enumerate()
        .map(|(seat, &placement)| {
            let arch = &w.archetypes[rng.skewed(ARCHETYPES)];
            let level = 7 + rng.below(4);
            let size = (level + rng.below(2)).min(10);
            let mut board: Vec<usize> = arch[..size.min(arch.len())].to_vec();
            // One or two units off-archetype.
            for _ in 0..rng.below(3) {
                let i = 2 + rng.below(board.len() - 2);
                let u = rng.below(UNITS);
                if !board.contains(&u) {
                    board[i] = u;
                }
            }
            let units: Vec<Unit> = board
                .iter()
                .enumerate()
                .map(|(j, &u)| {
                    let n_items = match j {
                        0 | 1 => 3,
                        2 => 2 + rng.below(2),
                        3 | 4 => rng.below(2),
                        _ => usize::from(rng.below(6) == 0),
                    };
                    let fav = &w.favourites[u];
                    let items = (0..n_items)
                        .map(|_| {
                            let i = if rng.below(10) < 8 { fav[rng.skewed(fav.len())] } else { rng.below(w.items.len()) };
                            w.items[i].clone()
                        })
                        .collect();
                    Unit {
                        character_id: unit_id(u),
                        tier: if j < 2 && rng.below(5) == 0 { 3 } else { 1 + rng.below(2) as i64 + i64::from(j < 5) }.min(3),
                        rarity: Some((u % 5) as i64),
                        item_names: Some(items),
                    }
                })
                .collect();
            let mut counts = vec![0i64; TRAITS];
            for &u in &board {
                for &t in &w.unit_traits[u] {
                    counts[t] += 1;
                }
            }
            let traits = counts
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(t, &n)| Trait {
                    name: trait_id(t),
                    num_units: n,
                    style: 1 + (n / 2).min(4),
                    tier_current: (n / 2).min(4), // 0 is inactive, dropped at ingest
                })
                .collect();
            Participant {
                puuid: format!("{id}-{seat}"),
                riot_id_game_name: None,
                riot_id_tagline: None,
                placement,
                level: Some(level as i64),
                last_round: Some(20 + rng.below(20) as i64),
                gold_left: Some(rng.below(30) as i64),
                total_damage_to_players: Some(rng.below(150) as i64),
                traits,
                units,
            }
        })
        .collect();
    RiotMatch {
        metadata: Metadata { match_id: id.to_string(), participants: Vec::new() },
        info: Info {
            game_datetime: at,
            game_length: Some(2000.0),
            game_version: None,
            queue_id: Some(1100),
            queue_id_camel: None,
            tft_set_number: Some(77),
            participants,
        },
    }
}

/// Set data for the synthetic world: units with their traits, traits, items with type, recipe and emblem trait.
fn static_data(conn: &Connection, w: &World) -> Result<()> {
    let mut add = conn.prepare("INSERT OR REPLACE INTO static_data (kind, id, name, icon, data) VALUES (?, ?, ?, NULL, ?)")?;
    for t in 0..TRAITS {
        add.execute(params!["trait", trait_id(t), format!("Trait {t:02}"), "{}"])?;
    }
    for (u, traits) in w.unit_traits.iter().enumerate() {
        let names: Vec<String> = traits.iter().map(|t| format!("Trait {t:02}")).collect();
        let data = serde_json::json!({ "cost": 1 + u % 5, "traits": names });
        add.execute(params!["unit", unit_id(u), format!("Champ {u:02}"), data.to_string()])?;
    }
    for (i, item) in w.items.iter().enumerate() {
        let data = if i < 45 {
            let (a, b) = (i % COMPONENTS, (i / COMPONENTS + i) % COMPONENTS);
            serde_json::json!({ "type": "craftable", "composition": [format!("TFT_Item_Comp{a}"), format!("TFT_Item_Comp{b}")] })
        } else if i < 90 {
            serde_json::json!({ "type": "radiant" })
        } else if i < 115 {
            serde_json::json!({ "type": "artifact" })
        } else if i < 125 {
            serde_json::json!({ "type": "support" })
        } else {
            serde_json::json!({ "type": "emblem", "trait": trait_id(i - 125) })
        };
        add.execute(params!["item", item, item, data.to_string()])?;
    }
    for c in 0..COMPONENTS {
        add.execute(params!["item", format!("TFT_Item_Comp{c}"), format!("Component {c}"), r#"{"type":"component"}"#])?;
    }
    let _ = &w.craftable;
    Ok(())
}

/// Stores `matches` synthetic matches in one patch (`77.1`), spread over the last 7 days and ranks Iron to
/// Challenger, then writes the set data. Returns the number stored.
pub fn synth(conn: &mut Connection, matches: usize, seed: u64) -> Result<usize> {
    conn.execute_batch("PRAGMA synchronous = OFF")?;
    let w = world();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let cal = Calendar(vec![PatchStart { patch: "77.1".into(), since: now - 30 * DAY }]);
    let ranks = [2i64, 3, 4, 4, 5, 5, 6, 6, 7, 8, 9];
    let mut rng = Rng(seed.max(1));
    let mut stored = 0;
    for i in 0..matches {
        let rank = ranks[rng.below(ranks.len())];
        let ing = Ingest { platform: "na1", rank, tier: tft_core::config::tier_name(rank), snowball: false };
        let at = now - 7 * DAY + (i as i64 * 7 * DAY) / matches.max(1) as i64;
        if ing.store(conn, &lobby(&w, &mut rng, &format!("SYN_{seed}_{i}"), at), &cal)? {
            stored += 1;
        }
    }
    static_data(conn, &w)?;
    conn.execute_batch("PRAGMA synchronous = NORMAL")?;
    Ok(stored)
}
