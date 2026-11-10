/**
 * Resume check: the Rust crawler, killed at any point and started again, ends with exactly what one
 * uninterrupted crawl stores. No duplicate rows, no skipped matches or players.
 *
 *   npm run resume
 *
 * `tft` talks to the fake Riot API through a small proxy in this file. The proxy counts requests and, when the
 * request a run is meant to die on arrives, SIGKILLs the crawler before answering it. So every kill lands on a
 * known request, and the script checks where each one landed.
 *
 * Kills on requests can never land between two database writes (all of a batch's downloads finish before any of
 * it is stored), so other runs set `TFT_DIE_AT=<point>:<n>`, a test-only hook in crates/tft-riot/src/crawl.rs that
 * aborts the crawler exactly between two writes: after a ladder page is saved, after a server's ladders are all
 * saved, after a match list is queued, after a match is downloaded but before it is stored, and after its store
 * commits but before it is marked done.
 *
 * 1. Reference: one `tft crawl --once` into a fresh database.
 * 2. Chaos, into another fresh database:
 *    - killed while the ladders are being read, then aborted after a ladder page is saved and after na1's ladders
 *      are all saved, then killed while the other server's ladders are read,
 *    - killed on a match-list request, then aborted right after a match list is queued,
 *    - killed on match downloads chosen from a fixed seed, each while that match is still queued,
 *    - aborted just before a match is stored, and just after its store commits (before it is marked done),
 *    - one run where the answer for one match is always cut off (it must be left queued, and the run must end),
 *    - a last run where that answer is cut off once (it must be fetched again and stored).
 * 3. The two databases are compared row by row, and the chaos database is checked for duplicates and orphans.
 *
 * Exits non-zero on any difference.
 */
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { createServer, request, type IncomingMessage, type ServerResponse } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Database from 'better-sqlite3'
import { startFakeRiot } from '../test/world'

const SEED = 188
const MATCH_KILLS = 6

const dir = mkdtempSync(join(tmpdir(), 'tft-resume-'))
const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'debug', process.platform === 'win32' ? 'tft.exe' : 'tft')
let failures = 0
const ok = (label: string) => console.log(`  ok  ${label}`)
const fail = (label: string, detail: string) => {
  failures++
  console.log(`  FAIL ${label}\n${detail}`)
}
const check = (cond: boolean, label: string, detail = '') => (cond ? ok(label) : fail(label, detail ? `    ${detail}` : ''))

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

// ---- Proxy ---------------------------------------------------------------------------------

