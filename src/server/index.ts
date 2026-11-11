/** Local API for the app. Everything except the summoner card reads from the local database only. */
import { serve } from '@hono/node-server'
import { serveStatic } from '@hono/node-server/serve-static'
import { Hono, type Context } from 'hono'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { BUCKETS, config, scopeKey, WINDOWS } from '../config'
import { kvGet, kvSet, openDb, rankedOnly } from '../db'
import { makeDescribe, type Entity } from './describe'
import { registerMe } from './me'
import { stress } from './worstcase'

const db = openDb() // creates the file and tables on first run, so the app opens before any crawl
const describe = makeDescribe(db)
const app = new Hono()
const dev = process.argv.includes('--dev')

// Dev only: ?data=worst (and friends) reshapes a response into stress data. See worstcase.ts.
if (dev) {
  app.use('/api/*', async (c, next) => {
    await next()
    const mode = c.req.query('data')
    if (!mode || c.req.method !== 'GET' || !c.res.headers.get('content-type')?.includes('json')) return
    const body = stress(mode, c.req.path, structuredClone(await c.res.json()))
    c.res = undefined
    c.res = c.json(body as object)
  })
}
const iconDir = join(dirname(config.dbPath), 'icons')
mkdirSync(iconDir, { recursive: true })

type Rates = { games: number; sum_place: number; wins: number; top4: number }
const rate = (r: Rates, total: number) => ({
  games: r.games,
  avgPlace: r.sum_place / r.games,
  winRate: r.wins / r.games,
  top4Rate: r.top4 / r.games,
  frequency: total ? r.games / total : 0,
})
const patchOrder = (a: string, b: string) => {
  const [a1, a2] = a.split('.').map(Number)
  const [b1, b2] = b.split('.').map(Number)
  return b1 - a1 || b2 - a2
}

/** Resolve ?patch=&bucket=&window=&min= against what has been built. */
function scope(c: Context) {
  const patches = (db.prepare('SELECT DISTINCT patch FROM stat_meta').all() as { patch: string }[]).map((r) => r.patch).sort(patchOrder)
  const patch = c.req.query('patch') || patches[0] || ''
  const bucket = BUCKETS.find((b) => b.key === c.req.query('bucket')) ?? BUCKETS[0]
  const window = WINDOWS.find((w) => w.key === c.req.query('window')) ?? WINDOWS[0]
  const key = scopeKey(bucket.key, window.key)
  const meta = db.prepare('SELECT matches, comps, built_at FROM stat_meta WHERE patch = ? AND bucket = ?').get(patch, key) as
    | { matches: number; comps: number; built_at: number } | undefined
  return {
    patch, bucket: bucket.key, window: window.key, key, rankMin: bucket.min,
    since: window.days ? Date.now() - window.days * 86400_000 : 0,
    min: Math.max(1, Number(c.req.query('min') ?? 10)),
    comps: meta?.comps ?? 0, matches: meta?.matches ?? 0, builtAt: meta?.built_at ?? null,
    patches, prevPatch: patches[patches.indexOf(patch) + 1] ?? null,
  }
}
type Scope = ReturnType<typeof scope>
const pub = (s: Scope) => ({
  patch: s.patch, bucket: s.bucket, window: s.window, min: s.min, comps: s.comps, matches: s.matches,
  builtAt: s.builtAt, patches: s.patches, prevPatch: s.prevPatch,
})

// ---- Comps -------------------------------------------------------------------

type CompUnit = Entity & { star: number; freq: number; avgItems: number; items: (Entity & { freq: number })[] }
type Comp = ReturnType<typeof rate> & {
  id: string; name: string; trait: Entity | null; carries: Entity[]; units: CompUnit[]; flex: (Entity & { freq: number })[]
  traits: (Entity & { units: number; style: number })[]; avgLevel: number; pickRate: number; play: { kind: string; label: string }
}
const compCache = new Map<string, Comp[]>()

