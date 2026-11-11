//! Stores one match in one transaction. Same rows, same order and same values as `src/crawler/ingest.ts`;
//! `npm run parity` checks that.

use anyhow::Result;
use rusqlite::{Connection, params};
use tft_core::cluster::{KeyTrait, KeyUnit, comp_key};
use tft_core::patches::Calendar;
use tft_core::riot::RiotMatch;

/// Who a match was found through. Everyone in a ranked lobby is close in rank, so the match (and, with
/// snowballing, every new player in it) inherits the seed player's rank.
#[derive(Debug, Clone)]
pub struct Ingest<'a> {
    pub platform: &'a str,
    pub rank: i64,
    pub tier: &'a str,
    pub snowball: bool,
}

impl Ingest<'_> {
    /// Stores the match. Returns false if it was already stored.
    pub fn store(&self, conn: &mut Connection, m: &RiotMatch, calendar: &Calendar) -> Result<bool> {
        let tx = conn.transaction()?;
        let info = &m.info;
        let added = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO matches (match_id, platform, patch, set_number, game_datetime, game_length, rank, queue_id) VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                m.metadata.match_id,
                self.platform,
                calendar.patch_for(info.game_version.as_deref(), info.game_datetime),
                info.tft_set_number,
                info.game_datetime,
                info.game_length,
                self.rank,
                info.queue(),
            ])?;
        if added == 0 {
            return Ok(false); // dropping the transaction rolls back nothing, since nothing was written
        }
        // When it was stored, for matches per hour (E10). Its own table, so `matches` rows stay what the TypeScript
        // engine stores.
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        tx.prepare_cached("INSERT OR REPLACE INTO match_stored (match_id, platform, at) VALUES (?,?,?)")?
            .execute(params![m.metadata.match_id, self.platform, now])?;

        {
            let mut ins_part = tx.prepare_cached(
                "INSERT INTO participants (match_id, puuid, placement, level, last_round, gold_left, damage, comp_key) VALUES (?,?,?,?,?,?,?,?)",
            )?;
            let mut ins_unit = tx.prepare_cached(
                "INSERT INTO units (participant_id, character_id, star, n_items, build, rarity) VALUES (?,?,?,?,?,?)",
            )?;
            let mut ins_item =
                tx.prepare_cached("INSERT INTO unit_items (participant_id, character_id, item) VALUES (?,?,?)")?;
            let mut ins_trait = tx.prepare_cached(
                "INSERT INTO traits (participant_id, name, num_units, tier_current, style) VALUES (?,?,?,?,?)",
            )?;
            let mut ins_player = tx.prepare_cached(
                "INSERT OR IGNORE INTO players (puuid, platform, tier, rank, last_crawled) VALUES (?,?,?,?,0)",
            )?;

            for p in &info.participants {
                let units: Vec<(&str, i64, Option<i64>, Vec<String>)> =
                    p.units.iter().map(|u| (u.character_id.as_str(), u.tier, u.rarity, u.items())).collect();
                let traits: Vec<_> = p.traits.iter().filter(|t| t.tier_current > 0).collect();

                let key_units: Vec<KeyUnit> = units
                    .iter()
                    .map(|(id, star, rarity, items)| KeyUnit {
                        character_id: id,
                        star: *star,
                        rarity: *rarity,
                        n_items: items.len() as i64,
                    })
                    .collect();
                let key_traits: Vec<KeyTrait> = traits
                    .iter()
                    .map(|t| KeyTrait {
                        name: &t.name,
                        num_units: t.num_units,
                        tier_current: t.tier_current,
                        style: t.style,
                    })
                    .collect();

                ins_part.execute(params![
                    m.metadata.match_id,
                    p.puuid,
                    p.placement,
                    p.level,
                    p.last_round,
                    p.gold_left,
                    p.total_damage_to_players,
                    comp_key(&key_units, &key_traits),
                ])?;
                let pid = tx.last_insert_rowid();

                for (id, star, rarity, items) in &units {
                    let build = (items.len() == 3).then(|| {
                        let mut sorted = items.clone();
                        sorted.sort();
                        sorted.join("|")
                    });
                    ins_unit.execute(params![pid, id, star, items.len() as i64, build, rarity])?;
                    for item in items {
                        ins_item.execute(params![pid, id, item])?;
                    }
                }
                for t in &traits {
                    ins_trait.execute(params![pid, t.name, t.num_units, t.tier_current, t.style])?;
                }
                if self.snowball {
                    ins_player.execute(params![p.puuid, self.platform, self.tier, self.rank])?;
                }
            }
        }
        tx.commit()?;
        Ok(true)
    }
}

