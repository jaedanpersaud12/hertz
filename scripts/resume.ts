/**
 * Resume check: the Rust crawler, killed at any point and started again, ends with exactly what one
 * uninterrupted crawl stores. No duplicate rows, no skipped matches or players.
 *
 *   npm run resume
 *
 * 1. Reference: one `tft crawl --once` against the fake Riot API into a fresh database.
 * 2. Chaos: the same crawl into another fresh database, killed with SIGKILL after a number of requests
 *    drawn from a fixed seed, then started again, until a run finishes on its own.
 * 3. The two databases are compared by content, and the chaos database is checked for duplicates.
 *
 * Exits non-zero on any difference.
 */
import { execFileSync, spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Database from 'better-sqlite3'
import { startFakeRiot } from '../test/world'

const SEED = 188
const KILLS = 8

const dir = mkdtempSync(join(tmpdir(), 'tft-resume-'))
const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'debug', process.platform === 'win32' ? 'tft.exe' : 'tft')
let failures = 0
const ok = (label: string) => console.log(`  ok  ${label}`)
const fail = (label: string, detail: string) => {
  failures++
  console.log(`  FAIL ${label}\n${detail}`)
}

/** Small seeded generator (mulberry32), so the kill points are the same on every run. */
function random(seed: number) {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const envFor = (port: number, dbPath: string) => ({
  ...process.env,
  NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost',
  RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${port}/{host}`, DB_PATH: dbPath,
  PLATFORMS: 'na1,euw1', TIERS: 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND', PAGES_PER_DIVISION: '2', SNOWBALL: 'true',
})

type Fake = Awaited<ReturnType<typeof startFakeRiot>>

/**
 * Runs `tft crawl --once`. With `killAt`, sends SIGKILL once the fake API has seen that many requests in
 * total. Resolves to 'killed' or 'finished'; rejects if the crawler fails on its own.
 */
function crawl(fake: Fake, dbPath: string, killAt?: number) {
  return new Promise<'killed' | 'finished'>((resolve, reject) => {
    const child = spawn(bin, ['crawl', '--once'], { cwd: root, env: envFor(fake.port, dbPath) })
    let out = ''
    let killed = false
    child.stdout.on('data', (d) => (out += d))
    child.stderr.on('data', (d) => (out += d))
    const watch = killAt === undefined ? undefined : setInterval(() => {
      if (!killed && fake.seen.total >= killAt) {
        killed = true
        child.kill('SIGKILL')
      }
    }, 2)
    child.on('exit', (code, signal) => {
      if (watch) clearInterval(watch)
      if (killed && signal === 'SIGKILL') resolve('killed')
      else if (code === 0) resolve('finished')
      else reject(new Error(`tft crawl exited ${code ?? signal}\n${out}`))
    })
  })
}

/** Content comparison that ignores participant ids and row order. */
function compareContent(a: Database.Database, b: Database.Database, label: string, sql: string) {
  const rows = (db: Database.Database) => (db.prepare(sql).raw().all() as unknown[][]).map((r) => JSON.stringify(r)).sort()
  const [ra, rb] = [rows(a), rows(b)]
  const inB = new Set(rb)
  const inA = new Set(ra)
  const onlyA = ra.filter((r) => !inB.has(r))
  const onlyB = rb.filter((r) => !inA.has(r))
  if (!onlyA.length && !onlyB.length && ra.length === rb.length) return ok(`${label}: ${ra.length} rows identical`)
  fail(label, `    uninterrupted ${ra.length}, resumed ${rb.length}. Only uninterrupted: ${onlyA.slice(0, 2).join(' ')} Only resumed: ${onlyB.slice(0, 2).join(' ')}`)
}

function noDuplicates(db: Database.Database, label: string, sql: string) {
  const n = (db.prepare(`SELECT COUNT(*) AS n FROM (${sql} HAVING COUNT(*) > 1)`).get() as { n: number }).n
  if (n === 0) ok(`no duplicate ${label}`)
  else fail(`duplicate ${label}`, `    ${n} repeated`)
}

try {
  console.log('Building the Rust engine...')
  execFileSync('cargo', ['build', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit' })

  // ---- 1. Reference ----------------------------------------------------------------------
  const refPath = join(dir, 'reference.db')
  let fake = await startFakeRiot()
  try {
    await crawl(fake, refPath)
  } finally {
    await fake.close()
  }
  const total = fake.seen.total
  console.log(`\nUninterrupted crawl: ${total} requests`)

  // ---- 2. Killed and restarted -----------------------------------------------------------
  // Kill points spread over the whole crawl, from the ladders to the last match.
  const next = random(SEED)
  // The first one lands while the ladders are still being read.
  const points = [3, ...Array.from({ length: KILLS }, () => 1 + Math.floor(next() * total))].sort((x, y) => x - y)
  console.log(`Killing after request ${points.join(', ')}`)
  const chaosPath = join(dir, 'resumed.db')
  fake = await startFakeRiot()
  let kills = 0
  try {
    for (const at of points) {
      if ((await crawl(fake, chaosPath, at)) === 'killed') kills++
      else break // finished before reaching this point
    }
    let runs = 0
    while ((await crawl(fake, chaosPath)) !== 'finished') if (++runs > 3) throw new Error('crawl never finished')
  } finally {
    await fake.close()
  }
  console.log(`Resumed crawl: killed ${kills} times, ${fake.seen.total} requests in all\n`)
  if (kills >= KILLS / 2) ok(`killed mid-crawl ${kills} times`)
  else fail('kills', `    only ${kills} of ${KILLS} kills landed before the crawl finished`)

  // ---- 3. Compare ------------------------------------------------------------------------
  const a = new Database(refPath, { readonly: true })
  const b = new Database(chaosPath, { readonly: true })
  noDuplicates(b, 'participants', 'SELECT match_id, puuid FROM participants GROUP BY match_id, puuid')
  const byBoard = (cols: string, table: string) =>
    `SELECT p.match_id, p.puuid, ${cols} FROM ${table} x JOIN participants p ON p.id = x.participant_id`
  noDuplicates(b, 'units', `${byBoard('x.character_id, x.star, x.n_items, x.build, x.rarity', 'units')} GROUP BY p.id, x.rowid`)
  noDuplicates(b, 'orphan rows', 'SELECT participant_id FROM units WHERE participant_id NOT IN (SELECT id FROM participants) GROUP BY participant_id, rowid')
  noDuplicates(b, 'participants without a match', 'SELECT match_id FROM participants WHERE match_id NOT IN (SELECT match_id FROM matches) GROUP BY match_id, id')
  compareContent(a, b, 'matches', 'SELECT * FROM matches')
  compareContent(a, b, 'participants', 'SELECT match_id, puuid, placement, level, last_round, gold_left, damage, comp_key FROM participants')
  compareContent(a, b, 'units', byBoard('x.character_id, x.star, x.n_items, x.build, x.rarity', 'units'))
  compareContent(a, b, 'unit_items', byBoard('x.character_id, x.item', 'unit_items'))
  compareContent(a, b, 'traits', byBoard('x.name, x.num_units, x.tier_current, x.style', 'traits'))
  compareContent(a, b, 'players', 'SELECT puuid, platform, tier, rank FROM players')
  compareContent(a, b, 'match_queue', 'SELECT match_id, route, platform, seed_rank, status FROM match_queue')
  a.close()
  b.close()
} finally {
  rmSync(dir, { recursive: true, force: true })
}

console.log(failures ? `\n${failures} problem(s). The crawler does not resume cleanly.` : '\nResume: a killed crawl picks up exactly where it stopped.')
process.exitCode = failures ? 1 : 0
