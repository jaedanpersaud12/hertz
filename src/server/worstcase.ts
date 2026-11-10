/**
 * Dev-only stress data for the interface. Never active under "npm start".
 *
 * With "npm run dev" (or "npm run demo"), adding ?data=worst | empty | one | many to an API call
 * reshapes the real response into a worst realistic case, so the real components render it.
 * The switch at the bottom of the app (dev only) sets that parameter for you.
 */
export type StressMode = 'worst' | 'empty' | 'one' | 'many'
export const STRESS_MODES: StressMode[] = ['worst', 'empty', 'one', 'many']

// Real names, chosen for what they do to a layout: long, two letters, apostrophes, an ampersand, other scripts.
const UNITS = ['Aurelion Sol', 'Nunu & Willump', 'Vi', "Kog'Maw", 'Renata Glasc', '아우렐리온 솔', 'Twisted Fate', 'Jax', 'Тахм Кенч', 'Miss Fortune', 'オレリオン・ソル', "Bel'Veth", 'ออเรเลียน โซล', 'Dr. Mundo']
const ITEMS = ["Rabadon's Deathcap", "Guinsoo's Rageblade", "Tactician's Crown", 'Unstable Treasure Chest', 'Battle Academia Emblem', 'B.F. Sword', "Zz'Rot Portal", 'Rabadons Todeshaube', '라바돈의 죽음모자', 'Needlessly Large Rod', "Protector's Vow"]
const TRAITS = ['Battle Academia', 'Star Guardian', 'Soul Fighter', 'Mighty Mech', 'The Crew', 'Supreme Cells', 'Wraith', 'Stance Master', 'Crystal Gambit', 'Monster Trainer', '별 수호자']
// Riot IDs allow 16 characters plus a 5-character tag. W is the widest letter.
const PLAYERS = ['WWWWWWWWWWWWWWWW#WWWWW', '숨겨진 드래곤 마스터#KR1', 'Jo#1', 'محمد العبدالله#EUNE', null, 'Aleksandra Wiśnie#EUW', 'ngọc ánh nguyễn#VN2', 'x#NA1']

const hash = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7)
type Obj = Record<string, any>
const isObj = (v: unknown): v is Obj => !!v && typeof v === 'object' && !Array.isArray(v)

/** Visit every object in a JSON tree. */
function walk(v: unknown, fn: (o: Obj) => void) {
  if (Array.isArray(v)) v.forEach((x) => walk(x, fn))
  else if (isObj(v)) { fn(v); Object.values(v).forEach((x) => walk(x, fn)) }
}

function rename(o: Obj) {
  if (typeof o.id !== 'string' || typeof o.name !== 'string') return
  const h = hash(o.id)
  if ('cost' in o) {
    o.name = UNITS[h % UNITS.length]
    if (h % 5 === 0) { o.cost = null; o.icon = null } // not in the downloaded set data yet
    if (Array.isArray(o.traits) && h % 3 === 0) o.traits = ['Battle Academia', 'Star Guardian', 'Monster Trainer', 'Crystal Gambit']
  } else if ('type' in o) o.name = ITEMS[h % ITEMS.length]
  else if ('breakpoints' in o || 'style' in o) o.name = TRAITS[h % TRAITS.length]
}

// Rows that share a table get different extremes, the way real data mixes them.
const EXTREMES: Obj[] = [
  { games: 12_345_678, avgPlace: 1, winRate: 1, top4Rate: 1, frequency: 1, pickRate: 8, placeChange: -3.21 },
  { games: 1, avgPlace: 8, winRate: 0, top4Rate: 0, frequency: 0.00004, pickRate: 0.0003, placeChange: 3.4 },
  { placeChange: null, avgStar: 3 },
]
const extreme = (rows: unknown) => {
  if (!Array.isArray(rows)) return
  rows.forEach((r, i) => {
    const e = EXTREMES[i]
    if (!e || !isObj(r)) return
    for (const k of Object.keys(e)) if (k in r) r[k] = e[k]
  })
}

