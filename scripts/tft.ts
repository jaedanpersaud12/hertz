/**
 * Builds the tft engine (release) if it changed, then runs it with these arguments. Works the same on Windows,
 * macOS and Linux. Used by `npm run dev`, `npm start`, `npm run crawl`, `npm run stats` and `npm run tft -- <args>`.
 *
 *   npm run tft -- health        any tft command; `npm run tft -- --help` lists them
 *
 * Needs Rust (https://rustup.rs). The first build takes a few minutes; later ones only rebuild what changed.
 */
import { spawn, spawnSync } from 'node:child_process'
import { join } from 'node:path'

const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'release', process.platform === 'win32' ? 'tft.exe' : 'tft')
const args = process.argv.slice(2)

const build = spawnSync('cargo', ['build', '--release', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' })
if (build.error) {
  console.error('Could not run cargo. Install Rust from https://rustup.rs, open a new terminal, and try again.')
  process.exit(1)
}
if (build.status !== 0) process.exit(build.status ?? 1)

const child = spawn(bin, args, { stdio: 'inherit' })
// Ctrl+C reaches tft as well, which finishes what is in flight and stops; this script waits for it.
// A stop from outside (concurrently, a terminal closing) is passed on.
process.on('SIGINT', () => {})
for (const sig of ['SIGTERM', 'SIGHUP'] as const) process.on(sig, () => child.kill(sig))
child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)))