/** Builds every comp for a scope from the stat tables. Cached until the stats are rebuilt. */
function loadComps(s: Scope): Comp[] {
  const cacheKey = `${s.patch}|${s.key}|${s.builtAt}`
  const cached = compCache.get(cacheKey)
  if (cached) return cached

  const k = [s.patch, s.key]
  const group = <T extends { comp_key: string }>(rows: T[]) => {
    const m = new Map<string, T[]>()
    for (const r of rows) m.set(r.comp_key, [...(m.get(r.comp_key) ?? []), r])
    return m
  }
  const unitRows = group(db.prepare('SELECT * FROM stat_comp_units WHERE patch = ? AND bucket = ? ORDER BY games DESC').all(...k) as any[])
  const itemRows = group(db.prepare('SELECT * FROM stat_comp_items WHERE patch = ? AND bucket = ? ORDER BY games DESC').all(...k) as any[])
  const traitRows = group(db.prepare('SELECT * FROM stat_comp_traits WHERE patch = ? AND bucket = ? ORDER BY games DESC').all(...k) as any[])

  const comps = (db.prepare('SELECT * FROM stat_comps WHERE patch = ? AND bucket = ?').all(...k) as any[]).map((c): Comp => {
    const [traitId, carryId] = (c.comp_key as string).split('|')
    const us = unitRows.get(c.comp_key) ?? []
    const its = itemRows.get(c.comp_key) ?? []

    // Core units: on the board in at least half the games. Small or messy comps fall back to the seven most common.
    let core = us.filter((u) => u.games / c.games >= 0.5).slice(0, 10)
    if (core.length < 4) core = us.slice(0, 7)
    const units: CompUnit[] = core.map((u): CompUnit => {
      const avgItems = u.sum_items / u.games
      return {
        ...describe('unit', u.character_id),
        star: Math.max(1, Math.round(u.sum_star / u.games)),
        freq: u.games / c.games,
        avgItems,
        // Only show items on units that usually carry them.
        items: avgItems >= 1.5
          ? its.filter((i) => i.character_id === u.character_id).slice(0, 3).map((i) => ({ ...describe('item', i.item), freq: i.games / u.games }))
          : [],
      }
    }).sort((a, b) => Number(a.cost ?? 9) - Number(b.cost ?? 9) || a.name.localeCompare(b.name))

    const coreIds = new Set(core.map((u) => u.character_id))
    const flex = us.filter((u) => !coreIds.has(u.character_id) && u.games / c.games >= 0.15).slice(0, 8)
      .map((u) => ({ ...describe('unit', u.character_id), freq: u.games / c.games }))

    // Traits active in at least half the games, shown at their most common level.
    const byTrait = new Map<string, any[]>()
    for (const t of traitRows.get(c.comp_key) ?? []) byTrait.set(t.name, [...(byTrait.get(t.name) ?? []), t])
    const traits = [...byTrait.values()]
      .filter((levels) => levels.reduce((n, l) => n + l.games, 0) / c.games >= 0.5)
      .map((levels) => levels[0]) // rows arrive most-played first
      .sort((a, b) => b.num_units - a.num_units)
      .slice(0, 9)
      .map((t) => ({ ...describe('trait', t.name), units: t.num_units as number, style: t.style as number }))

    // Carries: the units holding three items in most games. They name the comp.
    const carryRows = us.filter((u) => u.full_items / c.games >= 0.4).sort((a, b) => b.full_items - a.full_items).slice(0, 2)
    const carries = (carryRows.length ? carryRows.map((u) => u.character_id) : [carryId]).map((id) => describe('unit', id))
    const trait = traitId === 'Flex' ? null : describe('trait', traitId)

    const avgLevel = c.sum_level / c.games
    const main = us.find((u) => u.character_id === carries[0].id)
    const carryCost = Number(carries[0].cost ?? 0)
    const reroll = carryCost >= 1 && carryCost <= 3 && main && main.sum_star / main.games >= 2.5
    const play = reroll ? { kind: 'reroll', label: `Reroll lvl ${Math.round(avgLevel)}` }
      : avgLevel >= 8.6 ? { kind: 'fast', label: 'Fast 9' }
      : avgLevel >= 7.6 ? { kind: 'fast', label: 'Fast 8' }
      : { kind: 'level', label: `Lvl ${Math.round(avgLevel)}` }

    return {
      id: c.comp_key,
      name: [trait?.name ?? 'Flex', ...carries.map((u) => u.name)].join(' '),
      trait, carries, units, flex, traits, avgLevel, play,
      pickRate: s.matches ? c.games / s.matches : 0, // boards per lobby, like MetaTFT's pick rate
      ...rate(c, s.comps),
    }
  })
  if (compCache.size > 24) compCache.clear()
  compCache.set(cacheKey, comps)
  return comps
}

