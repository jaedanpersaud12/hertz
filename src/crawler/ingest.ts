import { compKey } from '../cluster'
import type { DB } from '../db'
import { patchFor } from '../patches'
import type { RiotMatch } from './riot'

export function makeIngest(db: DB) {
  const insMatch = db.prepare(
    'INSERT OR IGNORE INTO matches (match_id, platform, patch, set_number, game_datetime, game_length, rank, queue_id) VALUES (?,?,?,?,?,?,?,?)',
  )
  const insPart = db.prepare(
    'INSERT INTO participants (match_id, puuid, placement, level, last_round, gold_left, damage, comp_key) VALUES (?,?,?,?,?,?,?,?)',
  )
  const insUnit = db.prepare('INSERT INTO units (participant_id, character_id, star, n_items, build, rarity) VALUES (?,?,?,?,?,?)')
  const insUnitItem = db.prepare('INSERT INTO unit_items (participant_id, character_id, item) VALUES (?,?,?)')
  const insTrait = db.prepare('INSERT INTO traits (participant_id, name, num_units, tier_current, style) VALUES (?,?,?,?,?)')
  const insPlayer = db.prepare('INSERT OR IGNORE INTO players (puuid, platform, tier, rank, last_crawled) VALUES (?,?,?,?,0)')

  /** Stores one match. Returns false if it was already stored. */
  return db.transaction((match: RiotMatch, platform: string, rank: number, tier: string, snowball: boolean): boolean => {
    const { info } = match
    const added = insMatch.run(
      match.metadata.match_id, platform, patchFor(info.game_version, info.game_datetime), info.tft_set_number ?? null,
      info.game_datetime, info.game_length, rank, info.queue_id ?? info.queueId ?? null,
    )
    if (added.changes === 0) return false

    for (const p of info.participants) {
      const units = (p.units ?? []).map((u) => {
        const items = (u.itemNames ?? []).filter(Boolean)
        return { character_id: u.character_id, star: u.tier, rarity: u.rarity ?? null, items, n_items: items.length }
      })
      const traits = (p.traits ?? []).filter((t) => t.tier_current > 0)

      const pid = insPart.run(
        match.metadata.match_id, p.puuid, p.placement, p.level, p.last_round, p.gold_left, p.total_damage_to_players,
        compKey(units, traits),
      ).lastInsertRowid

      for (const u of units) {
        const build = u.n_items === 3 ? [...u.items].sort().join('|') : null
        insUnit.run(pid, u.character_id, u.star, u.n_items, build, u.rarity)
        for (const item of u.items) insUnitItem.run(pid, u.character_id, item)
      }
      for (const t of traits) insTrait.run(pid, t.name, t.num_units, t.tier_current, t.style)

      // Everyone in a ranked lobby is close in rank, so new players inherit the seed player's rank.
      if (snowball) insPlayer.run(p.puuid, platform, tier, rank)
    }
    return true
  })
}
