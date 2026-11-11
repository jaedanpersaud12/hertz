/**
 * Parity check: the Rust engine must store exactly what the TypeScript engine stores.
 *
 *   npm run parity
 *
 * 1. Ingest: the same match files go through both engines into two fresh databases. Schema and every raw
 *    table are compared row by row, in insertion order. Includes awkward cases on top of the fake world.
 * 2. Stats: copies of the ingested database get the same spread of ranks and dates, then their stat tables
 *    are built by each engine. Every stat table is compared row by row in primary key order, every value
 *    exactly; stat_meta.built_at is a timestamp and is left out. A check that the buckets and the 3-day
 *    window really narrow the input keeps the comparison from passing vacuously.
 * 3. Crawl: both crawlers run against the same fake Riot API (rate limits, a forced 429, server errors).
 *    The databases are compared by content, since the two crawlers fetch in a different order.
 * 4. API: the stats databases from step 2 are served by the TypeScript API and by `tft serve`, and the same
 *    read-only URLs (every route, scope and parameter, ids that exist and ids that do not, bad values) are
 *    requested from both. Status, content type and body must match; JSON bodies are compared after parsing.
 *    The summoner routes (/api/me*) and the write routes (POST /api/lists/*, POST and DELETE /api/teams*)
 *    are not ported yet and are not checked here.
 *
 * Exits non-zero on any difference.
 */
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { request, createServer, type Server } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import Database from 'better-sqlite3'
import { rankedMatches, startFakeRiot } from '../test/world'

const dir = mkdtempSync(join(tmpdir(), 'tft-parity-'))
const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'debug', process.platform === 'win32' ? 'tft.exe' : 'tft')
let failures = 0
const ok = (label: string) => console.log(`  ok  ${label}`)
const fail = (label: string, detail: string) => {
  failures++
  console.log(`  FAIL ${label}\n${detail}`)
}

// Calendar both engines date "?.?.?.?" matches against.
const CALENDAR = [
  { patch: '16.21', since: Date.parse('2026-10-21T07:15:23Z') },
  { patch: '16.22', since: Date.parse('2026-11-04T07:47:21Z') },
]

/** Matches the fake world never produces but Riot can. */
function awkwardMatches() {
  const base = structuredClone(rankedMatches[0])
  const mk = (id: string, edit: (m: any) => void) => {
    const m = structuredClone(base)
    m.metadata.match_id = id
    edit(m)
    return m
  }
  return [
    // Live Riot data: no version, dated by calendar, either side of a patch boundary.
    mk('EDGE_UNREAL_1', (m) => { m.info.game_version = 'TFT Unreal Version ?.?.?.?'; m.info.game_datetime = Date.parse('2026-11-03T12:00:00Z') }),
    mk('EDGE_UNREAL_2', (m) => { m.info.game_version = 'TFT Unreal Version ?.?.?.?'; m.info.game_datetime = Date.parse('2026-11-08T12:00:00Z') }),
    mk('EDGE_NOVERSION', (m) => { delete m.info.game_version; m.info.game_datetime = Date.parse('2026-01-01T00:00:00Z') }),
    // Blank item names, missing rarity, missing items, a board of three units.
    mk('EDGE_UNITS', (m) => {
      const p = m.info.participants
      p[0].units[0].itemNames = ['', 'TFT_Item_Thing1', '', 'TFT_Item_Thing0', 'TFT_Item_Thing9']
      delete p[1].units[0].rarity
      delete p[2].units[1].itemNames
      p[3].units = p[3].units.slice(0, 3)
      delete p[4].units
      p[5].traits = []
    }),
    // Ties that only dictionary ordering breaks: case, underscores, equal items and cost.
    mk('EDGE_TIES', (m) => {
      const p = m.info.participants[0]
      p.traits = [
        { name: 'TFT99_bastion', num_units: 4, style: 2, tier_current: 2 },
        { name: 'TFT99_Bilgewater', num_units: 4, style: 2, tier_current: 2 },
        { name: 'TFT99_A-B', num_units: 4, style: 2, tier_current: 2 },
        { name: 'TFT99_A_B', num_units: 4, style: 2, tier_current: 2 },
      ]
      p.units = ['TFT99_kogmaw', 'TFT99_KogMaw', 'TFT99_Kog_Maw', 'TFT99_Ekko'].map((id) => ({
        character_id: id, tier: 2, rarity: 4, itemNames: ['TFT_Item_Thing1', 'TFT_Item_Thing2'],
      }))
    }),
    // The same match twice: stored once.
    structuredClone(base),
  ]
}

function compareOrdered(a: Database.Database, b: Database.Database, table: string) {
  const rows = (db: Database.Database) => db.prepare(`SELECT * FROM ${table} ORDER BY rowid`).all()
  const [ra, rb] = [rows(a), rows(b)]
  const first = ra.findIndex((r, i) => JSON.stringify(r) !== JSON.stringify(rb[i]))
  if (ra.length === rb.length && first < 0) return ok(`${table}: ${ra.length} rows identical`)
  fail(`${table}`, `    TypeScript ${ra.length} rows, Rust ${rb.length} rows. First difference at row ${first}:\n` +
    `    TS:   ${JSON.stringify(ra[first])}\n    Rust: ${JSON.stringify(rb[first])}`)
}

/** Content comparison that ignores participant ids and row order. */
function compareContent(a: Database.Database, b: Database.Database, label: string, sql: string) {
  const rows = (db: Database.Database) => (db.prepare(sql).raw().all() as unknown[][]).map((r) => JSON.stringify(r)).sort()
  const [ra, rb] = [rows(a), rows(b)]
  const onlyA = ra.filter((r) => !rb.includes(r))
  const onlyB = rb.filter((r) => !ra.includes(r))
  if (!onlyA.length && !onlyB.length && ra.length === rb.length) return ok(`${label}: ${ra.length} rows identical`)
  fail(label, `    TypeScript ${ra.length}, Rust ${rb.length}. Only in TS: ${onlyA.slice(0, 2).join(' ')} Only in Rust: ${onlyB.slice(0, 2).join(' ')}`)
}