type Kind = 'ladder' | 'ids' | 'match' | 'other'
function kindOf(path: string): Kind {
  if (path.includes('/tft/league/v1/')) return 'ladder'
  if (/\/tft\/match\/v1\/matches\/by-puuid\//.test(path)) return 'ids'
  if (/\/tft\/match\/v1\/matches\/[^/]+$/.test(path)) return 'match'
  return 'other'
}
const matchIdOf = (path: string) => path.match(/\/tft\/match\/v1\/matches\/([^/?]+)$/)?.[1]

/** What the proxy does during one run. */
interface Plan {
  /** SIGKILL the crawler when the `nth` request of this kind (counted in this run, from 1) arrives. */
  kill?: { kind: Kind; nth: number }
  /** Cut off the answer for this match: always, or only the first time it is asked for. */
  cutOff?: { matchId?: string; firstMatch?: boolean; times: 'always' | 'once'; among?: Set<string> }
  /** Abort the crawler at an exact point between database writes: `TFT_DIE_AT=<point>:<n>` (test-only hook). */
  dieAt?: string
}

interface RunResult {
  outcome: 'killed' | 'died' | 'finished'
  /** Where `TFT_DIE_AT` aborted it: the point and what it was working on (a match id, a puuid, a server). */
  died?: { point: string; what: string }
  /** The request the crawler was killed on. */
  killedOn?: { kind: Kind; path: string }
  /** Match ids whose answers were cut off in this run. */
  cut: string[]
  out: string
  counts: Record<Kind, number>
}

async function startProxy(fakePort: number) {
  let child: ChildProcess | undefined
  let plan: Plan = {}
  let counts: Record<Kind, number> = { ladder: 0, ids: 0, match: 0, other: 0 }
  let killedOn: RunResult['killedOn']
  let cut: string[] = []
  let cutTarget: string | undefined
  const total = { requests: 0 }

  const forward = (req: IncomingMessage, then: (status: number, headers: Record<string, string>, body: Buffer) => void) => {
    const up = request(
      { host: '127.0.0.1', port: fakePort, path: req.url, method: req.method, headers: { 'x-riot-token': String(req.headers['x-riot-token'] ?? '') } },
      (r) => {
        const chunks: Buffer[] = []
        r.on('data', (c) => chunks.push(c))
        r.on('end', () => {
          const headers: Record<string, string> = {}
          for (const [k, v] of Object.entries(r.headers)) if (typeof v === 'string' && k !== 'content-length' && k !== 'connection') headers[k] = v
          then(r.statusCode ?? 502, headers, Buffer.concat(chunks))
        })
      },
    )
    up.on('error', () => then(502, {}, Buffer.from('{}')))
    up.end()
  }

  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    const path = new URL(req.url!, 'http://x').pathname
    const kind = kindOf(path)
    total.requests++
    counts[kind]++
    if (child && !killedOn && plan.kill && plan.kill.kind === kind && counts[kind] === plan.kill.nth) {
      // Killed before this request is answered: whatever it was fetching is in flight and must not be lost.
      killedOn = { kind, path }
      child.kill('SIGKILL')
      res.socket?.destroy()
      return
    }
    const id = matchIdOf(path)
    if (kind === 'match' && id && plan.cutOff && !cutTarget && plan.cutOff.firstMatch && (plan.cutOff.among?.has(id) ?? true)) cutTarget = id
    const cutThis = kind === 'match' && id !== undefined && plan.cutOff !== undefined && id === (plan.cutOff.matchId ?? cutTarget) &&
      (plan.cutOff.times === 'always' || !cut.includes(id))
    forward(req, (status, headers, body) => {
      if (cutThis && status === 200) {
        // Announce the whole answer, send half of it, hang up: what a laptop going to sleep looks like.
        cut.push(id!)
        res.writeHead(200, { ...headers, 'content-length': String(body.length) })
        res.write(body.subarray(0, Math.floor(body.length / 2)), () => res.socket?.destroy())
        return
      }
      res.writeHead(status, { ...headers, 'content-length': String(body.length) })
      res.end(body)
    })
  })
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r))
  const port = (server.address() as { port: number }).port

  return {
    port,
    total,
    /** Runs `tft crawl --once` through the proxy under `p`. Rejects if the crawler fails on its own. */
    run(dbPath: string, p: Plan = {}) {
      plan = p
      counts = { ladder: 0, ids: 0, match: 0, other: 0 }
      killedOn = undefined
      cut = []
      cutTarget = undefined
      return new Promise<RunResult>((resolve, reject) => {
        const env = { ...envFor(port, dbPath), ...(p.dieAt ? { TFT_DIE_AT: p.dieAt } : {}) }
        const c = spawn(bin, ['crawl', '--once'], { cwd: root, env })
        child = c
        let out = ''
        c.stdout.on('data', (d) => (out += d))
        c.stderr.on('data', (d) => (out += d))
        c.on('exit', (code, signal) => {
          child = undefined
          const result = { killedOn, cut: [...cut], out, counts: { ...counts } }
          const died = out.match(/^TFT_DIE_AT (\S+):\d+ (\S+)$/m)
          if (killedOn && signal === 'SIGKILL') resolve({ outcome: 'killed', ...result })
          else if (p.dieAt && died && signal === 'SIGABRT') resolve({ outcome: 'died', died: { point: died[1], what: died[2] }, ...result })
          else if (code === 0) resolve({ outcome: 'finished', ...result })
          else reject(new Error(`tft crawl exited ${code ?? signal}\n${out}`))
        })
      })
    },
    close: () => new Promise<void>((r) => { server.closeAllConnections(); server.close(() => r()) }),
  }
}

