/**
 * Your own account: rank, recent placements and match history.
 * This is the only part of the app that calls Riot while you browse. It needs RIOT_ID and MY_PLATFORM in .env.
 */
import type { Hono } from 'hono'
import { rarityCost } from '../cluster'
import { config, PLATFORM_ROUTE } from '../config'
import { loadCalendar, patchFor } from '../patches'
import { getAccount, getMatch, getMatchIds, getRank, type RiotMatch } from '../crawler/riot'
import { kvGet, kvSet, type DB } from '../db'
import type { Describe } from './describe'

type Me = {
  riotId: string; puuid: string; name: string; tag: string; platform: string
  tier: string | null; division: string | null; lp: number | null; wins: number | null; losses: number | null
  fetchedAt: number
}

const QUEUES: Record<number, string> = { 1100: 'Ranked', 1090: 'Normal', 1130: 'Hyper Roll', 1160: 'Double Up' }
const STALE_MS = 5 * 60_000

export function registerMe(app: Hono, db: DB, describe: Describe) {
  let refreshing: Promise<void> | null = null
  loadCalendar(db)
  let lastError: string | null = null
  const configured = () => config.riotId.includes('#')

  async function refresh() {
    const [name, tag] = config.riotId.split('#')
    const platform = config.myPlatform || config.platforms[0]
    const route = PLATFORM_ROUTE[platform]
    if (!route) throw new Error(`MY_PLATFORM "${platform}" is not a known server.`)

    // Account lookups have no "sea" host; any of the three others returns the same account.
    const account = await getAccount(route === 'sea' ? 'asia' : route, name.trim(), tag.trim())
    if (!account) throw new Error(`No Riot account found for ${config.riotId}. Check RIOT_ID in .env.`)

    const ranked = (await getRank(platform, account.puuid)).find((e) => e.queueType === 'RANKED_TFT')
    const have = db.prepare('SELECT 1 FROM my_matches WHERE match_id = ?')
    const save = db.prepare('INSERT OR REPLACE INTO my_matches (match_id, game_datetime, json) VALUES (?,?,?)')
    for (const id of await getMatchIds(route, account.puuid, null, 20)) {
      if (have.get(id)) continue
      const match = await getMatch(route, id)
      if (match) save.run(id, match.info.game_datetime, JSON.stringify(match))
    }
    kvSet(db, 'me', {
      riotId: config.riotId, puuid: account.puuid, name: account.gameName, tag: account.tagLine, platform,
      tier: ranked?.tier ?? null, division: ranked?.rank ?? null, lp: ranked?.leaguePoints ?? null,
      wins: ranked?.wins ?? null, losses: ranked?.losses ?? null, fetchedAt: Date.now(),
    } satisfies Me)
  }

  function startRefresh() {
    if (process.env.DEMO === '1') return Promise.resolve() // demo data: there is no real account to ask Riot about
    refreshing ??= refresh()
      .then(() => { lastError = null })
      .catch((e: Error) => { lastError = e.message })
      .finally(() => { refreshing = null })
    return refreshing
  }

  const myMatches = (me: Me, limit: number) =>
    (db.prepare('SELECT json FROM my_matches ORDER BY game_datetime DESC LIMIT ?').all(limit) as { json: string }[])
      .map((r) => JSON.parse(r.json) as RiotMatch)
      .filter((m) => m.info.participants.some((p) => p.puuid === me.puuid))

  app.get('/api/me', async (c) => {
    if (!configured()) return c.json({ configured: false })
    let me = kvGet<Me>(db, 'me')
    if (me && me.riotId !== config.riotId) me = null
    if (!me) await startRefresh()
    else if (Date.now() - me.fetchedAt > STALE_MS) void startRefresh()
    me = kvGet<Me>(db, 'me')
    if (!me || me.riotId !== config.riotId) return c.json({ configured: true, error: lastError ?? 'Could not load your account.' })

    const recent = myMatches(me, 10).map((m) => m.info.participants.find((p) => p.puuid === me!.puuid)!.placement)
    return c.json({
      configured: true, error: lastError, refreshing: !!refreshing,
      name: me.name, tag: me.tag, platform: me.platform, tier: me.tier, division: me.division, lp: me.lp,
      wins: me.wins, losses: me.losses, fetchedAt: me.fetchedAt,
      recent, avgPlace: recent.length ? recent.reduce((a, b) => a + b, 0) / recent.length : null,
    })
  })

  app.post('/api/me/refresh', async (c) => {
    if (!configured()) return c.json({ configured: false })
    await startRefresh()
    return c.json({ ok: !lastError, error: lastError })
  })

  app.get('/api/me/matches', (c) => {
    const me = kvGet<Me>(db, 'me')
    if (!configured() || !me) return c.json({ configured: configured(), matches: [] })
    const matches = myMatches(me, 40).map((m) => {
      const lobby = m.info.participants.map((p) => ({
        me: p.puuid === me.puuid,
        name: p.riotIdGameName ? `${p.riotIdGameName}#${p.riotIdTagline ?? ''}` : null,
        placement: p.placement, level: p.level, damage: p.total_damage_to_players,
        units: (p.units ?? []).map((u) => {
          const d = describe('unit', u.character_id)
          return { ...d, cost: d.cost ?? rarityCost(u.rarity), star: u.tier, items: (u.itemNames ?? []).map((i) => describe('item', i)) }
        }),
        traits: (p.traits ?? []).filter((t) => t.tier_current > 0).sort((a, b) => b.num_units - a.num_units)
          .map((t) => ({ ...describe('trait', t.name), units: t.num_units, style: t.style })),
      })).sort((a, b) => a.placement - b.placement)
      const queueId = m.info.queue_id ?? m.info.queueId ?? 0
      return {
        id: m.metadata.match_id, date: m.info.game_datetime, length: m.info.game_length,
        queue: QUEUES[queueId] ?? 'Other mode', patch: patchFor(m.info.game_version, m.info.game_datetime),
        mine: lobby.find((p) => p.me)!, lobby,
      }
    })
    return c.json({ configured: true, matches })
  })
}