/** Row-by-row comparison of a stat table in primary key order, every value exact. stat_meta.built_at is skipped. */
function compareStats(a: Database.Database, b: Database.Database, table: string) {
  const info = a.prepare(`PRAGMA table_info(${table})`).all() as { name: string; pk: number }[]
  const keys = info.filter((c) => c.pk > 0).sort((x, y) => x.pk - y.pk).map((c) => c.name)
  const cols = info.map((c) => c.name).filter((c) => !(table === 'stat_meta' && c === 'built_at'))
  const sql = `SELECT ${cols.join(', ')} FROM ${table} ORDER BY ${keys.join(', ')}`
  const rows = (db: Database.Database) => (db.prepare(sql).all() as unknown[]).map((r) => JSON.stringify(r))
  const [ra, rb] = [rows(a), rows(b)]
  if (!ra.length) return fail(table, '    the TypeScript engine wrote no rows, so this check proves nothing')
  const first = ra.findIndex((r, i) => r !== rb[i])
  if (ra.length === rb.length && first < 0) return ok(`${table}: ${ra.length} rows identical`)
  const at = first < 0 ? ra.length : first
  fail(table, `    TypeScript ${ra.length} rows, Rust ${rb.length} rows. First difference at row ${at}:\n` +
    `    TS:   ${ra[at]}\n    Rust: ${rb[at]}`)
}

