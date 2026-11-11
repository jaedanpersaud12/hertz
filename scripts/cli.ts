/**
 * Command line check (E11): the one `tft` binary runs the whole engine without the app.
 *
 *   npm run cli
 *
 * Runs target/debug/tft against the fake Riot API in test/world.ts:
 * 1. `tft crawl --once` stores matches and builds the stats before it exits.
 * 2. `tft stats --incremental` rebuilds nothing when nothing is new, and only the touched scopes after one
 *    low-rank match is added.
 * 3. `tft health` prints its numbers.
 * 4. `tft run` serves /api/meta while it crawls, and stops on Ctrl+C (SIGINT) within a few seconds.
 * 5. `tft run` without a Riot API key says so and keeps serving.
 *
 * Exits non-zero on any failure.
 */
import { execFileSync, spawn } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createServer, request } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Database from 'better-sqlite3'
import { rankedMatches, startFakeRiot } from '../test/world'

const dir = mkdtempSync(join(tmpdir(), 'tft-cli-'))
const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'debug', process.platform === 'win32' ? 'tft.exe' : 'tft')
let failures = 0
const check = (cond: boolean, label: string, detail = '') => {
  if (cond) return console.log(`  ok  ${label}`)
  failures++
  console.log(`  FAIL ${label}${detail ? `\n    ${detail.split('\n').join('\n    ')}` : ''}`)
}

const baseEnv = (dbPath: string, extra: Record<string, string> = {}) => ({
  ...process.env,
  NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost',
  DB_PATH: dbPath, RIOT_ID: '',
  PLATFORMS: 'na1,euw1', TIERS: 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND', PAGES_PER_DIVISION: '2', SNOWBALL: 'true',
  ...extra,
})

/** Runs the binary to the end; resolves with its exit code and everything it printed. */
const run = (args: string[], env: NodeJS.ProcessEnv) =>
  new Promise<{ code: number | null; out: string }>((resolve) => {
    const c = spawn(bin, args, { cwd: root, env })
    let out = ''
    c.stdout.on('data', (d) => (out += d))
    c.stderr.on('data', (d) => (out += d))
    c.on('exit', (code) => resolve({ code, out }))
  })

const freePort = () =>
  new Promise<number>((resolve) => {
    const s = createServer()
    s.listen(0, '127.0.0.1', () => {
      const port = (s.address() as { port: number }).port
      s.close(() => resolve(port))
    })
  })

const getStatus = (port: number, path: string) =>
  new Promise<number>((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port, path, method: 'GET', agent: false }, (res) => {
      res.resume()
      res.on('end', () => resolve(res.statusCode ?? 0))
    })
    req.on('error', reject)
    req.setTimeout(10_000, () => req.destroy(new Error('timed out')))
    req.end()
  })

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/** Starts `tft run`, waits until /api/meta answers 200. */
async function startRun(port: number, env: NodeJS.ProcessEnv) {
  const child = spawn(bin, ['run', '--port', String(port)], { cwd: root, env })
  const state = { out: '', exited: null as null | { code: number | null; signal: string | null; at: number } }
  child.stdout.on('data', (d) => (state.out += d))
  child.stderr.on('data', (d) => (state.out += d))
  child.on('exit', (code, signal) => (state.exited = { code, signal, at: Date.now() }))
  for (let i = 0; i < 300; i++) {
    if (state.exited) break
    try {
      if ((await getStatus(port, '/api/meta')) === 200) return { child, state, up: true }
    } catch {
      // not listening yet
    }
    await sleep(100)
  }
  return { child, state, up: false }
}

/** Sends SIGINT and resolves with how long the process took to exit (ms), or -1 after 10 s. */
async function interrupt(r: Awaited<ReturnType<typeof startRun>>) {
  const t0 = Date.now()
  r.child.kill('SIGINT')
  for (let i = 0; i < 100 && !r.state.exited; i++) await sleep(100)
  if (!r.state.exited) {
    r.child.kill('SIGKILL')
    return -1
  }
  return r.state.exited.at - t0
}

