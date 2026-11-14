/**
 * Stats build speed check: a full rebuild at real-data scale must stay fast.
 *
 *   npm run perf
 *
 * Fills a temporary database with synthetic matches (`tft synth`: 12,500 matches, 100,000 boards, one patch),
 * then times `tft stats --patches 1`, a full rebuild of all 8 scopes, with the release build.
 *
 * The limit is 90 seconds. On the 2-core cloud machine the gate runs on, the build takes about 26 seconds; the
 * build it replaced, which read the raw rows once per scope and counted pairs, teammates, trait sets and swaps
 * with SQL self-joins, took about 130. So the limit fails a change that brings either back, with room for a
 * slow machine. PERF_LIMIT_S overrides it.
 *
 * Exits non-zero when the build is over the limit or does not finish.
 */
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const MATCHES = 12_500
const limit = Number(process.env.PERF_LIMIT_S ?? 90)

const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'release', process.platform === 'win32' ? 'tft.exe' : 'tft')
const dir = mkdtempSync(join(tmpdir(), 'tft-perf-'))
const env = { ...process.env, DB_PATH: join(dir, 'tft.db'), RIOT_API_KEY: '', RIOT_ID: '' }

const run = (args: string[]) => {
  const t0 = Date.now()
  const r = spawnSync(bin, args, { cwd: dir, env, encoding: 'utf8' })
  return { ok: r.status === 0, out: `${r.stdout ?? ''}${r.stderr ?? ''}`, seconds: (Date.now() - t0) / 1000 }
}

let failed = false
try {
  const build = spawnSync('cargo', ['build', '--release', '-q', '-p', 'tft-cli'], {
    cwd: root, stdio: 'inherit', shell: process.platform === 'win32',
  })
  if (build.status !== 0) throw new Error('cargo build --release failed')

  const synth = run(['synth', '--matches', String(MATCHES)])
  if (!synth.ok) throw new Error(`tft synth failed:\n${synth.out}`)

  const stats = run(['stats', '--patches', '1'])
  if (!stats.ok) throw new Error(`tft stats failed:\n${stats.out}`)
  const scopes = stats.out.split('\n').filter((l) => l.startsWith('patch '))
  const boards = Number(/(\d+) comps/.exec(scopes[0] ?? '')?.[1] ?? 0)
  if (scopes.length !== 8 || boards !== MATCHES * 8) {
    throw new Error(`expected 8 scopes and ${MATCHES * 8} boards in the first:\n${stats.out}`)
  }

  const verdict = stats.seconds <= limit ? 'ok  ' : 'FAIL'
  console.log(`  ${verdict} full stats rebuild of ${boards} boards, 8 scopes: ${stats.seconds.toFixed(1)} s (limit ${limit} s)`)
  if (stats.seconds > limit) {
    failed = true
    console.log(`\n${stats.out}\nRun "tft stats --patches 1 --profile" on a synthetic database ("tft synth") to see the slow statements.`)
  }
} catch (e) {
  failed = true
  console.error(String(e instanceof Error ? e.message : e))
} finally {
  rmSync(dir, { recursive: true, force: true })
}
process.exit(failed ? 1 : 0)