const run = (cmd: string, args: string[], env: Record<string, string>) =>
  new Promise<string>((resolve, reject) => {
    const child = spawn(cmd, args, { cwd: root, env: { ...process.env, NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost', ...env } })
    let out = ''
    child.stdout.on('data', (d) => (out += d))
    child.stderr.on('data', (d) => (out += d))
    child.on('exit', (code) => (code === 0 ? resolve(out) : reject(new Error(`${cmd} ${args.join(' ')} exited ${code}\n${out}`))))
  })

// ---- API helpers (section 4) ------------------------------------------------------------

type Reply = { status: number; type: string; cache: string; body: Buffer }

/** A raw GET: the path goes out exactly as written, so "..", "%2e" and double slashes reach the server. */
const httpGet = (port: number, path: string) =>
  new Promise<Reply>((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port, path, method: 'GET', agent: false }, (res) => {
      const chunks: Buffer[] = []
      res.on('data', (c) => chunks.push(c))
      res.on('end', () => resolve({
        status: res.statusCode ?? 0, type: String(res.headers['content-type'] ?? ''),
        cache: String(res.headers['cache-control'] ?? ''), body: Buffer.concat(chunks),
      }))
    })
    req.on('error', reject)
    req.setTimeout(30_000, () => req.destroy(new Error(`timed out: ${path}`)))
    req.end()
  })

const freePort = () =>
  new Promise<number>((resolve) => {
    const s = createServer()
    s.listen(0, '127.0.0.1', () => {
      const port = (s.address() as { port: number }).port
      s.close(() => resolve(port))
    })
  })

/** Starts a server process and waits until /api/meta answers. */
async function startServer(label: string, cmd: string, args: string[], cwd: string, env: Record<string, string>, port: number) {
  const child = spawn(cmd, args, {
    cwd, env: { ...process.env, NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost', RIOT_API_KEY: '', RIOT_ID: '', ...env },
  })
  let out = ''
  child.stdout.on('data', (d) => (out += d))
  child.stderr.on('data', (d) => (out += d))
  for (let i = 0; i < 300; i++) {
    if (child.exitCode !== null) throw new Error(`${label} server exited ${child.exitCode}\n${out}`)
    try {
      await httpGet(port, '/api/meta')
      return { child, log: () => out }
    } catch {
      await new Promise((r) => setTimeout(r, 100))
    }
  }
  child.kill()
  throw new Error(`${label} server did not start on port ${port}\n${out}`)
}
const stop = (child: ChildProcess) =>
  new Promise<void>((resolve) => {
    if (child.exitCode !== null) return resolve()
    child.on('exit', () => resolve())
    child.kill()
  })

/** First difference between two parsed JSON values, or null. Numbers compare with ===, so 0 and -0 are equal. */
function jsonDiff(a: unknown, b: unknown, at = '$'): string | null {
  if (typeof a !== typeof b || Array.isArray(a) !== Array.isArray(b) || (a === null) !== (b === null)) {
    return `${at}: TS ${JSON.stringify(a)?.slice(0, 200)}, Rust ${JSON.stringify(b)?.slice(0, 200)}`
  }
  if (Array.isArray(a)) {
    const bb = b as unknown[]
    if (a.length !== bb.length) return `${at}: TS has ${a.length} entries, Rust ${bb.length}`
    for (let i = 0; i < a.length; i++) {
      const d = jsonDiff(a[i], bb[i], `${at}[${i}]`)
      if (d) return d
    }
    return null
  }
  if (a && typeof a === 'object') {
    const [ao, bo] = [a as Record<string, unknown>, b as Record<string, unknown>]
    const keys = new Set([...Object.keys(ao), ...Object.keys(bo)])
    for (const k of [...keys].sort()) {
      if (!(k in ao) || !(k in bo)) return `${at}.${k}: only in ${k in ao ? 'TS' : 'Rust'} (${JSON.stringify(ao[k] ?? bo[k])?.slice(0, 120)})`
      const d = jsonDiff(ao[k], bo[k], `${at}.${k}`)
      if (d) return d
    }
    return null
  }
  return a === b ? null : `${at}: TS ${JSON.stringify(a)}, Rust ${JSON.stringify(b)}`
}

/** One URL to check. `has` says what the TypeScript answer must contain, so the comparison cannot pass on nothing. */
type Check = { route: string; url: string; has?: (body: any) => boolean }

async function compareUrls(tsPort: number, rsPort: number, checks: Check[]) {
  const byRoute = new Map<string, { n: number; bad: number; withData: number }>()
  for (const c of checks) {
    const tally = byRoute.get(c.route) ?? { n: 0, bad: 0, withData: 0 }
    byRoute.set(c.route, tally)
    tally.n++
    const [a, b] = [await httpGet(tsPort, c.url), await httpGet(rsPort, c.url)]
    const problem = (() => {
      if (a.status !== b.status) return `status: TS ${a.status}, Rust ${b.status}`
      if (a.type !== b.type) return `content type: TS "${a.type}", Rust "${b.type}"`
      if (a.cache !== b.cache) return `cache-control: TS "${a.cache}", Rust "${b.cache}"`
      if (!a.type.startsWith('application/json')) {
        return a.body.equals(b.body) ? null : `body: TS ${JSON.stringify(a.body.toString().slice(0, 80))}, Rust ${JSON.stringify(b.body.toString().slice(0, 80))}`
      }
      const [ja, jb] = [JSON.parse(a.body.toString()), JSON.parse(b.body.toString())]
      // The Explorer reports how long its query took; only its presence is comparable.
      if ('ms' in ja || 'ms' in jb) {
        if (typeof ja.ms !== 'number' || typeof jb.ms !== 'number' || jb.ms < 0) return `ms: TS ${ja.ms}, Rust ${jb.ms}`
        ja.ms = jb.ms = 0
      }
      if (c.has) {
        if (!c.has(ja)) return 'the TypeScript answer has no data here, so this URL proves nothing'
        tally.withData++
      }
      return jsonDiff(ja, jb)
    })()
    if (problem) {
      tally.bad++
      if (tally.bad <= 3) fail(`GET ${c.url}`, `    ${problem}`)
    }
  }
  for (const [route, t] of byRoute) {
    const detail = `${t.n} URLs${t.withData ? `, ${t.withData} with data` : ''}`
    if (!t.bad) ok(`${route}: ${detail} identical`)
    else fail(route, `    ${t.bad} of ${detail} differ`)
  }
}

const nonEmpty = (b: any) => Array.isArray(b.rows) && b.rows.length > 0

/**
 * Comps and Explorer data built to sit on every cut-off the API applies: core units at exactly half the games and
 * more than ten of them, too few core units, more flex units, traits and carries than are shown, levels on the
 * Fast 9 / Fast 8 / reroll lines, halves that round up, a unit in more than twelve comps, and an Explorer board
 * with more partners and items than are listed. The fixture world never produces these, so they go in as rows.
 */
function seedEdges(db: Database.Database, now: number) {
  const [P, B] = ['98.1', 'ALL']
  db.prepare('INSERT INTO stat_meta (patch, bucket, matches, comps, built_at) VALUES (?, ?, ?, ?, ?)').run(P, B, 250, 2000, 1_750_000_000_000)
  const comp = db.prepare('INSERT INTO stat_comps (patch, bucket, comp_key, games, sum_place, wins, top4, sum_level) VALUES (?, ?, ?, ?, ?, ?, ?, ?)')
  const unit = db.prepare('INSERT INTO stat_comp_units (patch, bucket, comp_key, character_id, games, sum_star, sum_items, full_items, sum_place) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)')
  const item = db.prepare('INSERT INTO stat_comp_items (patch, bucket, comp_key, character_id, item, games, sum_place) VALUES (?, ?, ?, ?, ?, ?, ?)')
  const trait = db.prepare('INSERT INTO stat_comp_traits (patch, bucket, comp_key, name, tier_current, num_units, style, games) VALUES (?, ?, ?, ?, ?, ?, ?, ?)')
  const level = db.prepare('INSERT INTO stat_comp_levels (patch, bucket, comp_key, level, games, sum_place) VALUES (?, ?, ?, ?, ?, ?)')
  const U = (i: number) => `TFT98_U${i}`

  // Twelve core candidates (ten shown), ten flex candidates (eight shown), three carries (two shown), avg level 8.6.
  const big = 'TFT98_Big|TFT98_U0'
  comp.run(P, B, big, 100, 380, 20, 60, 860)
  for (let i = 0; i < 12; i++) {
    const g = 100 - i * 4 - (i === 11 ? 6 : 0) // the last one sits on exactly half
    const items = i === 0 ? 3 : i === 1 ? 2 : i === 2 ? 1.5 : 0.5
    unit.run(P, B, big, U(i), g, g * 2, Math.round(g * items), i === 0 ? 90 : i === 1 ? 70 : i === 2 ? 40 : 10, g * 4)
  }
  ;[49, 45, 41, 37, 33, 29, 25, 21, 17, 15, 14].forEach((g, n) => unit.run(P, B, big, U(12 + n), g, g, 0, 0, g * 4 + n))
  for (let n = 0; n < 10; n++) item.run(P, B, big, U(0), `TFT98_I${n}`, 90 - n * 5, (90 - n * 5) * 3 + n)
  for (let n = 0; n < 4; n++) item.run(P, B, big, U(1), `TFT98_I${n}`, 60 - n * 7, (60 - n * 7) * 5)
  for (let n = 4; n < 6; n++) item.run(P, B, big, U(2), `TFT98_I${n}`, 30 - n, (30 - n) * 4)
  for (let n = 0; n < 11; n++) trait.run(P, B, big, `TFT98_T${n}`, 1, [2, 9, 4, 4, 7, 12, 3, 6, 1, 5, 8][n], n % 4, 50 + n)
  trait.run(P, B, big, 'TFT98_T0', 2, 4, 2, 10)
  trait.run(P, B, big, 'TFT98_T11', 1, 10, 1, 30) // only active in half the games across its two levels
  trait.run(P, B, big, 'TFT98_T11', 2, 11, 2, 20)
  trait.run(P, B, big, 'TFT98_T12', 1, 13, 1, 49)
  for (const [l, g] of [[0, 5], [7, 20], [8, 35], [9, 40]]) level.run(P, B, big, l, g, g * 4 - l)

  // Three core units: falls back to the seven most common. Avg level exactly 7.6.
  const small = 'TFT98_Small|TFT98_U1'
  comp.run(P, B, small, 80, 300, 10, 50, 608)
  ;[[1, 40], [5, 45], [6, 50], [7, 39], [8, 30], [9, 28], [10, 25], [11, 20]].forEach(([i, g]) => unit.run(P, B, small, U(i), g, g, g * 2, i === 1 ? 32 : 0, g * 4))
  ;[16, 13, 12].forEach((g, n) => unit.run(P, B, small, U(12 + n), g, g, 0, 0, g * 4)) // 16 and 12 of 80 sit either side of the 15% flex line

  // A two-cost carry at exactly 2.5 stars: reroll, at level 6.5 (rounds to 7).
  const reroll = 'TFT98_Reroll|TFT98_U2'
  comp.run(P, B, reroll, 60, 200, 15, 40, 390)
  ;[[2, 60, 150, 24], [3, 60, 60, 0], [4, 55, 55, 0], [5, 50, 50, 0]].forEach(([i, g, star, full]) => unit.run(P, B, reroll, U(i), g, star, g * 3, full, g * 4))

  // No trait, no carry holding items: named after the key's carry. Level 5.5 rounds to 6.
  const flex = 'Flex|TFT98_U3'
  comp.run(P, B, flex, 40, 180, 2, 20, 220)
  for (let i = 3; i < 8; i++) unit.run(P, B, flex, U(i), 40, 40, 0, 0, 180)

  // Just under the Fast 9 and Fast 8 lines.
  comp.run(P, B, 'TFT98_Edge|TFT98_U4', 100, 400, 12, 50, 859)
  comp.run(P, B, 'TFT98_Edge2|TFT98_U5', 100, 400, 12, 50, 759)
  for (const key of ['TFT98_Edge|TFT98_U4', 'TFT98_Edge2|TFT98_U5']) for (let i = 4; i < 9; i++) unit.run(P, B, key, U(i), 100, 100, 0, 0, 400)
  unit.run(P, B, 'TFT98_Edge|TFT98_U4', U(9), 50, 50, 0, 0, 200) // exactly half the games: a core unit

  // Fourteen more comps with TFT98_U0 in the core: its unit page lists twelve.
  for (let n = 0; n < 14; n++) {
    const key = `TFT98_Many${n}|TFT98_U0`
    comp.run(P, B, key, 21 + n, (21 + n) * 4, 3, 10, (21 + n) * 7)
    for (let i = 0; i < 4; i++) unit.run(P, B, key, U(i), 21 + n, 21 + n, 0, 0, (21 + n) * 4)
  }
  db.prepare('INSERT INTO stat_units (patch, bucket, character_id, games, sum_place, wins, top4, avg_star) VALUES (?, ?, ?, ?, ?, ?, ?, ?)').run(P, B, U(0), 300, 1200, 40, 160, 2.1)
  const put = db.prepare('INSERT OR REPLACE INTO static_data (kind, id, name, icon, data) VALUES (?, ?, ?, ?, ?)')
  for (let i = 0; i < 10; i++) put.run('unit', U(i), `Edge ${i}`, null, JSON.stringify({ cost: i === 2 ? 2 : 1 + (i % 5), traits: ['Edge'] }))

  // One Explorer board family: 45 partners and 35 items on TFT98_U0, in three lobbies.
  db.prepare('INSERT INTO matches (match_id, platform, patch, set_number, game_datetime, game_length, rank) VALUES (?, ?, ?, ?, ?, ?, ?)')
    .run('API_WIDE_1', 'na1', P, 98, now - 3_600_000, 1800.5, 9)
  const part = db.prepare('INSERT INTO participants (id, match_id, puuid, placement, level, last_round, gold_left, damage, comp_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)')
  const board = db.prepare('INSERT INTO units (participant_id, character_id, star, n_items, build, rarity) VALUES (?, ?, ?, ?, ?, ?)')
  const held = db.prepare('INSERT INTO unit_items (participant_id, character_id, item) VALUES (?, ?, ?)')
  ;[[1, 45, 35], [4, 30, 20], [7, 15, 10]].forEach(([place, partners, items], n) => {
    const id = 900_001 + n
    part.run(id, 'API_WIDE_1', `wide-${n}`, place, 9, 30, 0, 100, big)
    board.run(id, U(0), 2, 3, null, 0)
    for (let w = 0; w < partners; w++) board.run(id, `TFT98_W${w}`, 1, 0, null, 0)
    for (let x = 0; x < items; x++) held.run(id, U(0), `TFT98_X${x}`)
  })
}

/** Builds the URL list from what the fixture database holds. */
function apiChecks(db: Database.Database, phase: 'bare' | 'full', icons: { ok: string; missing: string; refused: string; cached: string }) {
  const q = (sql: string) => db.prepare(sql).all() as any[]
  const latest = '99.3'
  const comps = q(`SELECT comp_key FROM stat_comps WHERE patch = '${latest}' AND bucket = 'ALL' ORDER BY comp_key`).map((r) => r.comp_key as string)
  const units = q(`SELECT DISTINCT character_id FROM stat_units WHERE patch = '${latest}' AND bucket = 'ALL' ORDER BY character_id`).map((r) => r.character_id as string)
  const items = q(`SELECT DISTINCT item FROM stat_items WHERE patch = '${latest}' AND bucket = 'ALL' ORDER BY item`).map((r) => r.item as string)
  const enc = encodeURIComponent
  const checks: Check[] = []
  const add = (route: string, url: string, has?: (b: any) => boolean) => checks.push({ route, url, has })

  if (phase === 'bare') {
    // No static data and no built app: names come from ids, pickers fall back to what is in the stats.
    add('/api/static', '/api/static', (b) => b.loaded === false && b.units.length > 0 && b.traits.length > 0)
    add('/api/comps', '/api/comps', nonEmpty)
    add('/api/comps/:id', `/api/comps/${enc(comps[0])}`, (b) => b.comp !== null)
    add('/api/units/:id', '/api/units/TFT99_Unit6', (b) => b.unit.games > 0)
    add('/api/items/:id', '/api/items/TFT_Item_Thing1', (b) => b.item.games > 0)
    add('/api/explore', '/api/explore?units=TFT99_Unit6', (b) => b.summary.games > 0)
    add('/api/lists', '/api/lists')
    add('/api/icon/:kind/:id', '/api/icon/unit/TFT99_Unit1')
    for (const url of ['/', '/comps', '/api/nothing', '/api/comps/', '/api/comps/a/b']) add('no built app: 404', url)
    return checks
  }

  // Every scope: patch (newest by default, an old one, one with no 3-day data, one that does not exist),
  // rank bucket (each, the default, a lower-case one, an unknown one) and window (each, the default, an unknown one).
  const patches = ['', latest, '16.22', '16.21', 'nope']
  const buckets = ['', 'ALL', 'PLATINUM%2B', 'DIAMOND%2B', 'MASTER%2B', 'platinum%2B', 'PLATINUM+', 'GOLD']
  const windows = ['', '3d', '7d']
  const scopes: { qs: string; data: boolean }[] = []
  for (const p of patches) for (const bk of buckets) for (const w of windows) {
    const parts = [p && `patch=${p}`, bk && `bucket=${bk}`, w && `window=${w}`].filter(Boolean)
    scopes.push({ qs: parts.length ? `?${parts.join('&')}` : '', data: p === '' || p === latest })
  }
  // The minimum-games filter, including values Number() reads oddly.
  const mins = ['0', '1', '5', '25', '400', '5.5', '', 'abc', '1e1', '0x10', '-3', 'Infinity', '%207%20', '9999999']
  // Query strings parsed the way Hono parses them: the first value wins, names can be encoded, a bare name is empty.
  const quirks = ['?patch=16.22&patch=99.3', '?pat%63h=16.22', '?min', '?min=', '?bucket=MASTER%2B&min=3&window=3d', '?patch=&bucket=', '?window=3d&window=']

  for (const route of ['/api/comps', '/api/units', '/api/items', '/api/traits', '/api/meta']) {
    const has = route === '/api/meta' ? (b: any) => b.totals.matches > 0 && b.byPatch.length > 0 : nonEmpty
    for (const s of scopes) add(route, `${route}${s.qs}`, s.data ? has : undefined)
    for (const m of mins) add(route, `${route}?min=${m}`, route === '/api/meta' || ['0', '1', '5', '', '1e1', '%207%20'].includes(m) ? has : undefined)
    for (const qs of quirks) add(route, `${route}${qs}`)
  }

  // Detail routes: every id the fixture has, a sample of scopes, ids that do not exist, encoded ids.
  const someScopes = ['', '?bucket=MASTER%2B', '?window=3d', '?bucket=DIAMOND%2B&window=3d', '?patch=16.22', '?patch=16.21&window=3d', '?patch=nope', '?min=200', '?min=abc', '?min=2']
  for (const id of comps) {
    add('/api/comps/:id', `/api/comps/${enc(id)}`, (b) => b.comp !== null && b.levels.length > 0 && b.builds.length > 0)
    for (const s of someScopes) add('/api/comps/:id', `/api/comps/${enc(id)}${s}`)
  }
  add('/api/comps/:id', `/api/comps/${comps[0].replace('|', '%7c')}`, (b) => b.comp !== null) // lower-case escape
  add('/api/comps/:id', `/api/comps/${comps[0]}`, (b) => b.comp !== null) // raw "|"
  for (const id of ['nope', 'Flex%7CTFT99_Unit1', '%25', '%E0%A4%A', 'TFT99_Brawler%7C']) add('/api/comps/:id', `/api/comps/${id}`)

  for (const id of units) {
    add('/api/units/:id', `/api/units/${enc(id)}`, (b) => b.unit.games > 0)
    if (id.startsWith('TFT99_Unit1')) for (const s of someScopes) add('/api/units/:id', `/api/units/${enc(id)}${s}`)
  }
  for (const id of ['nope', 'tft99_unit6', 'TFT99%5FUnit6', 'TFT99_Unit6%2Fx', '%E0%A4%A', 'TFT99_KOGMAW']) add('/api/units/:id', `/api/units/${id}`)

  for (const id of items) {
    add('/api/items/:id', `/api/items/${enc(id)}`, (b) => b.item.games > 0)
    for (const s of someScopes.slice(0, 6)) add('/api/items/:id', `/api/items/${enc(id)}${s}`)
  }
  for (const id of ['nope', 'tft_item_thing5', 'TFT_Item_Spatula', 'TFT_Item_BFSword']) add('/api/items/:id', `/api/items/${id}`)

  // Explorer: one to five units (only four count), traits, spacing and empty entries, scopes, nothing found.
  const ex = (qs: string, has?: (b: any) => boolean) => add('/api/explore', `/api/explore?${qs}`, has)
  const found = (b: any) => b.summary?.games > 0 && b.partners.length > 0 && b.items.length > 0
  for (const u of units.slice(0, 8)) ex(`units=${u}`, found)
  ex('units=TFT99_Unit6,TFT99_Unit7', found)
  ex('units=TFT99_Unit6,TFT99_Unit7,TFT99_Unit5,TFT99_Unit4,TFT99_Unit19')
  ex('units=%20TFT99_Unit6%20,,TFT99_Unit7,', found)
  ex('units=TFT99_Unit6&traits=TFT99_Mystic', found)
  ex('units=TFT99_Unit6&traits=TFT99_Mystic,TFT99_Shared,TFT99_Loner,TFT99_Brawler')
  ex('units=TFT99_Unit6&bucket=MASTER%2B&window=3d', found)
  ex('units=TFT99_Unit6&patch=16.22')
  ex('units=TFT99_Unit6&patch=nope')
  ex('units=TFT99_Unit6&min=abc', found)
  ex('units=nope')
  ex('units=,,')
  ex('traits=TFT99_Mystic')
  add('/api/explore', '/api/explore')

  // The cut-offs, on the rows seedEdges adds.
  const edge = (b: any, key: string) => b.rows.find((r: any) => r.id === key)
  add('cut-offs', '/api/comps?patch=98.1&min=1', (b) => b.rows.length === 20 && edge(b, 'TFT98_Big|TFT98_U0').units.length === 10 &&
    edge(b, 'TFT98_Big|TFT98_U0').flex.length === 8 && edge(b, 'TFT98_Big|TFT98_U0').traits.length === 9 &&
    edge(b, 'TFT98_Small|TFT98_U1').units.length === 7 && edge(b, 'TFT98_Reroll|TFT98_U2').play.kind === 'reroll')
  for (const key of ['TFT98_Big|TFT98_U0', 'TFT98_Small|TFT98_U1', 'TFT98_Reroll|TFT98_U2', 'Flex|TFT98_U3', 'TFT98_Edge|TFT98_U4', 'TFT98_Edge2|TFT98_U5']) {
    add('cut-offs', `/api/comps/${enc(key)}?patch=98.1&min=1`, (b) => b.comp !== null)
  }
  add('cut-offs', '/api/units/TFT98_U0?patch=98.1', (b) => b.comps.length === 12)
  add('cut-offs', '/api/explore?units=TFT98_U0&patch=98.1', (b) => b.partners.length === 39 && b.items.length === 30) // 40 listed, less the unit asked about
  add('cut-offs', '/api/explore?units=TFT98_U0,TFT98_W3&patch=98.1&bucket=MASTER%2B', (b) => b.summary.games === 3)

  add('/api/static', '/api/static', (b) => b.loaded === true && b.units.length > 0 && b.items.length > 0)
  add('/api/lists', '/api/lists', (b) => b.favs.length > 0 && b.hidden.length > 0 && b.pinned && b.teams.length > 0)

  // Icons: downloaded, served from the cache on the second request, origin 404, origin unreachable, already cached
  // (origin unreachable), no icon, unknown id, letter case, wrong kind.
  for (const url of [
    icons.ok, icons.ok, icons.missing, icons.refused, icons.cached, '/api/icon/unit/TFT99_Unit0', '/api/icon/unit/nope',
    icons.ok.toLowerCase(), '/api/icon/item/TFT99_Unit1', '/api/icon/trait/TFT99_Brawler', '/api/icon/item/TFT_Item_Thing5',
    '/api/icon/unit/', '/api/icon/unit',
  ]) add('/api/icon/:kind/:id', url)

  // The built app and its fallback: files by type, folders, the page for every other path, paths that try to escape.
  for (const url of [
    '/', '/index.html', '/assets/app.js', '/assets/app.css', '/favicon.svg', '/assets', '/assets/', '/data.json', '/x.unknownext',
    '/comps', '/units/TFT99_Unit1', '/api/nothing', '/api/comps/', '/api/comps/a/b', '/api/icon/unit', '/../secret.txt',
    '/%2e%2e/secret.txt', '/assets/..%2fsecret.txt', '//index.html', '/assets%2Fapp.js', '/%E0%A4%A', '/a%2520b.txt',
  ]) add('built app', url)
  return checks
}

/** Section 4: the read-only API routes, served from the stats databases of section 2. */
async function apiParity(tsStats: string, rsStats: string) {
  const tsDir = join(dir, 'api-ts')
  const rsDir = join(dir, 'api-rs')
  const tsDb = join(tsDir, 'tft.db')
  const rsDb = join(rsDir, 'tft.db')
  for (const d of [tsDir, rsDir]) mkdirSync(join(d, 'icons'), { recursive: true })
  copyFileSync(tsStats, tsDb)
  copyFileSync(rsStats, rsDb)
  // built_at is when each engine ran its rebuild; everything else in the stat tables was proven equal above.
  {
    const a = new Database(tsDb, { readonly: true })
    const b = new Database(rsDb)
    const set = b.prepare('UPDATE stat_meta SET built_at = ? WHERE patch = ? AND bucket = ?')
    for (const r of a.prepare('SELECT patch, bucket, built_at FROM stat_meta').all() as any[]) set.run(r.built_at, r.patch, r.bucket)
    a.close()
    b.close()
  }

  // A stand-in for Community Dragon: one icon it has, one it does not, and an address nothing listens on.
  let originHits = 0
  const origin: Server = createServer((req, res) => {
    originHits++
    if (req.url === '/ok.png') res.writeHead(200, { 'Content-Type': 'image/png' }).end(Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2, 3]))
    else res.writeHead(404).end('no')
  })
  await new Promise<void>((r) => origin.listen(0, '127.0.0.1', () => r()))
  const originPort = (origin.address() as { port: number }).port
  const deadPort = await freePort()
  const src = {
    ok: `http://127.0.0.1:${originPort}/ok.png`,
    missing: `http://127.0.0.1:${originPort}/missing.png`,
    refused: `http://127.0.0.1:${deadPort}/refused.png`,
    cached: `http://127.0.0.1:${deadPort}/cached.png`,
  }
  const icons = {
    ok: '/api/icon/unit/TFT99_Unit1', missing: '/api/icon/unit/TFT99_Unit2', refused: '/api/icon/unit/TFT99_Unit4', cached: '/api/icon/unit/TFT99_Unit5',
  }
  const sha1 = (s: string) => createHash('sha1').update(s).digest('hex')
  for (const d of [tsDir, rsDir]) writeFileSync(join(d, 'icons', `${sha1(src.cached)}.png`), 'cached icon bytes')

  // Two working folders: one with no built app, one with a small one (dist/) to serve.
  const bare = join(dir, 'cwd-bare')
  const app = join(dir, 'cwd-app')
  mkdirSync(bare, { recursive: true })
  mkdirSync(join(app, 'dist', 'assets'), { recursive: true })
  writeFileSync(join(app, 'dist', 'index.html'), '<!doctype html><title>Hertz</title><p>café — app</p>')
  writeFileSync(join(app, 'dist', 'assets', 'app.js'), 'console.log("app")')
  writeFileSync(join(app, 'dist', 'assets', 'app.css'), 'body{}')
  writeFileSync(join(app, 'dist', 'favicon.svg'), '<svg/>')
  writeFileSync(join(app, 'dist', 'data.json'), '{"a":1}')
  writeFileSync(join(app, 'dist', 'x.unknownext'), 'x')
  writeFileSync(join(app, 'dist', 'a%20b.txt'), 'percent file')
  writeFileSync(join(app, 'secret.txt'), 'outside dist')

  const tsx = pathToFileURL(join(root, 'node_modules', 'tsx', 'dist', 'loader.mjs')).href
  const serverTs = join(root, 'src', 'server', 'index.ts')
  const both = async (cwd: string, phase: 'bare' | 'full', viaEnv: boolean) => {
    const [tsPort, rsPort] = [await freePort(), await freePort()]
    const ts = await startServer('TypeScript', process.execPath, ['--import', tsx, serverTs], cwd, { DB_PATH: tsDb, PORT: String(tsPort) }, tsPort)
    let rs: Awaited<ReturnType<typeof startServer>>
    try {
      // The Rust server takes its port from PORT like the TypeScript one, or from --port.
      rs = await startServer('Rust', bin, viaEnv ? ['--db', rsDb, 'serve'] : ['--db', rsDb, 'serve', '--port', String(rsPort)], cwd,
        viaEnv ? { PORT: String(rsPort) } : { PORT: '1' }, rsPort)
    } catch (e) {
      await stop(ts.child)
      return fail('tft serve', `    ${(e as Error).message.split('\n').slice(0, 3).join('\n    ')}`)
    }
    try {
      const view = new Database(tsDb, { readonly: true })
      const checks = apiChecks(view, phase, icons)
      view.close()
      await compareUrls(tsPort, rsPort, checks)
      const line = `Hertz running at http://127.0.0.1:${rsPort}`
      if (rs.log().includes(line) && ts.log().includes(`Hertz running at http://127.0.0.1:${tsPort}`)) ok(`both servers print "${line.replace(String(rsPort), 'PORT')}"`)
      else fail('startup line', `    TS: ${ts.log().trim()}\n    Rust: ${rs.log().trim()}`)
    } finally {
      await stop(ts.child)
      await stop(rs.child)
    }
  }

  console.log('  No static data, no built app:')
  await both(bare, 'bare', true)

  // Static data (names, costs, icons, recipes) and your lists, the same in both copies.
  const seededAt = Date.now()
  for (const path of [tsDb, rsDb]) {
    const db = new Database(path)
    const put = db.prepare('INSERT OR REPLACE INTO static_data (kind, id, name, icon, data) VALUES (?, ?, ?, ?, ?)')
    const names: Record<number, string> = { 11: 'Ünit 11', 12: 'unit 12', 13: 'Unit-13', 14: 'Unit 14' }
    for (let i = 0; i < 20; i++) {
      if (i === 3) continue // left out: its name comes from its id
      const icon = i === 0 ? null : i === 1 ? src.ok : i === 2 ? src.missing : i === 5 ? src.cached : src.refused
      const data: Record<string, unknown> = {
        cost: i === 7 ? undefined : i === 8 ? '3' : 1 + Math.floor(i / 4),
        traits: i === 9 ? [] : ['Brawler', i % 2 ? 'Shared' : 'Loner'],
        role: i % 3 ? 'Attack Carry' : null,
        ability: i === 10 ? null : { name: 'Big Hit', text: `Deal ${100 + i}/150/900 damage.\nStun.` },
        stats: { hp: 800 + i, mana: 60, attackSpeed: 0.75, range: i % 5 },
      }
      put.run('unit', `TFT99_Unit${i}`, names[i] ?? `Unit ${i}`, icon, JSON.stringify(data))
    }
    put.run('unit', 'TFT99_kogmaw', "Kog'Maw", src.refused, JSON.stringify({ cost: 2, traits: ['Sniper'] }))
    put.run('unit', 'TFT99_Ekko', 'Ekko', null, JSON.stringify({ cost: 6, traits: ['Shared'] }))
    put.run('unit', 'TFT99_Dummy', 'Target Dummy', null, '{}')
    for (const [id, name] of [['TFT99_Brawler', 'Brawler'], ['TFT99_Invoker', 'Invoker'], ['TFT99_Sniper', 'Sniper'], ['TFT99_Vanguard', 'Vanguard'],
      ['TFT99_Shared', 'Shared'], ['TFT99_Loner', 'Loner'], ['TFT99_Bilgewater', 'Bílgewater'], ['TFT99_bastion', 'bastion']]) {
      put.run('trait', id, name, id === 'TFT99_Brawler' ? src.ok : null, JSON.stringify(id === 'TFT99_Loner' ? {} : { breakpoints: [{ units: 2, style: 1 }, { units: 4, style: 3 }] }))
    }
    for (let i = 0; i < 9; i++) {
      const id = i === 5 ? 'tft_item_thing5' : `TFT_Item_Thing${i}`
      const data: Record<string, unknown> = {
        composition: i === 0 ? ['TFT_Item_Thing1', 'TFT_Item_Thing2'] : i === 1 ? ['TFT_Item_BFSword', 'TFT_Item_ChainVest'] : i === 6 ? undefined : [],
        type: i === 4 ? undefined : i < 2 ? 'craftable' : 'other',
        text: i === 2 ? undefined : `Thing ${i}: +${i * 10}% éclat`,
      }
      put.run('item', id, `Thing ${i}`, i === 5 ? src.ok : i === 3 ? '' : src.refused, JSON.stringify(data))
    }
    put.run('item', 'TFT_Item_Spatula', 'Spatula', null, JSON.stringify({ composition: [], type: 'component', text: '' }))
    const comps = (db.prepare("SELECT comp_key FROM stat_comps WHERE patch = '99.3' AND bucket = 'ALL' ORDER BY comp_key").all() as any[]).map((r) => r.comp_key)
    const mark = db.prepare('INSERT INTO comp_marks (comp_key, fav, hidden) VALUES (?, ?, ?)')
    mark.run(comps[0], 1, 0)
    mark.run(comps[1], 0, 1)
    mark.run(comps[2], 1, 1)
    mark.run('Gone|TFT99_Unit1', 0, 0)
    db.prepare("INSERT INTO kv (key, value) VALUES ('pinned', ?)").run(JSON.stringify(comps[0]))
    const team = db.prepare('INSERT INTO saved_teams (id, name, board, created) VALUES (?, ?, ?, ?)')
    team.run(1, 'First', JSON.stringify([{ unit: 'TFT99_Unit1', hex: 3, items: ['TFT_Item_Thing1'] }]), 1_700_000_000_000)
    team.run(2, 'Newest — team', JSON.stringify([]), 1_800_000_000_000)
    team.run(3, 'Middle', JSON.stringify({ odd: true, n: 1.5 }), 1_750_000_000_000)
    seedEdges(db, seededAt)
    db.close()
  }
  console.log('  Static data, your lists, icons and a built app:')
  const hitsBefore = originHits
  await both(app, 'full', false)
  // Each server downloads the icon it has not cached once (the second request is served from disk) and asks for
  // the missing one once: four requests in all, two per server.
  if (originHits - hitsBefore === 4) ok('icons: each server downloaded once, then served from its cache')
  else fail('icon cache', `    the icon origin saw ${originHits - hitsBefore} requests, expected 4`)
  await new Promise<void>((r) => origin.close(() => r()))
}

