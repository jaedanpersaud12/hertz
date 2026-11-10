/**
 * Runs the app on made-up matches, so you can look around before you have a Riot key.
 *
 *   npm run demo
 *
 * The first run builds data/demo.db from the same fake matches the tests use (about 30 seconds).
 * Your real database (data/tft.db) is never touched.
 */
import { spawn } from 'node:child_process'
import { existsSync, mkdirSync } from 'node:fs'

const DB = 'data/demo.db'
const run = (cmd: string, env: Record<string, string>) =>
  new Promise<void>((resolve, reject) => {
    const child = spawn(cmd, { shell: true, stdio: 'inherit', env: { ...process.env, ...env } })
    child.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`"${cmd}" exited with ${code}`))))
  })

if (!existsSync(DB)) {
  mkdirSync('data', { recursive: true })
  console.log('Building demo data (one time, about 30 seconds)...')
  await run('npx tsx test/e2e.ts', { TEST_KEEP_DB: DB })
}
console.log('\nDemo data ready. Opening the app on http://127.0.0.1:5174\n')
await run('npm run dev', { DB_PATH: DB, DEMO: '1', RIOT_ID: 'Tester#NA1', MY_PLATFORM: 'na1', RIOT_API_KEY: 'demo' })
