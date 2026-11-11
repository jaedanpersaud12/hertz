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
 *
 * Exits non-zero on any difference. API routes join this check as they are ported.
 */
import { execFileSync, spawn } from 'node:child_process'
import { copyFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Database from 'better-sqlite3'
import { otherModeMatches, rankedMatches, startFakeRiot } from '../test/world'

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
    // Other game modes (E12): stored with their queue id, either spelling; no queue at all is stored as NULL.
    mk('EDGE_DOUBLEUP', (m) => { m.info.queue_id = 1160 }),
    mk('EDGE_HYPER_CAMEL', (m) => { delete m.info.queue_id; m.info.queueId = 1130 }),
    mk('EDGE_NOQUEUE', (m) => { delete m.info.queue_id }),
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

try {
  console.log('Building the Rust engine...')
  execFileSync('cargo', ['build', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit' })

  // ---- 1. Ingest -------------------------------------------------------------------------
  console.log('\nIngest: same files through both engines')
  const files = [...rankedMatches, ...otherModeMatches, ...awkwardMatches()]
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
} finally {
  rmSync(dir, { recursive: true, force: true })
}

console.log(failures ? `\n${failures} difference(s). The Rust engine does not match yet.` : '\nParity: the Rust engine matches the TypeScript engine.')
process.exitCode = failures ? 1 : 0