/// Gives a patch to matches stored before the calendar was known. Returns how many were fixed.
pub fn relabel_unknown(conn: &mut Connection, calendar: &Calendar) -> Result<usize> {
    let Some(first) = calendar.0.first() else { return Ok(0) };
    let tx = conn.transaction()?;
    let mut changed = 0;
    {
        let mut fix = tx.prepare("UPDATE matches SET patch = ?1 WHERE patch = 'unknown' AND game_datetime >= ?2")?;
        for p in calendar.0.iter().rev() {
            changed += fix.execute(params![p.patch, p.since])?;
        }
        changed += tx.execute("UPDATE matches SET patch = ?1 WHERE patch = 'unknown'", [&first.patch])?;
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tft_core::patches::PatchStart;

    fn sample(id: &str) -> RiotMatch {
        serde_json::from_value(serde_json::json!({
            "metadata": { "match_id": id, "participants": ["a", "b"] },
            "info": {
                "game_datetime": 1_000, "game_length": 2100.5, "game_version": "TFT Unreal Version ?.?.?.?",
                "queue_id": 1100, "tft_set_number": 16,
                "participants": [
                    { "puuid": "a", "placement": 1, "level": 9, "last_round": 35, "gold_left": 4, "total_damage_to_players": 120,
                      "traits": [ { "name": "T_Big", "num_units": 6, "style": 4, "tier_current": 3 },
                                  { "name": "T_Off", "num_units": 1, "style": 0, "tier_current": 0 } ],
                      "units": [ { "character_id": "U1", "tier": 2, "rarity": 6, "itemNames": ["Z", "", "A", "M"] },
                                 { "character_id": "U2", "tier": 3, "rarity": 0, "itemNames": ["Q"] },
                                 { "character_id": "U3", "tier": 1 },
                                 { "character_id": "U4", "tier": 2, "rarity": 4, "itemNames": [] } ] },
                    { "puuid": "b", "placement": 2, "traits": [], "units": [ { "character_id": "U1", "tier": 1 } ] }
                ]
            }
        }))
        .unwrap()
    }

    #[test]
    fn stores_rows_and_ignores_repeats() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        let cal = Calendar(vec![PatchStart { patch: "16.21".into(), since: 0 }, PatchStart { patch: "16.22".into(), since: 500 }]);
        let ing = Ingest { platform: "na1", rank: 9, tier: "CHALLENGER", snowball: true };

        assert!(ing.store(&mut conn, &sample("NA1_1"), &cal).unwrap());
        assert!(!ing.store(&mut conn, &sample("NA1_1"), &cal).unwrap());

        let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        assert_eq!(one("SELECT patch FROM matches"), "16.22");
        assert_eq!(one("SELECT comp_key FROM participants WHERE puuid = 'a'"), "T_Big|U1");
        assert_eq!(one("SELECT COALESCE(comp_key, 'none') FROM participants WHERE puuid = 'b'"), "none");
        assert_eq!(one("SELECT build FROM units WHERE character_id = 'U1' AND n_items = 3"), "A|M|Z");
        assert_eq!(one("SELECT group_concat(item, ',') FROM unit_items WHERE character_id = 'U1'"), "Z,A,M");
        assert_eq!(one("SELECT COUNT(*) || '' FROM traits"), "1");
        assert_eq!(one("SELECT COUNT(*) || '' FROM players"), "2");
        assert_eq!(one("SELECT match_id || ' ' || platform FROM match_stored"), "NA1_1 na1", "one stored-at row, not two");
        let at: i64 = conn.query_row("SELECT at FROM match_stored", [], |r| r.get(0)).unwrap();
        assert!(at > 1_700_000_000_000, "stored at the current time in ms, got {at}");
    }

    /// E12: the queue is stored on every match, whichever way Riot spells it; no queue at all is NULL.
    #[test]
    fn stores_the_queue_id() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        let ing = Ingest { platform: "na1", rank: 9, tier: "CHALLENGER", snowball: false };
        let with = |id: &str, snake: Option<i64>, camel: Option<i64>| {
            let mut m = sample(id);
            (m.info.queue_id, m.info.queue_id_camel) = (snake, camel);
            m
        };
        for m in [with("R", Some(1100), None), with("D", Some(1160), None), with("H", None, Some(1130)), with("N", None, None)] {
            ing.store(&mut conn, &m, &Calendar::default()).unwrap();
        }
        let got: Vec<(String, Option<i64>)> = conn
            .prepare("SELECT match_id, queue_id FROM matches ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let want = [("R", Some(1100)), ("D", Some(1160)), ("H", Some(1130)), ("N", None)];
        assert_eq!(got, want.map(|(id, q)| (id.to_string(), q)));
    }

    #[test]
    fn relabels_unknown_patches() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        let ing = Ingest { platform: "na1", rank: 9, tier: "CHALLENGER", snowball: false };
        ing.store(&mut conn, &sample("M1"), &Calendar::default()).unwrap();
        let cal = Calendar(vec![PatchStart { patch: "16.21".into(), since: 0 }, PatchStart { patch: "16.22".into(), since: 500 }]);
        assert_eq!(relabel_unknown(&mut conn, &cal).unwrap(), 1);
        let patch: String = conn.query_row("SELECT patch FROM matches", [], |r| r.get(0)).unwrap();
        assert_eq!(patch, "16.22");
    }
}