const marks = () => {
  const rows = db.prepare('SELECT comp_key, fav, hidden FROM comp_marks').all() as { comp_key: string; fav: number; hidden: number }[]
  return {
    favs: rows.filter((r) => r.fav).map((r) => r.comp_key),
    hidden: rows.filter((r) => r.hidden).map((r) => r.comp_key),
    pinned: kvGet<string>(db, 'pinned'),
  }
}

app.get('/api/comps', (c) => {
  const s = scope(c)
  return c.json({ ...pub(s), ...marks(), rows: loadComps(s).filter((r) => r.games >= s.min) })
})

app.get('/api/comps/:id', (c) => {
  const s = scope(c)
  const id = c.req.param('id')
  const comp = loadComps(s).find((r) => r.id === id)
  if (!comp) return c.json({ ...pub(s), comp: null })
  const k = [s.patch, s.key, id]
  const levels = (db.prepare('SELECT * FROM stat_comp_levels WHERE patch = ? AND bucket = ? AND comp_key = ? ORDER BY level').all(...k) as any[])
    .filter((l) => l.level > 0)
    .map((l) => ({ level: l.level, games: l.games, share: l.games / comp.games, avgPlace: l.sum_place / l.games }))
  const items = db.prepare('SELECT * FROM stat_comp_items WHERE patch = ? AND bucket = ? AND comp_key = ? ORDER BY games DESC').all(...k) as any[]
  const unitGames = new Map((db.prepare('SELECT character_id, games FROM stat_comp_units WHERE patch = ? AND bucket = ? AND comp_key = ?').all(...k) as any[])
    .map((u) => [u.character_id, u.games]))
  const builds = comp.units.filter((u) => u.avgItems >= 1.5).sort((a, b) => b.avgItems - a.avgItems).map((u) => ({
    unit: u,
    items: items.filter((i) => i.character_id === u.id).slice(0, 8).map((i) => ({
      ...describe('item', i.item), games: i.games, freq: i.games / (unitGames.get(u.id) ?? i.games),
      avgPlace: i.sum_place / i.games, placeChange: i.sum_place / i.games - comp.avgPlace,
    })),
  }))
  return c.json({ ...pub(s), comp, levels, builds })
})

// ---- Units -------------------------------------------------------------------

app.get('/api/units', (c) => {
  const s = scope(c)
  const rows = db.prepare('SELECT * FROM stat_units WHERE patch = ? AND bucket = ? AND games >= ?').all(s.patch, s.key, s.min) as any[]
  return c.json({ ...pub(s), rows: rows.map((r) => ({ ...describe('unit', r.character_id), ...rate(r, s.comps), avgStar: r.avg_star })) })
})

app.get('/api/units/:id', (c) => {
  const s = scope(c)
  const id = c.req.param('id')
  const k = [s.patch, s.key, id]
  const unit = db.prepare('SELECT * FROM stat_units WHERE patch = ? AND bucket = ? AND character_id = ?').get(...k) as any
  if (!unit) return c.json({ ...pub(s), unit: describe('unit', id, true), items: [], builds: [], stars: [], itemCounts: [], comps: [], trend: [], prev: null })

  const dist = db.prepare('SELECT * FROM stat_unit_dist WHERE patch = ? AND bucket = ? AND character_id = ?').all(...k) as any[]
  const fold = (field: 'star' | 'n_items') => {
    const m = new Map<number, { games: number; sum: number }>()
    for (const d of dist) {
      const cur = m.get(d[field]) ?? { games: 0, sum: 0 }
      m.set(d[field], { games: cur.games + d.games, sum: cur.sum + d.sum_place })
    }
    return [...m].sort((a, b) => a[0] - b[0]).map(([value, v]) => ({ value, games: v.games, share: v.games / unit.games, avgPlace: v.sum / v.games }))
  }

  const items = db.prepare('SELECT * FROM stat_unit_items WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?').all(...k, s.min) as any[]
  const builds = db.prepare(`SELECT * FROM stat_unit_builds WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?
    ORDER BY games DESC LIMIT 80`).all(...k, s.min) as any[]

  // Same unit, same rank filter, every patch we have stats for.
  const trend = (db.prepare(`SELECT su.patch, su.games, su.sum_place, su.wins, su.top4, sm.comps FROM stat_units su
    JOIN stat_meta sm ON sm.patch = su.patch AND sm.bucket = su.bucket WHERE su.bucket = ? AND su.character_id = ?`).all(s.key, id) as any[])
    .sort((a, b) => patchOrder(b.patch, a.patch)).map((r) => ({ patch: r.patch, ...rate(r, r.comps) }))
  const prevRow = trend.find((t) => t.patch === s.prevPatch) ?? null

  const comps = loadComps(s).filter((comp) => comp.units.some((u) => u.id === id)).sort((a, b) => b.games - a.games).slice(0, 12)
    .map((comp) => ({ id: comp.id, name: comp.name, games: comp.games, avgPlace: comp.avgPlace, pickRate: comp.pickRate, units: comp.units, play: comp.play }))

  return c.json({
    ...pub(s),
    unit: { ...describe('unit', id, true), ...rate(unit, s.comps), avgStar: unit.avg_star },
    prev: prevRow ? { patch: prevRow.patch, avgPlace: prevRow.avgPlace, frequency: prevRow.frequency } : null,
    stars: fold('star'), itemCounts: fold('n_items'), trend, comps,
    items: items.map((r) => ({ ...describe('item', r.item), ...rate(r, unit.games), placeChange: r.place_change })),
    builds: builds.map((r) => ({
      id: r.build, items: (r.build as string).split('|').map((i) => describe('item', i)),
      ...rate(r, unit.games), placeChange: r.place_change,
    })),
  })
})

