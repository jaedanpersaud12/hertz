/**
 * Your own account: rank, recent placements and match history.
 * This is the only part of the app that calls Riot while you browse.
 * The account comes from the app (saved in the database) or, failing that, RIOT_ID and MY_PLATFORM in .env.
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

/** What the user typed into the app. Takes priority over .env. */
type Account = { riotId: string; platform: string }
const ACCOUNT_KEY = 'account'

const QUEUES: Record<number, string> = { 1100: 'Ranked', 1090: 'Normal', 1130: 'Hyper Roll', 1160: 'Double Up' }
const STALE_MS = 5 * 60_000

export function registerMe(app: Hono, db: DB, describe: Describe) {
  let refreshing: Promise<void> | null = null
  loadCalendar(db)
  let lastError: string | null = null
  const account = (): Account => {
    const saved = kvGet<Account>(db, ACCOUNT_KEY)
    if (saved?.riotId) return saved
    return { riotId: config.riotId, platform: config.myPlatform || config.platforms[0] }
  }
  const configured = () => account().riotId.includes('#')

  async function refresh() {
    const { riotId, platform } = account()
    const [name, tag] = riotId.split('#')
    const route = PLATFORM_ROUTE[platform]
    if (!route) throw new Error(`"${platform}" is not a known server. Pick your server again.`)

    // Account lookups have no "sea" host; any of the three others returns the same account.
    const found = await getAccount(route === 'sea' ? 'asia' : route, name.trim(), tag.trim())
    if (!found) throw new Error(`No Riot account called ${riotId}. Check the spelling and the tag after #.`)

    const ranked = (await getRank(platform, found.puuid)).find((e) => e.queueType === 'RANKED_TFT')
    const have = db.prepare('SELECT 1 FROM my_matches WHERE match_id = ?')
    const save = db.prepare('INSERT OR REPLACE INTO my_matches (match_id, game_datetime, json) VALUES (?,?,?)')
    for (const id of await getMatchIds(route, found.puuid, null, 20)) {
      if (have.get(id)) continue
      const match = await getMatch(route, id)
      if (match) save.run(id, match.info.game_datetime, JSON.stringify(match))
    }
    kvSet(db, 'me', {
      riotId, puuid: found.puuid, name: found.gameName, tag: found.tagLine, platform,
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
    const { riotId, platform } = account()
    if (!configured()) return c.json({ configured: false, riotId, platform })
    let me = kvGet<Me>(db, 'me')
    if (me && me.riotId !== riotId) me = null
    if (!me) await startRefresh()
    else if (Date.now() - me.fetchedAt > STALE_MS) void startRefresh()
    me = kvGet<Me>(db, 'me')
    if (!me || me.riotId !== riotId) return c.json({ configured: true, riotId, platform, error: lastError ?? 'Could not load your account.' })

    const recent = myMatches(me, 10).map((m) => m.info.participants.find((p) => p.puuid === me!.puuid)!.placement)
    return c.json({
      configured: true, riotId, error: lastError, refreshing: !!refreshing,
      name: me.name, tag: me.tag, platform: me.platform, tier: me.tier, division: me.division, lp: me.lp,
      wins: me.wins, losses: me.losses, fetchedAt: me.fetchedAt,
      recent, avgPlace: recent.length ? recent.reduce((a, b) => a + b, 0) / recent.length : null,
    })
  })

  /** Set from the app: { riotId: "Name#TAG", platform: "na1" }. Checks the account with Riot straight away. */
  app.post('/api/me/account', async (c) => {
    const body = await c.req.json().catch(() => ({})) as Partial<Account>
    const riotId = String(body.riotId ?? '').trim().replace(/\s*#\s*/, '#')
    const platform = String(body.platform ?? '').trim().toLowerCase()
    if (!/^[^#]{3,16}#[^#]{2,5}$/.test(riotId)) return c.json({ ok: false, error: 'Write it as Name#TAG, the way the client shows it.' }, 400)
    if (!PLATFORM_ROUTE[platform]) return c.json({ ok: false, error: 'Pick the server you play on.' }, 400)
    kvSet(db, ACCOUNT_KEY, { riotId, platform } satisfies Account)
    if (kvGet<Me>(db, 'me')?.riotId !== riotId) db.prepare('DELETE FROM my_matches').run() // games of the previous account
    await startRefresh()
    return c.json({ ok: !lastError, error: lastError })
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
