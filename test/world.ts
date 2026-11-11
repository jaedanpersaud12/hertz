/**
 * The fake TFT world and fake Riot API shared by the end-to-end test and the parity check.
 * Deterministic: the same seed gives the same ladders, matches and placements every run.
 */
import { createServer } from 'node:http'

// ---- A small fake world ----------------------------------------------------
/** When the world was made. Every ranked match ends an hour before it; the golden files record times against it. */
export const WORLD_NOW = Date.now()
let seed = 42
const rand = () => ((seed = (seed * 1664525 + 1013904223) % 4294967296) / 4294967296)
const pick = <T,>(xs: T[]) => xs[Math.floor(rand() * xs.length)]

export const RARITY = [0, 1, 2, 4, 6] // Riot's rarity value for 1- to 5-cost units
export const UNITS = Array.from({ length: 20 }, (_, i) => ({ id: `TFT99_Unit${i}`, cost: 1 + Math.floor(i / 4), strength: (i - 10) * 0.12 }))
export const ITEMS = Array.from({ length: 10 }, (_, i) => ({ id: `TFT_Item_Thing${i}`, bonus: (i - 5) * 0.25 }))
export const ARCHETYPES = [
  { trait: 'TFT99_Vanguard', carry: 17, core: [17, 16, 12, 13, 8, 9, 4], level: 8, carryStar: 2 },
  { trait: 'TFT99_Invoker', carry: 14, core: [14, 15, 10, 11, 6, 7, 2], level: 8, carryStar: 2 },
  { trait: 'TFT99_Brawler', carry: 9, core: [9, 8, 5, 4, 1, 0, 3], level: 7, carryStar: 3 },
  { trait: 'TFT99_Sniper', carry: 19, core: [19, 18, 13, 12, 11, 10, 16], level: 9, carryStar: 2 },
  { trait: 'TFT99_Mystic', carry: 6, core: [6, 5, 4, 7, 0, 1, 2], level: 6, carryStar: 3 },
]
export const PLATFORMS = { na1: 'americas', euw1: 'europe' } as const

type Match = any
export const ladders: Record<string, Record<string, string[]>> = {}
export const matches = new Map<string, Match>()
export const idsByPlayer = new Map<string, string[]>()
export const archetypeOf = new Map<string, number>() // `${matchId}|${puuid}` -> archetype index

for (const platform of Object.keys(PLATFORMS)) {
  const mk = (tier: string, n: number) => Array.from({ length: n }, (_, i) => `${platform}-${tier}-${i}`)
  ladders[platform] = { challenger: mk('chal', 30), grandmaster: mk('gm', 30), master: mk('mas', 40), DIAMOND: mk('dia', 50) }
  const everyone = Object.values(ladders[platform]).flat()
  for (let m = 0; m < 220; m++) {
    const id = `${platform.toUpperCase()}_${1000 + m}`
    const lobby = [...everyone].sort(() => rand() - 0.5).slice(0, 8)
    const ranked = m % 10 !== 0 // every tenth match is a normal game and must be skipped
    const boards = lobby.map((puuid) => {
      const a = Math.floor(rand() * ARCHETYPES.length)
      const arch = ARCHETYPES[a]
      archetypeOf.set(`${id}|${puuid}`, a)
      const onBoard = new Set(arch.core)
      const units = arch.core.map((idx, slot) => {
        // Non-carry slots are sometimes swapped for another unit, like real boards.
        if (slot > 1 && rand() < 0.25) {
          const spare = UNITS.map((_, i) => i).filter((i) => !onBoard.has(i))
          onBoard.delete(idx); idx = pick(spare); onBoard.add(idx)
        }
        const n = slot === 0 ? 3 : slot === 1 ? (rand() < 0.6 ? 3 : 2) : slot === 2 ? 1 : 0
        return { unit: UNITS[idx], star: slot === 0 ? arch.carryStar : rand() < 0.7 ? 2 : 1, items: Array.from({ length: n }, () => pick(ITEMS)) }
      })
      const power = units.reduce((s, u) => s + u.unit.strength + u.items.reduce((x, it) => x + it.bonus, 0), 0) + a * 0.3 + rand() * 2
      return { puuid, arch, units, power }
    })
    const order = [...boards].sort((a, b) => b.power - a.power)
    matches.set(id, {
      metadata: { match_id: id, participants: lobby },
      info: {
        game_datetime: WORLD_NOW - 3600_000, game_length: 2100,
        game_version: 'Linux Version 99.3.123.4567 (Nov 01 2026/10:00:00) [PUBLIC] <Releases/99.3>',
        queue_id: ranked ? 1100 : 1090, tft_set_number: 99,
        participants: boards.map((b) => ({
          puuid: b.puuid, riotIdGameName: b.puuid, riotIdTagline: 'T', placement: order.indexOf(b) + 1,
          level: b.arch.level, last_round: 30, gold_left: 3, total_damage_to_players: 90,
          traits: [
            { name: b.arch.trait, num_units: 6, style: 4, tier_current: 3 },
            { name: 'TFT99_Shared', num_units: 2, style: 1, tier_current: rand() < 0.7 ? 1 : 0 },
            { name: 'TFT99_Loner', num_units: 1, style: 3, tier_current: 1 },
          ],
          units: b.units.map((u) => ({ character_id: u.unit.id, tier: u.star, rarity: RARITY[u.unit.cost - 1], itemNames: u.items.map((i) => i.id) })),
        })),
      },
    })
    for (const p of lobby) idsByPlayer.set(p, [...(idsByPlayer.get(p) ?? []), id])
  }
}