// ---- Traits ------------------------------------------------------------------

app.get('/api/traits', (c) => {
  const s = scope(c)
  const rows = db.prepare('SELECT * FROM stat_traits WHERE patch = ? AND bucket = ?').all(s.patch, s.key) as any[]
  const out = rows.filter((r) => r.tier_current === 0 && r.games >= s.min).map((r) => ({
    ...describe('trait', r.name),
    ...rate(r, s.comps),
    levels: rows.filter((l) => l.name === r.name && l.tier_current > 0).sort((a, b) => a.tier_current - b.tier_current)
      .map((l) => ({ level: l.tier_current, units: l.num_units, ...rate(l, s.comps) })),
  }))
  return c.json({ ...pub(s), rows: out })
})

// ---- Items -------------------------------------------------------------------

app.get('/api/items', (c) => {
  const s = scope(c)
  const rows = db.prepare('SELECT * FROM stat_items WHERE patch = ? AND bucket = ? AND games >= ?').all(s.patch, s.key, s.min) as any[]
  // Top five holders per item, from one pass over the unit-item table.
  const holders = new Map<string, string[]>()
  for (const h of db.prepare('SELECT item, character_id FROM stat_unit_items WHERE patch = ? AND bucket = ? ORDER BY games DESC')
    .iterate(s.patch, s.key) as Iterable<{ item: string; character_id: string }>) {
    const list = holders.get(h.item) ?? []
    if (list.length < 5) holders.set(h.item, [...list, h.character_id])
  }
  return c.json({
    ...pub(s),
    rows: rows.map((r) => ({
      ...describe('item', r.item), ...rate(r, s.comps), placeChange: r.place_change,
      holders: (holders.get(r.item) ?? []).map((id) => describe('unit', id)),
    })),
  })
})

app.get('/api/items/:id', (c) => {
  const s = scope(c)
  const id = c.req.param('id')
  const full = describe('item', id, true)
  const recipe = ((full.composition as string[]) ?? []).map((i) => describe('item', i))
  const item = db.prepare('SELECT * FROM stat_items WHERE patch = ? AND bucket = ? AND item = ?').get(s.patch, s.key, id) as any
  if (!item) return c.json({ ...pub(s), item: full, recipe, holders: [], prev: null })
  const prev = s.prevPatch
    ? (db.prepare(`SELECT si.*, sm.comps FROM stat_items si JOIN stat_meta sm ON sm.patch = si.patch AND sm.bucket = si.bucket
        WHERE si.patch = ? AND si.bucket = ? AND si.item = ?`).get(s.prevPatch, s.key, id) as any)
    : null
  const holders = db.prepare('SELECT * FROM stat_unit_items WHERE patch = ? AND bucket = ? AND item = ? AND games >= ?').all(s.patch, s.key, id, s.min) as any[]
  return c.json({
    ...pub(s),
    item: { ...full, ...rate(item, s.comps), placeChange: item.place_change },
    recipe,
    prev: prev ? { patch: s.prevPatch, avgPlace: prev.sum_place / prev.games, frequency: prev.games / prev.comps } : null,
    holders: holders.map((r) => ({ ...describe('unit', r.character_id), ...rate(r, item.games), placeChange: r.place_change })),
  })
})

// ---- Explorer: live questions against the raw boards ---------------------------