const envFor = (port: number, dbPath: string) => ({
  ...process.env,
  NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost',
  RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${port}/{host}`, DB_PATH: dbPath,
  PLATFORMS: 'na1,euw1', TIERS: 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND', PAGES_PER_DIVISION: '2', SNOWBALL: 'true',
})

// ---- Checks --------------------------------------------------------------------------------

/** Row-by-row comparison that ignores participant ids and row order. */
function compareContent(a: Database.Database, b: Database.Database, label: string, sql: string) {
  const rows = (db: Database.Database) => (db.prepare(sql).raw().all() as unknown[][]).map((r) => JSON.stringify(r)).sort()
  const [ra, rb] = [rows(a), rows(b)]
  const firstDiff = ra.findIndex((r, i) => r !== rb[i])
  if (ra.length === rb.length && firstDiff === -1) return ok(`${label}: ${ra.length} rows identical`)
  const at = firstDiff === -1 ? ra.length : firstDiff
  fail(label, `    uninterrupted ${ra.length} rows, resumed ${rb.length}. First difference at sorted row ${at}:\n` +
    `      uninterrupted: ${ra[at] ?? '(none)'}\n      resumed:       ${rb[at] ?? '(none)'}`)
}

const count = (db: Database.Database, sql: string, ...args: unknown[]) => (db.prepare(sql).get(...args) as { n: number }).n

function none(db: Database.Database, label: string, sql: string) {
  const n = count(db, `SELECT COUNT(*) AS n FROM (${sql})`)
  check(n === 0, `no ${label}`, `${n} found`)
}

const seedPending = (dbPath: string): string[] => {
  const db = new Database(dbPath, { readonly: true })
  try {
    const row = db.prepare("SELECT value FROM kv WHERE key = 'seed_pending'").get() as { value: string } | undefined
    return row ? JSON.parse(row.value) : []
  } finally {
    db.close()
  }
}

function lastCrawled(dbPath: string, puuid: string) {
  const db = new Database(dbPath, { readonly: true })
  try {
    const crawled = (db.prepare('SELECT last_crawled AS n FROM players WHERE puuid = ?').get(puuid) as { n: number } | undefined)?.n ?? 0
    const queuedFor = count(db, 'SELECT COUNT(*) AS n FROM match_queue')
    return { crawled, queuedFor }
  } finally {
    db.close()
  }
}

function queued(dbPath: string, matchId: string) {
  const db = new Database(dbPath, { readonly: true })
  try {
    const status = (db.prepare('SELECT status FROM match_queue WHERE match_id = ?').get(matchId) as { status: number } | undefined)?.status
    const stored = count(db, 'SELECT COUNT(*) AS n FROM matches WHERE match_id = ?', matchId)
    const players = count(db, 'SELECT COUNT(*) AS n FROM players')
    return { status, stored, players }
  } finally {
    db.close()
  }
}

try {
  console.log('Building the Rust engine...')
  execFileSync('cargo', ['build', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit' })

  // ---- 1. Reference ----------------------------------------------------------------------
  const refPath = join(dir, 'reference.db')
  let fake = await startFakeRiot()
  let proxy = await startProxy(fake.port)
  let ref: RunResult
  try {
    ref = await proxy.run(refPath)
  } finally {
    await proxy.close()
    await fake.close()
  }
  const c = ref.counts
  // Matches the uninterrupted crawl stored. The cut-off runs pick one of these that is not stored yet.
  const refDb = new Database(refPath, { readonly: true })
  const refStored = new Set((refDb.prepare('SELECT match_id FROM matches').raw().all() as string[][]).map((r) => r[0]))
  refDb.close()
  console.log(`\nUninterrupted crawl: ${proxy.total.requests} requests (${c.ladder} ladder, ${c.ids} match lists, ${c.match} matches)`)

  // ---- 2. Killed and restarted -----------------------------------------------------------
  const chaosPath = join(dir, 'resumed.db')
  fake = await startFakeRiot()
  proxy = await startProxy(fake.port)
  try {
    // Ladders: na1 is read first. Request 3 arrives after na1's Challenger and Grandmaster lists are saved.
    let r = await proxy.run(chaosPath, { kill: { kind: 'ladder', nth: 3 } })
    let q = queued(chaosPath, '')
    check(r.outcome === 'killed' && r.killedOn?.kind === 'ladder', 'kill 1 lands while the ladders are read', `${r.outcome} on ${r.killedOn?.path}`)
    check(q.players > 0, 'kill 1: some players were saved before it', `${q.players} players`)
    check(seedPending(chaosPath).includes('na1'), 'kill 1: na1 is marked as not read to the end', `seed_pending = ${JSON.stringify(seedPending(chaosPath))}`)

    // Aborted right after the 2nd ladder page of the re-read is saved: na1 must still be marked as not read to the end.
    r = await proxy.run(chaosPath, { dieAt: 'after_ladder:2' })
    check(r.outcome === 'died' && r.died?.point === 'after_ladder' && r.died.what.startsWith('na1/'),
      'abort after a ladder page is saved lands in the na1 ladders', `${r.outcome} at ${r.died?.point} ${r.died?.what}`)
    check(seedPending(chaosPath).includes('na1'), 'after_ladder: na1 is still marked as not read to the end', `seed_pending = ${JSON.stringify(seedPending(chaosPath))}`)

    // Aborted after all of na1's ladders are saved but before na1 leaves seed_pending: it must be read again.
    r = await proxy.run(chaosPath, { dieAt: 'after_seed:1' })
    check(r.outcome === 'died' && r.died?.point === 'after_seed' && r.died.what === 'na1',
      'abort after na1\'s ladders are all saved lands before na1 is marked done', `${r.outcome} at ${r.died?.point} ${r.died?.what}`)
    check(seedPending(chaosPath).includes('na1'), 'after_seed: na1 is still marked, so its ladders are read again', `seed_pending = ${JSON.stringify(seedPending(chaosPath))}`)

    // Restarted: na1 is read again (8 requests), then euw1. Request 15 is in the middle of euw1.
    r = await proxy.run(chaosPath, { kill: { kind: 'ladder', nth: 15 } })
    check(r.outcome === 'killed' && /^\/euw1\//.test(r.killedOn?.path ?? ''), 'kill 2 lands while the euw1 ladders are read', `${r.outcome} on ${r.killedOn?.path}`)
    const pending = seedPending(chaosPath)
    check(!pending.includes('na1') && pending.includes('euw1'), 'kill 2: na1 is done, euw1 is not', `seed_pending = ${JSON.stringify(pending)}`)

    // Match lists.
    r = await proxy.run(chaosPath, { kill: { kind: 'ids', nth: 3 } })
    check(r.outcome === 'killed' && r.killedOn?.kind === 'ids', 'kill 3 lands on a match-list request', `${r.outcome} on ${r.killedOn?.path}`)
    check(seedPending(chaosPath).length === 0, 'kill 3: both ladders were read to the end', `seed_pending = ${JSON.stringify(seedPending(chaosPath))}`)

    // Aborted right after one player's match list is committed: that player is marked as looked at and its matches are
    // queued.
    r = await proxy.run(chaosPath, { dieAt: 'after_enqueue:2' })
    const lc = lastCrawled(chaosPath, r.died?.what ?? '')
    check(r.outcome === 'died' && r.died?.point === 'after_enqueue' && lc.crawled > 0 && lc.queuedFor > 0,
      `abort after a match list is queued lands with ${r.died?.what} marked as looked at`, `${r.outcome} at ${r.died?.point} ${r.died?.what}, last_crawled ${lc.crawled}, queue ${lc.queuedFor}`)

    // Match downloads, each at a seeded point inside the first two batches of the run.
    const next = random(SEED)
    let midMatch = 0
    for (let i = 0; i < MATCH_KILLS; i++) {
      const nth = 1 + Math.floor(next() * 40)
      r = await proxy.run(chaosPath, { kill: { kind: 'match', nth } })
      const id = r.killedOn?.kind === 'match' ? matchIdOf(r.killedOn.path) : undefined
      q = queued(chaosPath, id ?? '')
      const landed = r.outcome === 'killed' && id !== undefined && q.status === 0 && q.stored === 0
      if (landed) midMatch++
      check(landed, `kill ${4 + i} lands on match download ${nth} of the run, with ${id} still queued`,
        `${r.outcome} on ${r.killedOn?.path}, queue status ${q.status}, stored ${q.stored}`)
    }
    check(midMatch > 0, `${midMatch} kills landed mid-match`)

    // Aborted between the database writes for one match: just before its store, and just after its store commits but
    // before it is marked done. The first must leave it queued and unstored; the second stored but still queued, so
    // the next run stores it again, which must add nothing (the duplicate and row-by-row checks below).
    for (const [point, n] of [['before_store', 1], ['after_store', 1], ['after_store', 13], ['before_store', 13], ['after_store', 27], ['before_store', 27]] as const) {
      r = await proxy.run(chaosPath, { dieAt: `${point}:${n}` })
      const id = r.died?.what ?? ''
      q = queued(chaosPath, id)
      const want = point === 'before_store' ? q.status === 0 && q.stored === 0 : q.status === 0 && q.stored === 1
      check(r.outcome === 'died' && r.died?.point === point && want,
        `abort ${point}:${n} lands on ${id} ${point === 'before_store' ? 'still queued and not stored' : 'stored but still queued'}`,
        `${r.outcome} at ${r.died?.point} ${id}, queue status ${q.status}, stored ${q.stored}`)
    }

    // A ranked match not stored yet whose answer is always cut off: left queued, and the run still ends.
    const chaosDb = new Database(chaosPath, { readonly: true })
    const notYet = new Set([...refStored].filter((id) => count(chaosDb, 'SELECT COUNT(*) AS n FROM matches WHERE match_id = ?', id) === 0))
    chaosDb.close()
    r = await proxy.run(chaosPath, { cutOff: { firstMatch: true, times: 'always', among: notYet } })
    const stuck = r.cut[0]
    q = queued(chaosPath, stuck ?? '')
    check(r.outcome === 'finished', 'a run where one answer is always cut off still finishes', r.outcome)
    check(stuck !== undefined && r.cut.length === 3 && r.cut.every((x) => x === stuck), `that answer was asked for 3 times`, `cut off: ${r.cut.join(', ')}`)
    check(q.status === 0 && q.stored === 0, `${stuck} is left queued for the next run`, `queue status ${q.status}, stored ${q.stored}`)
    check(/was cut off\. It will be fetched again\./.test(r.out) && /cut off 3 times\. Leaving it queued for the next run\./.test(r.out),
      'the crawler says the answer was cut off and what happens next', r.out.split('\n').filter((l) => l.includes(stuck ?? '?')).join(' | '))

    // The same answer cut off once: fetched again in the same run and stored.
    r = await proxy.run(chaosPath, { cutOff: { matchId: stuck, times: 'once' } })
    q = queued(chaosPath, stuck ?? '')
    check(r.outcome === 'finished' && r.cut.length === 1 && r.cut[0] === stuck, `the last run cut off ${stuck} once`, `${r.outcome}, cut off: ${r.cut.join(', ')}`)
    check(q.status === 1 && q.stored === 1, `${stuck} was fetched again and stored`, `queue status ${q.status}, stored ${q.stored}`)
  } finally {
    await proxy.close()
    await fake.close()
  }
  console.log(`\nResumed crawl: ${proxy.total.requests} requests in all\n`)

  // ---- 3. Compare ------------------------------------------------------------------------
  const a = new Database(refPath, { readonly: true })
  const b = new Database(chaosPath, { readonly: true })
  check(count(a, 'SELECT COUNT(*) AS n FROM matches') > 0, 'the uninterrupted crawl stored matches')
  check(count(a, 'SELECT COUNT(*) AS n FROM players') > 0, 'the uninterrupted crawl found players')
  none(b, 'duplicate participants', 'SELECT 1 FROM participants GROUP BY match_id, puuid HAVING COUNT(*) > 1')
  none(b, 'participants without a match', 'SELECT 1 FROM participants WHERE match_id NOT IN (SELECT match_id FROM matches)')
  for (const t of ['units', 'unit_items', 'traits']) {
    none(b, `${t} without a participant`, `SELECT 1 FROM ${t} WHERE participant_id NOT IN (SELECT id FROM participants)`)
  }
  const byBoard = (cols: string, table: string) =>
    `SELECT p.match_id, p.puuid, ${cols} FROM ${table} x JOIN participants p ON p.id = x.participant_id`
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