// Other game modes (E12): Double Up, Hyper Roll and a revival queue. Stored with their queue id, left out of every
// stat. Added after the matches above, so those stay exactly as they were. Boards are copied from a match above
// into a new lobby; every sixth one sends Riot's other spelling, queueId.
export const OTHER_QUEUES = [1160, 1130, 6000]
for (const platform of Object.keys(PLATFORMS)) {
  const everyone = Object.values(ladders[platform]).flat()
  for (let m = 0; m < 24; m++) {
    const id = `${platform.toUpperCase()}_${2000 + m}`
    const from = matches.get(`${platform.toUpperCase()}_${1000 + m * 7}`)
    const lobby = [...everyone].sort(() => rand() - 0.5).slice(0, 8)
    const info = structuredClone(from.info)
    info.participants.forEach((p: any, i: number) => {
      archetypeOf.set(`${id}|${lobby[i]}`, archetypeOf.get(`${from.metadata.match_id}|${p.puuid}`)!)
      p.puuid = lobby[i]; p.riotIdGameName = lobby[i]
    })
    delete info.queue_id
    if (m % 6 === 5) info.queueId = OTHER_QUEUES[m % 3]
    else info.queue_id = OTHER_QUEUES[m % 3]
    matches.set(id, { metadata: { match_id: id, participants: lobby }, info })
    for (const p of lobby) idsByPlayer.set(p, [...(idsByPlayer.get(p) ?? []), id])
  }
}
export const rankedMatches = [...matches.values()].filter((m) => m.info.queue_id === 1100)
/** The matches that are not ranked: normal games and the other modes above. Stored, but in no stat. */
export const otherModeMatches = [...matches.values()].filter((m) => m.info.queue_id !== 1100)

// ---- Fake Riot API with real rate limits -----------------------------------
export const LIMITS: [number, number][] = [[50, 1], [400, 10]]

/** Starts a fake Riot API on a free local port. Each server counts its own requests and limits. */
export async function startFakeRiot() {
  const hits = new Map<string, number[]>()
  const seen = { total: 0, overLimit: 0, forced429: 0, forced500: 0 }
  const server = createServer((req, res) => {
  const url = new URL(req.url!, 'http://x')
  const [, host, ...rest] = url.pathname.split('/')
  const path = '/' + rest.join('/')
  const send = (status: number, body: unknown, headers: Record<string, string> = {}) => {
    res.writeHead(status, { 'content-type': 'application/json', 'x-app-rate-limit': LIMITS.map((l) => l.join(':')).join(','), ...headers })
    res.end(JSON.stringify(body))
  }
  if (req.headers['x-riot-token'] !== 'TEST-KEY') return send(403, {})

  seen.total++
  const now = Date.now()
  const log = (hits.get(host) ?? []).filter((t) => t > now - 10_000)
  const over = LIMITS.some(([n, s]) => log.filter((t) => t > now - s * 1000).length >= n)
  if (over) { seen.overLimit++; return send(429, {}, { 'retry-after': '1' }) }
  log.push(now); hits.set(host, log)
  if (seen.total === 37) { seen.forced429++; return send(429, {}, { 'retry-after': '1' }) }
  if (seen.total % 97 === 0) { seen.forced500++; return send(500, {}) }

  let m: RegExpMatchArray | null
  if ((m = path.match(/^\/tft\/league\/v1\/(challenger|grandmaster|master)$/))) {
    return send(200, { entries: ladders[host][m[1]].map((puuid) => ({ puuid, leaguePoints: 100 })) })
  }
  if ((m = path.match(/^\/tft\/league\/v1\/entries\/(\w+)\/(\w+)$/))) {
    const first = m[2] === 'I' && url.searchParams.get('page') === '1'
    return send(200, first ? (ladders[host][m[1]] ?? []).map((puuid) => ({ puuid })) : [])
  }
  if (path === '/riot/account/v1/accounts/by-riot-id/Tester/NA1') return send(200, { puuid: 'na1-chal-0', gameName: 'Tester', tagLine: 'NA1' })
  if (path.startsWith('/tft/league/v1/by-puuid/')) {
    return send(200, [{ queueType: 'RANKED_TFT', tier: 'CHALLENGER', rank: 'I', leaguePoints: 512, wins: 40, losses: 60 }])
  }
  if ((m = path.match(/^\/tft\/match\/v1\/matches\/by-puuid\/([^/]+)\/ids$/))) {
    return send(200, (idsByPlayer.get(m[1]) ?? []).slice(0, Number(url.searchParams.get('count') ?? 20)))
  }
  if ((m = path.match(/^\/tft\/match\/v1\/matches\/([^/]+)$/))) return matches.has(m[1]) ? send(200, matches.get(m[1])) : send(404, {})
  send(404, {})
})
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r))
  const port = (server.address() as { port: number }).port
  return { port, seen, close: () => new Promise<void>((r) => server.close(() => r())) }
}