try {
  console.log('Building the Rust engine...')
  execFileSync('cargo', ['build', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit' })

  // ---- 1. Ingest -------------------------------------------------------------------------
  console.log('\nIngest: same files through both engines')
  const files = [...rankedMatches, ...awkwardMatches()]
  const matchFile = join(dir, 'matches.json')
  writeFileSync(matchFile, JSON.stringify(files))

  const tsPath = join(dir, 'ts.db')
  const rsPath = join(dir, 'rust.db')
  // Rust creates its own database from its own copy of the schema.
  await run(bin, ['--db', rsPath, 'init'], {})
  const rs = new Database(rsPath)
  rs.prepare('INSERT INTO kv (key, value) VALUES (?, ?)').run('patch_calendar', JSON.stringify(CALENDAR))
  rs.close()
  await run(bin, ['--db', rsPath, 'ingest', matchFile, '--platform', 'na1', '--tier', 'GRANDMASTER'], {})

  process.env.DB_PATH = tsPath
  const { openDb, kvSet } = await import('../src/db')
  const { loadCalendar } = await import('../src/patches')
  const { makeIngest } = await import('../src/crawler/ingest')
  const ts = openDb(tsPath)
  kvSet(ts, 'patch_calendar', CALENDAR)
  loadCalendar(ts)
  const ingest = makeIngest(ts)
  for (const m of files) ingest(m, 'na1', 8, 'GRANDMASTER', true)

  const rust = new Database(rsPath, { readonly: true })
  const schema = (db: Database.Database) =>
    JSON.stringify(db.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name").all())
  if (schema(ts) === schema(rust)) ok('schema identical')
  else fail('schema', '    sqlite_master differs between the two databases')
  const version = (db: Database.Database) => db.pragma('user_version', { simple: true })
  if (version(ts) === version(rust)) ok(`stats version ${version(ts)}`)
  else fail('user_version', `    TS ${version(ts)}, Rust ${version(rust)}`)
  for (const t of ['matches', 'participants', 'units', 'unit_items', 'traits', 'players', 'kv']) compareOrdered(ts, rust, t)
  ts.close()
  rust.close()

  // ---- 2. Stats --------------------------------------------------------------------------
  console.log('\nStats: the same database through both engines\' stats rebuild')
  const PATCHES = 4 // the fixture has three patches; one more checks that a short list is handled
  const tsStats = join(dir, 'ts-stats.db')
  const rsStats = join(dir, 'rust-stats.db')
  copyFileSync(tsPath, tsStats)
  copyFileSync(tsPath, rsStats)
  // Every fixture match is rank 8 and an hour old, which would give all buckets and both windows the same
  // input. Spread ranks over 0..9 and move a third of the matches 4 to 10 days back, the same in both copies.
  const DAY = 86_400_000
  for (const path of [tsStats, rsStats]) {
    const db = new Database(path)
    db.prepare(`UPDATE matches SET rank = rowid % 10,
      game_datetime = CASE WHEN rowid % 3 = 0 THEN game_datetime - (4 + rowid % 7) * ? ELSE game_datetime END`).run(DAY)
    db.close()
  }
  const { buildStats } = await import('../src/stats/build')
  const { STAT_TABLES } = await import('../src/db')
  const tsS = openDb(tsStats)
  buildStats(tsS, PATCHES, () => {})
  tsS.close()
  try {
    await run(bin, ['--db', rsStats, 'stats', '--patches', String(PATCHES)], {})
  } catch (e) {
    fail('tft stats', `    ${(e as Error).message.split('\n').slice(0, 3).join('\n    ')}`)
  }
  const sa = new Database(tsStats, { readonly: true })
  const sb = new Database(rsStats, { readonly: true })
  for (const t of STAT_TABLES) compareStats(sa, sb, t)
  // The comparison only means something if the buckets and the window really narrow the input.
  const counts = (scope: string) =>
    (sa.prepare("SELECT matches FROM stat_meta WHERE patch = '99.3' AND bucket = ?").get(scope) as { matches: number } | undefined)?.matches ?? 0
  const whole = ['ALL', 'PLATINUM+', 'DIAMOND+', 'MASTER+'].map(counts)
  const recent = ['ALL@3d', 'PLATINUM+@3d', 'DIAMOND+@3d', 'MASTER+@3d'].map(counts)
  const label = `patch 99.3 matches per bucket ${whole.join(' > ')}, last 3 days ${recent.join(' > ')}`
  const decreasing = (xs: number[]) => xs.every((x, i) => i === 0 || x < xs[i - 1]) && xs[xs.length - 1] > 0
  if (decreasing(whole) && decreasing(recent) && whole.every((w, i) => recent[i] < w)) ok(label)
  else fail('buckets and window', `    ${label}: each bucket must have fewer matches than the one before, and each 3-day count fewer than its whole patch`)
  sa.close()
  sb.close()

  // ---- 3. Crawl --------------------------------------------------------------------------
  console.log('\nCrawl: both crawlers against the same fake Riot API')
  const crawl = async (label: string, dbPath: string, cmd: string, args: string[]) => {
    const fake = await startFakeRiot()
    const env = {
      RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${fake.port}/{host}`, DB_PATH: dbPath,
      PLATFORMS: 'na1,euw1', TIERS: 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND', PAGES_PER_DIVISION: '2', SNOWBALL: 'true',
    }
    const t0 = Date.now()
    try {
      await run(cmd, args, env)
    } finally {
      await fake.close()
    }
    console.log(`  ${label}: ${fake.seen.total} requests in ${((Date.now() - t0) / 1000).toFixed(1)} s, ` +
      `${fake.seen.overLimit} over the limit, recovered from ${fake.seen.forced429} forced 429 and ${fake.seen.forced500} server errors`)
    if (fake.seen.overLimit > 0) fail(`${label} rate limit`, `    sent ${fake.seen.overLimit} requests over the announced limit`)
  }
  const tsCrawl = join(dir, 'ts-crawl.db')
  const rsCrawl = join(dir, 'rust-crawl.db')
  await crawl('TypeScript', tsCrawl, process.execPath, ['--import', 'tsx', 'src/crawler/crawl.ts', '--once'])
  await crawl('Rust', rsCrawl, bin, ['crawl', '--once'])

  const a = new Database(tsCrawl, { readonly: true })
  const b = new Database(rsCrawl, { readonly: true })
  compareContent(a, b, 'matches', 'SELECT * FROM matches')
  compareContent(a, b, 'participants', 'SELECT match_id, puuid, placement, level, last_round, gold_left, damage, comp_key FROM participants')
  const byBoard = (cols: string, table: string) =>
    `SELECT p.match_id, p.puuid, ${cols} FROM ${table} x JOIN participants p ON p.id = x.participant_id`
  compareContent(a, b, 'units', byBoard('x.character_id, x.star, x.n_items, x.build, x.rarity', 'units'))
  compareContent(a, b, 'unit_items', byBoard('x.character_id, x.item', 'unit_items'))
  compareContent(a, b, 'traits', byBoard('x.name, x.num_units, x.tier_current, x.style', 'traits'))
  compareContent(a, b, 'players', 'SELECT puuid, platform, tier, rank FROM players')
  compareContent(a, b, 'match_queue', 'SELECT match_id, route, platform, seed_rank, status FROM match_queue')
  a.close()
  b.close()

  // A second Rust run must find nothing new.
  const fake = await startFakeRiot()
  try {
    await run(bin, ['crawl', '--once'], {
      RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${fake.port}/{host}`, DB_PATH: rsCrawl,
      PLATFORMS: 'na1,euw1', TIERS: 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND', PAGES_PER_DIVISION: '2',
    })
  } finally {
    await fake.close()
  }
  if (fake.seen.total === 0) ok('a second Rust crawl made no requests')
  else fail('second crawl', `    made ${fake.seen.total} requests`)

  // ---- 4. API ----------------------------------------------------------------------------
  console.log('\nAPI: the same URLs against the TypeScript API and tft serve')
  await apiParity(tsStats, rsStats)
} finally {
  rmSync(dir, { recursive: true, force: true })
}

console.log(failures ? `\n${failures} difference(s). The Rust engine does not match yet.` : '\nParity: the Rust engine matches the TypeScript engine.')
process.exitCode = failures ? 1 : 0