app.get('/api/explore', (c) => {
  const s = scope(c)
  const ids = (q: string, max: number) => (c.req.query(q) ?? '').split(',').map((x) => x.trim()).filter(Boolean).slice(0, max)
  const units = ids('units', 4)
  const traits = ids('traits', 3)
  if (!units.length) return c.json({ ...pub(s), summary: null, partners: [], items: [], ms: 0 })

  const t0 = Date.now()
  db.exec('DROP TABLE IF EXISTS temp.hit')
  db.prepare(`
    CREATE TEMP TABLE hit AS
    SELECT DISTINCT pa.id, pa.placement FROM units u0
    JOIN participants pa ON pa.id = u0.participant_id
    JOIN matches m ON m.match_id = pa.match_id
    WHERE u0.character_id = ? AND m.patch = ? AND m.rank >= ? AND m.game_datetime >= ? AND ${rankedOnly()}
    ${units.slice(1).map(() => 'AND EXISTS (SELECT 1 FROM units x WHERE x.participant_id = pa.id AND x.character_id = ?)').join('\n')}
    ${traits.map(() => 'AND EXISTS (SELECT 1 FROM traits t WHERE t.participant_id = pa.id AND t.name = ?)').join('\n')}
  `).run(units[0], s.patch, s.rankMin, s.since, ...units.slice(1), ...traits)

  const sum = db.prepare('SELECT COUNT(*) games, COALESCE(SUM(placement), 0) sum_place, COALESCE(SUM(placement = 1), 0) wins, COALESCE(SUM(placement <= 4), 0) top4 FROM hit').get() as Rates
  if (!sum.games) return c.json({ ...pub(s), summary: { games: 0 }, partners: [], items: [], ms: Date.now() - t0 })

  const partners = db.prepare(`
    SELECT character_id, COUNT(*) games, SUM(placement) sum_place, SUM(placement = 1) wins, SUM(placement <= 4) top4
    FROM (SELECT DISTINCT u.participant_id, u.character_id, h.placement FROM hit h JOIN units u ON u.participant_id = h.id)
    GROUP BY character_id ORDER BY games DESC LIMIT 40`).all() as any[]
  const items = db.prepare(`
    SELECT item, COUNT(*) games, SUM(placement) sum_place, SUM(placement = 1) wins, SUM(placement <= 4) top4
    FROM (SELECT DISTINCT ui.participant_id, ui.item, h.placement FROM hit h JOIN unit_items ui ON ui.participant_id = h.id WHERE ui.character_id = ?)
    GROUP BY item ORDER BY games DESC LIMIT 30`).all(units[0]) as any[]
  db.exec('DROP TABLE IF EXISTS temp.hit')

  const summary = rate(sum, s.comps)
  return c.json({
    ...pub(s), summary, ms: Date.now() - t0,
    partners: partners.filter((p) => !units.includes(p.character_id))
      .map((p) => ({ ...describe('unit', p.character_id), ...rate(p, sum.games), placeChange: p.sum_place / p.games - summary.avgPlace })),
    items: items.map((i) => ({ ...describe('item', i.item), ...rate(i, sum.games), placeChange: i.sum_place / i.games - summary.avgPlace })),
    itemsOn: describe('unit', units[0]),
  })
})

// ---- Static data for pickers and the team builder -------------------------------

app.get('/api/static', (c) => {
  const all = (kind: 'unit' | 'trait' | 'item') =>
    (db.prepare('SELECT id FROM static_data WHERE kind = ? ORDER BY name').all(kind) as { id: string }[]).map((r) => describe(kind, r.id))
  // No download yet: fall back to whatever has shown up in matches, so pickers still work.
  const seen = (table: string, col: string, kind: 'unit' | 'trait') =>
    (db.prepare(`SELECT DISTINCT ${col} id FROM ${table}`).all() as { id: string }[]).map((r) => describe(kind, r.id)).sort((a, b) => a.name.localeCompare(b.name))
  const units = all('unit').filter((u) => Number(u.cost) >= 1 && Number(u.cost) <= 5 && (u.traits as string[]).length > 0)
  const traits = all('trait')
  return c.json({
    loaded: units.length > 0,
    units: units.length ? units : seen('stat_units', 'character_id', 'unit'),
    traits: traits.length ? traits : seen('stat_traits', 'name', 'trait'),
    items: all('item'),
  })
})

// ---- Your lists ----------------------------------------------------------------