// ---- Fake Community Dragon -------------------------------------------------

/** The patch folders Community Dragon lists, as its /json/ listing gives them: two patches, plus "latest" and "pbe". */
export const CDRAGON_LISTING = [
  { name: '16.21', type: 'directory', mtime: 'Wed, 21 Oct 2026 07:15:23 GMT' }, { name: 'latest', type: 'directory', mtime: 'Wed, 04 Nov 2026 07:47:21 GMT' },
  { name: '16.22', type: 'directory', mtime: 'Wed, 04 Nov 2026 07:47:21 GMT' }, { name: 'pbe', type: 'directory', mtime: 'Thu, 27 Feb 2025 06:58:53 GMT' },
]

/** Set data for the fake world, shaped like Community Dragon's en_us.json: an old set and set 99 with every unit and trait. */
export function fakeSetData() {
  const icon = (n: string) => `ASSETS/Characters/${n}/HUD/${n}_Square.TFT_Set99.tex`
  const traitNames = [...ARCHETYPES.map((a) => a.trait), 'TFT99_Shared', 'TFT99_Loner']
  return {
    items: ITEMS.map((i, n) => ({ apiName: i.id, name: `Thing ${n}`, icon: `ASSETS/Maps/TFT/Icons/Items/Thing${n}.TFT_Set99.tex`, composition: n < 6 ? ['TFT_Item_BFSword', 'TFT_Item_ChainVest'] : [] })),
    sets: {
      '98': { name: 'Old', champions: [], traits: [] },
      '99': {
        name: 'Test Set',
        traits: traitNames.map((t) => ({ apiName: t, name: t.replace('TFT99_', ''), effects: [{ minUnits: 2, style: 1 }, { minUnits: 4, style: 2 }, { minUnits: 6, style: 4 }] })),
        champions: UNITS.map((unit, n) => ({
          apiName: unit.id, name: `Unit ${n}`, cost: unit.cost, role: 'Attack Carry', tileIcon: icon(`TFT99_Unit${n}`),
          traits: ARCHETYPES.filter((a) => a.core.includes(n)).map((a) => a.trait.replace('TFT99_', '')).concat(n % 2 ? ['Shared'] : []),
          ability: { name: 'Big Hit', desc: 'Deal <physicalDamage>@Damage@ (%i:scaleAD%)</physicalDamage> damage.<br>Stun for @StunDuration@ seconds.', variables: [{ name: 'Damage', value: [0, 100, 150, 900] }, { name: 'StunDuration', value: [1, 1, 1, 1] }] },
          stats: { hp: 800, mana: 60, initialMana: 0, damage: 55, armor: 40, magicResist: 40, attackSpeed: 0.75, range: 4 },
        })),
      },
    },
  }
}

/** Starts a fake Community Dragon on a free local port: the patch listing at /json/ and the set data at /latest/. */
export async function startFakeCdragon() {
  const seen: string[] = []
  const server = createServer((req, res) => {
    seen.push(req.url ?? '')
    const send = (body: unknown) => res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify(body))
    if (req.url === '/json/') return send(CDRAGON_LISTING)
    if (req.url === '/latest/cdragon/tft/en_us.json') return send(fakeSetData())
    res.writeHead(404).end()
  })
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r))
  const port = (server.address() as { port: number }).port
  return { port, base: `http://127.0.0.1:${port}`, seen, close: () => new Promise<void>((r) => server.close(() => r())) }
}