function worst(path: string, body: Obj) {
  walk(body, rename)
  for (const key of ['rows', 'items', 'builds', 'holders', 'partners', 'comps', 'trend']) extreme(body[key])

  walk(body, (o) => {
    // A comp: its name is built from its trait and carries, and one comp gets the fullest possible board.
    if (o.play && Array.isArray(o.units) && Array.isArray(o.carries)) {
      o.name = [o.trait?.name ?? 'Flex', ...o.carries.map((u: Obj) => u.name)].join(' ')
      if (hash(o.id) % 2 === 0 && o.units.length) {
        const full = (u: Obj) => ({ ...u, star: 3, items: [0, 1, 2].map((n) => ({ id: `i${n}`, name: ITEMS[n], icon: null })) })
        o.units = Array.from({ length: 10 }, (_, n) => full({ ...o.units[n % o.units.length], id: `${o.units[n % o.units.length].id}~${n}` }))
        if (Array.isArray(o.traits) && o.traits.length) o.traits = Array.from({ length: 9 }, (_, n) => ({ ...o.traits[n % o.traits.length], id: `t${n}`, name: TRAITS[n], units: n === 0 ? 10 : 2 + (n % 4), style: 1 + (n % 5) }))
        if (Array.isArray(o.flex)) o.flex = Array.from({ length: 8 }, (_, n) => ({ ...o.units[n], id: `f${n}`, freq: 0.15 + n / 30, items: [] }))
        o.play = { kind: 'reroll', label: 'Reroll lvl 10' }
      }
    }
    // A player in a lobby.
    if ('placement' in o && 'damage' in o) {
      o.name = PLAYERS[hash(String(o.damage) + o.placement + (o.name ?? '')) % PLAYERS.length]
      o.level = 10; o.damage = 248
    }
    if ('queue' in o && 'length' in o) { if (hash(o.id) % 3 === 0) { o.queue = 'Queue 6120'; o.length = 0 } }
  })

  if (path === '/api/meta') {
    Object.assign(body.totals, { matches: 12_345_678, comps: 98_765_432, players: 1_284_003, queued: 1, newest: Date.now() - 400 * 86400_000 })
    body.comps = 98_765_432; body.matches = 12_345_678
    body.byPatch = [{ patch: '16.22', matches: 12_345_678 }, { patch: '16.21', matches: 1 }, { patch: 'unknown', matches: 0 }]
  }
  if (path === '/api/me' && body.name) {
    Object.assign(body, { name: 'WWWWWWWWWWWWWWWW', tag: 'WWWWW', tier: 'GRANDMASTER', lp: 2417, recent: [8, 8, 8, 8, 8, 8, 8, 8, 8, 8], avgPlace: 8,
      error: 'Riot did not answer on americas after several tries. Check your connection, or try again in a few minutes.' })
  }
  if (path === '/api/lists' && Array.isArray(body.teams)) {
    body.teams.forEach((t: Obj, i: number) => { t.name = i ? 'x' : 'Battle Academia reroll into Aurelion Sol if Nunu hits early!!'; t.created = Date.now() - 400 * 86400_000 })
  }
  return body
}

const LISTS = ['rows', 'matches', 'teams', 'favs', 'hidden', 'builds', 'levels', 'partners', 'items', 'holders', 'comps', 'trend', 'stars', 'itemCounts', 'units', 'traits', 'recent', 'byPatch']

function shrink(path: string, body: Obj, keep: 0 | 1) {
  for (const key of LISTS) if (Array.isArray(body[key])) body[key] = body[key].slice(0, keep)
  if (path === '/api/meta') {
    for (const k of Object.keys(body.totals)) body.totals[k] = keep ? 1 : 0
    body.totals.newest = keep ? Date.now() - 86400_000 : null
    Object.assign(body, { comps: keep, matches: keep })
    if (!keep) Object.assign(body, { patches: [], builtAt: null })
  }
  if (path === '/api/static') body.loaded = !!keep
  if (path === '/api/me' && body.name) Object.assign(body, keep ? { avgPlace: body.recent[0] ?? null } : { tier: null, avgPlace: null })
  if (path.startsWith('/api/explore') && body.summary) body.summary = keep ? { ...body.summary, games: 1, frequency: 0.0000003 } : { games: 0 }
  return body
}

function many(body: Obj) {
  if (Array.isArray(body.rows) && body.rows.length) {
    const base = body.rows
    body.rows = Array.from({ length: 1000 }, (_, n) => ({ ...base[n % base.length], id: `${base[n % base.length].id}~${n}`, games: 1000 - n }))
  }
  return body
}

export function stress(mode: string | undefined, path: string, body: unknown): unknown {
  if (!isObj(body) || !STRESS_MODES.includes(mode as StressMode)) return body
  if (mode === 'worst') return worst(path, body)
  if (mode === 'many') return many(body)
  return shrink(path, body, mode === 'one' ? 1 : 0)
}