try {
  console.log('Building the Rust engine...')
  execFileSync('cargo', ['build', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit' })

  // ---- 1. tft crawl --once ---------------------------------------------------------------
  console.log('\ntft crawl --once: crawls until up to date, then builds the stats')
  const db = join(dir, 'crawl.db')
  {
    const fake = await startFakeRiot()
    const r = await run(['crawl', '--once'], baseEnv(db, { RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${fake.port}/{host}` }))
    await fake.close()
    check(r.code === 0, 'exits 0', r.out.slice(-2000))
    const d = new Database(db, { readonly: true })
    const matches = (d.prepare('SELECT COUNT(*) n FROM matches').get() as { n: number }).n
    const meta = (d.prepare('SELECT COUNT(*) n FROM stat_meta').get() as { n: number }).n
    const units = (d.prepare('SELECT COUNT(*) n FROM stat_units').get() as { n: number }).n
    d.close()
    check(matches > 0, `stored ${matches} matches`)
    check(meta > 0 && units > 0, `built the stats (${meta} stat_meta rows, ${units} stat_units rows)`, r.out.slice(-2000))
  }

  // ---- 2. tft stats --incremental --------------------------------------------------------
  console.log('\ntft stats --incremental: rebuilds only the scopes with new matches')
  {
    const env = baseEnv(db, { RIOT_API_KEY: '' })
    const idle = await run(['stats', '--incremental'], env)
    check(idle.code === 0 && idle.out.includes('Stats are up to date: no new matches.') && !idle.out.includes('Rebuilt'),
      'nothing new: nothing rebuilt', idle.out)

    // One low-rank match, played now: only the ALL bucket (whole patch and last 3 days) of its patch changes.
    const m = structuredClone(rankedMatches[0])
    m.metadata.match_id = 'NA1_CLI_1'
    m.info.game_datetime = Date.now()
    const file = join(dir, 'one.json')
    writeFileSync(file, JSON.stringify(m))
    const ing = await run(['ingest', file, '--tier', 'IRON', '--no-snowball'], env)
    check(ing.code === 0 && ing.out.includes('Stored 1 matches'), 'ingested one IRON match', ing.out)

    const inc = await run(['stats', '--incremental'], env)
    const line = inc.out.match(/^Rebuilt (\d+) scopes: (.*)$/m)
    const scopes = line ? line[2].split(', ') : []
    const patches = new Set(scopes.map((s) => s.split('/')[0]))
    const kinds = scopes.map((s) => s.split('/')[1]).sort()
    check(inc.code === 0 && line !== null && Number(line[1]) === scopes.length, `reports the scopes it rebuilt: ${line?.[2] ?? '(none)'}`, inc.out)
    check(patches.size === 1 && kinds.join(',') === 'ALL,ALL@3d', 'only the ALL scopes of one patch', `got ${scopes.join(', ')}`)

    const full = await run(['stats'], env)
    check(full.code === 0 && !full.out.includes('Rebuilt'), 'without --incremental: the full rebuild, as before', full.out)
  }

  // ---- 3. tft health ---------------------------------------------------------------------
  console.log('\ntft health: the numbers in plain lines')
  {
    const h = await run(['health'], baseEnv(db, { RIOT_API_KEY: '' }))
    check(h.code === 0, 'exits 0', h.out)
    check(/^ {2}na1 +\d+ in the last hour, [1-9]\d* in the last 24 hours$/m.test(h.out), 'matches per hour for na1', h.out)
    check(/^ {2}euw1 +\d+ in the last hour, [1-9]\d* in the last 24 hours$/m.test(h.out), 'matches per hour for euw1', h.out)
    check(/^Matches waiting to be fetched: \d+/m.test(h.out), 'queue depth', h.out)
    check(/^Last problem/m.test(h.out), 'last problem line', h.out)
    check(/^Stats built .* \(\d+ minutes old\)\.$/m.test(h.out), 'stats age', h.out)
  }

  // ---- 4. tft run ------------------------------------------------------------------------
  console.log('\ntft run: serves the app while crawling, stops on Ctrl+C')
  {
    const fake = await startFakeRiot()
    const port = await freePort()
    const runDb = join(dir, 'run.db')
    const r = await startRun(port, baseEnv(runDb, { RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${fake.port}/{host}` }))
    check(r.up, '/api/meta answers 200', r.state.out.slice(-2000))
    // Crawling at the same time: the fake Riot API keeps getting requests while the app answers.
    const before = fake.seen.total
    let served = 0
    for (let i = 0; i < 10; i++) {
      if ((await getStatus(port, '/api/meta').catch(() => 0)) === 200) served++
      await sleep(100)
    }
    const during = fake.seen.total - before
    check(served === 10 && during > 0, `served /api/meta 10 times while the crawler sent ${during} requests`, r.state.out.slice(-2000))
    const ms = await interrupt(r)
    await fake.close()
    check(ms >= 0 && ms < 5000, `stopped ${ms >= 0 ? `${ms} ms` : 'not within 10 s'} after SIGINT`, r.state.out.slice(-2000))
    check(r.state.exited?.code === 0 && r.state.out.includes('Stopped.'), 'exited 0 and said so', r.state.out.slice(-2000))
    const gone = await getStatus(port, '/api/meta').then(() => false, () => true)
    check(gone, 'the port is closed afterwards')
    const d = new Database(runDb, { readonly: true })
    const n = (d.prepare('SELECT COUNT(*) n FROM matches').get() as { n: number }).n
    d.close()
    check(n > 0, `the crawl half stored ${n} matches in the same database`)
  }

  // ---- 5. tft run without a key ----------------------------------------------------------
  console.log('\ntft run without a Riot API key: says so and keeps serving')
  {
    const port = await freePort()
    const r = await startRun(port, baseEnv(join(dir, 'nokey.db'), { RIOT_API_KEY: '' }))
    check(r.up, '/api/meta answers 200', r.state.out.slice(-2000))
    await sleep(1500)
    check(!r.state.exited, 'still running after the crawl gave up', r.state.out.slice(-2000))
    check(r.state.out.includes('Your Riot API key is not set.'), 'prints the missing key message', r.state.out)
    check((await getStatus(port, '/api/meta').catch(() => 0)) === 200, 'still serving /api/meta')
    const ms = await interrupt(r)
    check(ms >= 0 && ms < 5000 && r.state.exited?.code === 0, `stopped ${ms} ms after SIGINT`, r.state.out.slice(-2000))
  }
} finally {
  rmSync(dir, { recursive: true, force: true })
}

console.log(failures ? `\n${failures} check(s) failed.` : '\nCommand line: every check passed.')
process.exitCode = failures ? 1 : 0