const teams = () => (db.prepare('SELECT id, name, board, created FROM saved_teams ORDER BY created DESC').all() as any[])
  .map((t) => ({ id: t.id, name: t.name, created: t.created, board: JSON.parse(t.board) }))

app.get('/api/lists', (c) => c.json({ ...marks(), teams: teams() }))

app.post('/api/lists/mark', async (c) => {
  const { key, fav, hidden } = (await c.req.json()) as { key: string; fav?: boolean; hidden?: boolean }
  db.prepare('INSERT OR IGNORE INTO comp_marks (comp_key) VALUES (?)').run(key)
  if (fav !== undefined) db.prepare('UPDATE comp_marks SET fav = ? WHERE comp_key = ?').run(fav ? 1 : 0, key)
  if (hidden !== undefined) db.prepare('UPDATE comp_marks SET hidden = ? WHERE comp_key = ?').run(hidden ? 1 : 0, key)
  return c.json(marks())
})

app.post('/api/lists/pin', async (c) => {
  const { key } = (await c.req.json()) as { key: string | null }
  if (key) kvSet(db, 'pinned', key)
  else db.prepare("DELETE FROM kv WHERE key = 'pinned'").run()
  return c.json(marks())
})

app.post('/api/teams', async (c) => {
  const { name, board } = (await c.req.json()) as { name: string; board: unknown }
  db.prepare('INSERT INTO saved_teams (name, board, created) VALUES (?,?,?)').run(String(name).slice(0, 60) || 'Untitled team', JSON.stringify(board), Date.now())
  return c.json({ teams: teams() })
})

app.delete('/api/teams/:id', (c) => {
  db.prepare('DELETE FROM saved_teams WHERE id = ?').run(Number(c.req.param('id')))
  return c.json({ teams: teams() })
})

// ---- Status --------------------------------------------------------------------

app.get('/api/meta', (c) => {
  const s = scope(c)
  // Ranked matches and their boards only (E12), as in the stats. Other game modes are stored but not counted.
  const totals = db.prepare(`SELECT (SELECT COUNT(*) FROM matches m WHERE ${rankedOnly()}) matches,
    (SELECT COUNT(*) FROM participants pa JOIN matches m ON m.match_id = pa.match_id WHERE ${rankedOnly()}) comps,
    (SELECT COUNT(*) FROM players) players, (SELECT COUNT(*) FROM match_queue WHERE status = 0) queued,
    (SELECT MAX(game_datetime) FROM matches) newest, (SELECT COUNT(*) FROM static_data) staticRows`).get()
  const byPatch = db.prepare(`SELECT patch, COUNT(*) matches FROM matches m WHERE ${rankedOnly()}
    GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT 8`).all()
  return c.json({
    ...pub(s), totals, byPatch,
    buckets: BUCKETS.map((b) => b.key),
    windows: WINDOWS.map((w) => ({ key: w.key, label: w.label })),
  })
})

registerMe(app, db, describe)

// Icons are downloaded from Community Dragon once, then served from disk.
const iconSource = db.prepare('SELECT icon FROM static_data WHERE kind = ? AND id = ? COLLATE NOCASE')
app.get('/api/icon/:kind/:id', async (c) => {
  const s = iconSource.get(c.req.param('kind'), c.req.param('id')) as { icon: string | null } | undefined
  if (!s?.icon) return c.notFound()
  const file = join(iconDir, createHash('sha1').update(s.icon).digest('hex') + '.png')
  if (!existsSync(file)) {
    try {
      const res = await fetch(s.icon, { signal: AbortSignal.timeout(15_000) })
      if (!res.ok) return c.notFound()
      writeFileSync(file, Buffer.from(await res.arrayBuffer()))
    } catch {
      return c.notFound()
    }
  }
  return c.body(readFileSync(file), 200, { 'Content-Type': 'image/png', 'Cache-Control': 'public, max-age=604800' })
})

// "npm run dev" passes --dev: the API sits behind Vite's dev server.
// Otherwise ("npm start", after "npm run build") this one server hosts the app and the API together.
if (!dev && existsSync('dist/index.html')) {
  app.use('/*', serveStatic({ root: './dist' }))
  app.get('*', (c) => c.html(readFileSync('dist/index.html', 'utf8')))
}

const port = dev ? config.port + 1 : config.port
serve({ fetch: app.fetch, port, hostname: '127.0.0.1' }, () =>
  console.log(dev ? `API on http://127.0.0.1:${port}` : `Hertz running at http://127.0.0.1:${port}`))
