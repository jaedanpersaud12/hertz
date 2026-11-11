/**
 * End-to-end check of the tft engine against a fake Riot API and a fake Community Dragon running on this machine.
 * Covers: ladder seeding, rate limiting, retries, match storage, comp grouping, stats maths and the app's API.
 *
 *   npm test
 */
import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Database from 'better-sqlite3'

import { compareHash, pickForCompare, readCompare } from '../src/ui/compare'
import { filterQuery, NO_FILTERS, readFilters, writeFilters } from '../src/ui/tierFilters'
import { ARCHETYPES, idsByPlayer, ITEMS, matches, otherModeMatches, rankedMatches, startFakeCdragon, startFakeRiot, UNITS, archetypeOf } from './world'

const root = join(import.meta.dirname, '..')
const bin = join(root, 'target', 'debug', process.platform === 'win32' ? 'tft.exe' : 'tft')
execFileSync('cargo', ['build', '-q', '-p', 'tft-cli'], { cwd: root, stdio: 'inherit' })

/** Runs `tft <args>` and resolves with what it printed. */
const run = (args: string[], env: Record<string, string>) =>
  new Promise<string>((resolve, reject) => {
    const child = spawn(bin, args, { env: { ...process.env, ...env } })
    let out = ''
    child.stdout.on('data', (d) => (out += d))
    child.stderr.on('data', (d) => (out += d))
    child.on('exit', (code) => (code === 0 ? resolve(out) : reject(new Error(`tft ${args.join(' ')} exited ${code}\n${out}`))))
  })

const dir = mkdtempSync(join(tmpdir(), 'tft-local-test-'))
const dbPath = process.env.TEST_KEEP_DB ?? join(dir, 'tft.db') // set TEST_KEEP_DB to keep the fixture database for a look around
const fake = await startFakeRiot()
const cdragon = await startFakeCdragon()
const { seen } = fake
const mockPort = fake.port
const env = {
  NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost', CDRAGON_BASE: cdragon.base,
  RIOT_API_KEY: 'TEST-KEY', RIOT_API_BASE: `http://127.0.0.1:${mockPort}/{host}`, DB_PATH: dbPath,
  PLATFORMS: 'na1,euw1', TIERS: 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND', PAGES_PER_DIVISION: '2', SNOWBALL: 'true', PORT: '5298',
  RIOT_ID: 'Tester#NA1', MY_PLATFORM: 'na1',
}
const ok = (label: string) => console.log(`  ok  ${label}`)
const near = (a: number, b: number, label: string) => assert.ok(Math.abs(a - b) < 1e-9, `${label}: ${a} vs ${b}`)

try {
  // 0. A database made by the first version of this project gains the new columns
  {
    const oldPath = join(dir, 'old.db')
    const old = new Database(oldPath)
    old.exec(`
      CREATE TABLE participants (id INTEGER PRIMARY KEY, match_id TEXT NOT NULL, puuid TEXT NOT NULL, placement INTEGER NOT NULL, level INTEGER, last_round INTEGER, gold_left INTEGER, damage INTEGER);
      CREATE TABLE units (participant_id INTEGER NOT NULL, character_id TEXT NOT NULL, star INTEGER NOT NULL, n_items INTEGER NOT NULL, build TEXT);
      CREATE TABLE traits (participant_id INTEGER NOT NULL, name TEXT NOT NULL, num_units INTEGER NOT NULL, tier_current INTEGER NOT NULL, style INTEGER NOT NULL);
      CREATE TABLE stat_units (patch TEXT, bucket TEXT, character_id TEXT, games INTEGER);
      INSERT INTO participants VALUES (1, 'M1', 'p', 3, 8, 30, 0, 50);
      INSERT INTO units VALUES (1,'A',2,3,'x|y|z'), (1,'B',2,0,NULL), (1,'C',1,0,NULL), (1,'D',2,1,NULL);
      INSERT INTO traits VALUES (1,'Big',6,3,4), (1,'Small',2,1,1);`)
    old.close()
    await run(['recluster'], { ...env, DB_PATH: oldPath })
    const upgraded = new Database(oldPath)
    assert.equal((upgraded.prepare('SELECT comp_key FROM participants').get() as any).comp_key, 'Big|A')
    assert.equal((upgraded.prepare("SELECT COUNT(*) n FROM pragma_table_info('stat_units') WHERE name = 'avg_star'").get() as any).n, 1)
    upgraded.close()
    ok('an older database is upgraded in place and its boards get comp labels')
  }

  // 1. Crawl
  console.log('Crawling the fake API...')
  const t0 = Date.now()
  await run(['crawl', '--once'], env)
  console.log(`  took ${((Date.now() - t0) / 1000).toFixed(1)} s, ${seen.total} requests`)

  const db = new Database(dbPath)
  const count = (sql: string) => (db.prepare(sql).get() as { n: number }).n
  // Every game mode is stored (E12), each with its queue id; only ranked ones reach the stats below.
  assert.equal(count('SELECT COUNT(*) n FROM matches'), matches.size)
  assert.equal(count('SELECT COUNT(*) n FROM matches WHERE queue_id = 1100'), rankedMatches.length)
  for (const m of otherModeMatches) {
    const want = m.info.queue_id ?? m.info.queueId
    assert.equal((db.prepare('SELECT queue_id FROM matches WHERE match_id = ?').get(m.metadata.match_id) as any).queue_id, want)
  }
  assert.ok(otherModeMatches.some((m) => m.info.queue_id === undefined), 'the world has a match that spells it queueId')
  ok(`stored all ${matches.size} matches with their queue: ${rankedMatches.length} ranked, ${otherModeMatches.length} other modes`)
  assert.equal(count('SELECT COUNT(*) n FROM participants'), matches.size * 8)
  assert.equal(count('SELECT COUNT(*) n FROM players'), 300)
  ok('seeded 300 players from apex and paged ladders')
  assert.equal(seen.overLimit, 0, `client broke the rate limit ${seen.overLimit} times`)
  ok('never exceeded the rate limit the server announced')
  assert.ok(seen.forced429 === 1 && seen.forced500 > 0)
  ok(`recovered from a forced 429 and ${seen.forced500} server errors`)
  assert.equal(count("SELECT COUNT(*) n FROM matches WHERE patch = '99.3'"), matches.size)
  ok('parsed the patch number')

  // 2. Crawl again: nothing new should be fetched or duplicated
  const before = seen.total
  await run(['crawl', '--once'], env)
  assert.equal(count('SELECT COUNT(*) n FROM matches'), matches.size)
  assert.equal(seen.total, before)
  ok('a second run made no requests and stored no duplicates')

  // 3. Stats, checked against a straight recount of the fixtures
  await run(['stats'], env)
  const stat = (sql: string, ...a: unknown[]) => db.prepare(sql).get(...a) as any
  const boards = rankedMatches.flatMap((m) => m.info.participants.map((p: any) => ({ ...p, arch: archetypeOf.get(`${m.metadata.match_id}|${p.puuid}`)! })))
  const avg = (bs: any[]) => bs.reduce((s, b) => s + b.placement, 0) / bs.length

  for (const unit of [UNITS[0], UNITS[19]]) {
    const with_ = boards.filter((b: any) => b.units.some((u: any) => u.character_id === unit.id))
    const row = stat("SELECT * FROM stat_units WHERE bucket = 'ALL' AND character_id = ?", unit.id)
    assert.equal(row.games, with_.length)
    near(row.sum_place / row.games, avg(with_), 'unit avg place')
    assert.equal(row.wins, with_.filter((b: any) => b.placement === 1).length)
  }
  ok('unit games, average place and wins match a recount')

  const carry = UNITS[9].id // the three-star reroll carry
  const threeStar = boards.filter((b: any) => b.units.some((u: any) => u.character_id === carry && u.tier === 3))
  const distRow = stat("SELECT SUM(games) games, SUM(sum_place) sum_place FROM stat_unit_dist WHERE bucket='ALL' AND character_id=? AND star=3", carry)
  assert.equal(distRow.games, threeStar.length)
  near(distRow.sum_place / distRow.games, avg(threeStar), 'three-star avg place')
  ok('star-level breakdown matches a recount')

  const trait = stat("SELECT * FROM stat_traits WHERE bucket='ALL' AND name='TFT99_Shared' AND tier_current = 0")
  assert.equal(trait.games, boards.filter((b: any) => b.traits.find((t: any) => t.name === 'TFT99_Shared').tier_current > 0).length)
  ok('trait counts include only active traits')

  const u = UNITS[17].id, it = ITEMS[9].id
  const unitBoards = boards.filter((b: any) => b.units.some((x: any) => x.character_id === u))
  const pairBoards = unitBoards.filter((b: any) => b.units.some((x: any) => x.character_id === u && x.itemNames.includes(it)))
  const pair = stat("SELECT * FROM stat_unit_items WHERE bucket='ALL' AND character_id=? AND item=?", u, it)
  assert.equal(pair.games, pairBoards.length)
  near(pair.place_change, avg(pairBoards) - avg(unitBoards), 'place change')
  ok('unit + item place change matches a recount')
  const best = stat("SELECT place_change c FROM stat_items WHERE bucket='ALL' AND item=?", ITEMS[9].id).c
  const worst = stat("SELECT place_change c FROM stat_items WHERE bucket='ALL' AND item=?", ITEMS[0].id).c
  assert.ok(best < 0 && worst > 0)
  ok(`the good item shows a negative place change (${best.toFixed(2)}), the bad one positive (+${worst.toFixed(2)})`)

  // Comps: five archetypes went in, five comps should come out, with the right boards in each
  const compRows = db.prepare("SELECT * FROM stat_comps WHERE bucket='ALL' ORDER BY games DESC").all() as any[]
  assert.equal(compRows.length, ARCHETYPES.length, `expected ${ARCHETYPES.length} comps, got ${compRows.map((c) => `${c.comp_key}:${c.games}`).join(', ')}`)
  assert.equal(compRows.reduce((n, c) => n + c.games, 0), boards.length)
  for (const [i, arch] of ARCHETYPES.entries()) {
    const mine = boards.filter((b: any) => b.arch === i)
    const row = compRows.find((c) => c.comp_key.startsWith(arch.trait + '|'))
    assert.equal(row.games, mine.length)
    near(row.sum_place / row.games, avg(mine), 'comp avg place')
  }
  ok(`boards group into ${ARCHETYPES.length} comps, split labels merged, games and average place match a recount`)

  // Comp page sections (PRD 2 C5 to C7), recounted from the fixture boards of one comp. Its carry (unit 17) always
  // holds three items, so every one of its boards is a carry board.
  {
    const key = compRows.find((c) => c.comp_key.startsWith('TFT99_Vanguard|')).comp_key
    const mine = boards.filter((b: any) => b.arch === 0)
    const sum = (bs: any[]) => bs.reduce((n, b) => n + b.placement, 0)
    for (const [band, lo, hi] of [[1, 1, 2], [2, 3, 4], [3, 5, 8]]) {
      const bs = mine.filter((b: any) => b.placement >= lo && b.placement <= hi)
      const row = stat("SELECT SUM(games) games, SUM(sum_place) sp, SUM(sum_round) sr FROM stat_comp_level_bands WHERE bucket='ALL' AND comp_key=? AND band=?", key, band)
      assert.equal(row.games, bs.length); assert.equal(row.sp, sum(bs)); assert.equal(row.sr, bs.reduce((n: number, b: any) => n + b.last_round, 0))
    }
    const carry = UNITS[17].id
    const has = (b: any, id: string) => b.units.some((u: any) => u.character_id === id)
    const full = mine.filter((b: any) => b.units.some((u: any) => u.character_id === carry && u.itemNames.length === 3))
    assert.equal(full.length, mine.length)
    const cu = stat("SELECT * FROM stat_comp_carry_units WHERE bucket='ALL' AND comp_key=? AND character_id=?", key, carry)
    assert.equal(cu.full_games, full.length); assert.equal(cu.full_sum_place, sum(full)); assert.equal(cu.full_top4, full.filter((b: any) => b.placement <= 4).length)
    const builds = stat("SELECT SUM(games) games, COUNT(*) n FROM stat_comp_builds WHERE bucket='ALL' AND comp_key=? AND character_id=?", key, carry)
    assert.equal(builds.games, full.length, 'each board has one build on the carry')
    const built = new Set(full.map((b: any) => [...b.units.find((u: any) => u.character_id === carry).itemNames].sort().join('|')))
    assert.equal(builds.n, built.size)
    const withItem = full.filter((b: any) => b.units.some((u: any) => u.character_id === carry && u.itemNames.includes(ITEMS[9].id)))
    assert.equal(stat("SELECT games FROM stat_comp_carry_items WHERE bucket='ALL' AND comp_key=? AND character_id=? AND item=?", key, carry, ITEMS[9].id).games, withItem.length)
    const holder = UNITS[12].id // a 4-cost unit next to a 5-cost carry
    const held = full.filter((b: any) => has(b, holder))
    const h = stat("SELECT * FROM stat_comp_holders WHERE bucket='ALL' AND comp_key=? AND carry=? AND holder=?", key, carry, holder)
    assert.equal(h.games, held.length); assert.equal(h.sum_place, sum(held)); assert.equal(h.top4, held.filter((b: any) => b.placement <= 4).length)
    assert.equal(stat("SELECT COUNT(*) n FROM stat_comp_holders WHERE bucket='ALL' AND carry=? AND holder=?", carry, UNITS[16].id).n, 0, 'same cost is not a holder')
    assert.equal(stat("SELECT COUNT(*) n FROM stat_comp_special").n, 0, 'the fixture has no emblems, artifacts or radiants')
  }
  ok('comp page sections: levelling bands, carry builds and items, and item holders match a recount')

  // Flex (PRD 2 C8), recounted for every comp from the fixture boards with the cores worked out here: units on
  // half the comp's boards, and for "add at 9" half its level 8 boards when it has 10 of them (else the core).
  {
    let adds = 0, swaps = 0
    for (const [i, arch] of ARCHETYPES.entries()) {
      const key = compRows.find((c) => c.comp_key.startsWith(arch.trait + '|')).comp_key
      const mine = boards.filter((b: any) => b.arch === i)
      const on = (b: any) => new Set<string>(b.units.map((u: any) => u.character_id))
      const ids = [...new Set(mine.flatMap((b: any) => [...on(b)]))] as string[]
      const coreOf = (bs: any[]) => ids.filter((id) => bs.filter((b) => on(b).has(id)).length * 2 >= bs.length)
      const core = coreOf(mine)
      const l8 = mine.filter((b: any) => b.level === 8)
      const core8 = l8.length >= 10 ? coreOf(l8) : core
      const tally = (m: Map<string, [number, number]>, k: string, b: any) => { const t = m.get(k) ?? [0, 0]; m.set(k, [t[0] + 1, t[1] + b.placement]) }
      const wantAdds = new Map<string, [number, number]>(), wantSwaps = new Map<string, [number, number]>()
      for (const b of mine) {
        const u = on(b)
        if (b.level >= 9) for (const id of u) if (!core8.includes(id)) tally(wantAdds, id, b)
        for (const out of core) if (!u.has(out)) for (const id of u) if (!core.includes(id)) tally(wantSwaps, `${out}>${id}`, b)
      }
      const got = (sql: string) => new Map((db.prepare(sql).all(key) as any[]).map((r) => [r.k, [r.games, r.sum_place]]))
      assert.deepEqual(got("SELECT character_id k, games, sum_place FROM stat_comp_flex_adds WHERE bucket='ALL' AND comp_key=?"), wantAdds, `adds of ${key}`)
      assert.deepEqual(got("SELECT out_unit || '>' || in_unit k, games, sum_place FROM stat_comp_swaps WHERE bucket='ALL' AND comp_key=?"), wantSwaps, `swaps of ${key}`)
      adds += wantAdds.size; swaps += wantSwaps.size
    }
    assert.ok(adds > 0 && swaps > 20, `${adds} adds, ${swaps} swaps: too few to prove much`)
  }
  ok('flex: units added at 9 and swaps for core units match a recount for every comp')

  const master = stat("SELECT matches FROM stat_meta WHERE bucket='MASTER+'").matches
  const all = stat("SELECT matches FROM stat_meta WHERE bucket='ALL'").matches
  assert.ok(master > 0 && master < all)
  assert.equal(stat("SELECT matches FROM stat_meta WHERE bucket='ALL@3d'").matches, all)
  assert.equal(all, rankedMatches.length, 'stats count ranked matches only')
  assert.equal(stat("SELECT comps FROM stat_meta WHERE bucket='ALL'").comps, rankedMatches.length * 8)
  ok(`rank and time filters work (${master} of ${all} matches are Master+)`)

  // Riot's real match data has no usable version, so patches are dated from a calendar.
  // The calendar comes from Community Dragon's listing (the fake one lists 16.21, latest, 16.22 and pbe);
  // tft ingest dates each match by it. The same cases run as unit tests in tft-core (patches.rs).
  {
    const calPath = join(dir, 'calendar.db')
    const bare = join(dir, 'no-calendar.db')
    await run(['refresh'], { ...env, DB_PATH: calPath })
    const cal = JSON.parse((new Database(calPath).prepare("SELECT value FROM kv WHERE key = 'patch_calendar'").get() as any).value)
    assert.deepEqual(cal.map((p: any) => p.patch), ['16.21', '16.22'])
    const at = (id: string, version: string, when: number) => {
      const m = structuredClone(rankedMatches[0])
      m.metadata.match_id = id
      m.info.game_version = version
      m.info.game_datetime = when
      return m
    }
    const file = join(dir, 'versions.json')
    writeFileSync(file, JSON.stringify([
      at('CAL_1110', 'TFT Unreal Version ?.?.?.?', Date.parse('2026-11-10T18:00:00Z')),
      at('CAL_1103', 'TFT Unreal Version ?.?.?.?', Date.parse('2026-11-03T18:00:00Z')),
      at('CAL_0901', 'TFT Unreal Version ?.?.?.?', Date.parse('2026-09-01T00:00:00Z')),
      at('CAL_159', 'Linux Version 15.9.1 [PUBLIC] <Releases/15.9>', 0),
    ]))
    await run(['ingest', file, '--no-snowball'], { ...env, DB_PATH: calPath })
    await run(['ingest', file, '--no-snowball'], { ...env, DB_PATH: bare })
    const patchOf = (path: string, id: string) => (new Database(path, { readonly: true }).prepare('SELECT patch FROM matches WHERE match_id = ?').get(id) as any).patch
    assert.equal(patchOf(calPath, 'CAL_1110'), '16.22')
    assert.equal(patchOf(calPath, 'CAL_1103'), '16.21')
    assert.equal(patchOf(calPath, 'CAL_0901'), '16.21')
    assert.equal(patchOf(bare, 'CAL_1110'), 'unknown')
    assert.equal(patchOf(calPath, 'CAL_159'), '15.9')
    ok('matches without a version are dated to a patch from the calendar')
  }

  // 4. Static data
  // tft refresh downloads the set data from the fake Community Dragon (test/world.ts, fakeSetData: sets 98 and 99).
  // The text cleaning and item type cases run as unit tests in tft-riot (cdragon.rs, stored_rows_are_the_typescript_ones).
  await run(['refresh'], env)
  assert.equal(stat("SELECT COUNT(*) n FROM static_data WHERE kind='unit' AND id LIKE 'TFT99_%'").n, UNITS.length)
  assert.equal(stat("SELECT icon FROM static_data WHERE kind='unit' AND id='TFT99_Unit19'").icon,
    'https://raw.communitydragon.org/latest/game/assets/characters/tft99_unit19/hud/tft99_unit19_square.tft_set99.png')
  assert.equal(JSON.parse(stat("SELECT data FROM static_data WHERE kind='unit' AND id='TFT99_Unit9'").data).ability.text, 'Deal 100/150/900 damage.\nStun for 1 seconds.')
  assert.deepEqual(ITEMS.map((i) => JSON.parse(stat("SELECT data FROM static_data WHERE kind='item' AND id=?", i.id).data).type),
    ITEMS.map((_, n) => (n < 6 ? 'craftable' : 'other')))
  ok('static data picks the newest set, builds icon URLs, cleans ability text and sorts items into types')
  db.close()

  // 5. The app's API
  const api = spawn(bin, ['serve', '--dev'], { env: { ...process.env, ...env } }) // PORT + 1, as behind Vite
  let apiOut = ''
  api.stdout.on('data', (d) => (apiOut += d)); api.stderr.on('data', (d) => (apiOut += d))
  try {
    const call = async (p: string, init?: RequestInit) => {
      for (let i = 0; i < 50; i++) {
        try { const r = await fetch(`http://127.0.0.1:5299${p}`, init); return { status: r.status, body: r.status === 200 ? await r.json() as any : null } }
        catch { await new Promise((r) => setTimeout(r, 200)) }
      }
      throw new Error('API did not start\n' + apiOut)
    }
    const get = async (p: string) => (await call(p)).body
    const post = async (p: string, body: unknown) => (await call(p, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })).body

    const stat2 = (sql: string, ...a: unknown[]) => { const d = new Database(dbPath, { readonly: true }); try { return d.prepare(sql).get(...a) as any } finally { d.close() } }
    const meta = await get('/api/meta')
    assert.equal(meta.patch, '99.3'); assert.equal(meta.totals.matches, rankedMatches.length); assert.equal(meta.windows.length, 2)

    const comps = await get('/api/comps')
    assert.equal(comps.rows.length, ARCHETYPES.length)
    const vanguard = comps.rows.find((r: any) => r.id.startsWith('TFT99_Vanguard|'))
    assert.ok(vanguard.name.startsWith('Vanguard Unit 1'), vanguard.name)
    for (const idx of [17, 16]) assert.ok(vanguard.units.some((x: any) => x.id === UNITS[idx].id), `core unit ${idx} missing`)
    assert.equal(vanguard.units.find((x: any) => x.id === UNITS[17].id).items.length, 3)
    assert.equal(vanguard.traits[0].name, 'Vanguard'); assert.equal(vanguard.traits[0].units, 6)
    assert.ok(!vanguard.traits.some((t: any) => t.name === 'Loner') || vanguard.traits.length > 1)
    assert.equal(vanguard.play.label, 'Fast 8')
    assert.equal(comps.rows.find((r: any) => r.id.startsWith('TFT99_Sniper|')).play.label, 'Fast 9')
    assert.equal(comps.rows.find((r: any) => r.id.startsWith('TFT99_Brawler|')).play.label, 'Reroll lvl 7')
    near(comps.rows.reduce((n: number, r: any) => n + r.pickRate, 0), 8, 'pick rates add up to 8 boards per lobby')
    const detail = await get(`/api/comps/${encodeURIComponent(vanguard.id)}`)
    assert.equal(detail.levels.length, 1); assert.equal(detail.levels[0].level, 8); assert.ok(detail.builds[0].items.length > 3)
    ok('comps: names, core units, carry items, traits, playstyle tags, levels')
    assert.deepEqual(detail.levelling.map((b: any) => [b.label, b.commonLevel]), [['1st–2nd', 8], ['3rd–4th', 8], ['5th–8th', 8]])
    assert.equal(detail.levelling.reduce((n: number, b: any) => n + b.games, 0), vanguard.games)
    assert.equal(detail.carries[0].unit.id, UNITS[17].id); assert.ok(detail.carries[0].items.length > 0)
    assert.ok(detail.carries[0].items.every((i: any, n: number, xs: any[]) => i.games >= 10 && (n === 0 || xs[n - 1].avgPlace <= i.avgPlace)))
    const holders = detail.holders[0].units
    assert.ok(holders.some((u: any) => u.id === UNITS[12].id && u.sharedTraits.includes('Vanguard')))
    assert.ok(holders.every((u: any) => (u.cost ?? 0) < 5), 'holders cost less than the carry')
    assert.deepEqual(detail.special, [])
    ok('comp page: levelling by placement band, carry items, item holders, special items')

    // Flex on the comp page (C8): 10 games or more, the baseline too; place change against it.
    const vSwaps = detail.swaps as any[]
    assert.ok(vSwaps.length > 0 && vSwaps.length <= 8, `${vSwaps.length} swaps`)
    for (const sw of vSwaps) {
      assert.ok(sw.games >= 10 && sw.baseGames >= 10)
      const row = stat2("SELECT games, sum_place FROM stat_comp_swaps WHERE bucket='ALL' AND comp_key=? AND out_unit=? AND in_unit=?", vanguard.id, sw.out.id, sw.in.id)
      const base = stat2("SELECT games, sum_place FROM stat_comp_units WHERE bucket='ALL' AND comp_key=? AND character_id=?", vanguard.id, sw.out.id)
      near(sw.placeChange, row.sum_place / row.games - base.sum_place / base.games, 'swap place change')
      near(sw.freq, row.games / (vanguard.games - base.games), 'swap share of boards without the core unit')
      assert.ok(vanguard.units.some((u: any) => u.id === sw.out.id), 'the unit swapped out is on the comp board')
    }
    assert.ok(vSwaps.every((x, n) => n === 0 || vSwaps[n - 1].games >= x.games), 'most played first')
    assert.deepEqual(detail.addAt9, [], 'every Vanguard board is level 8')
    const sniper = comps.rows.find((r: any) => r.id.startsWith('TFT99_Sniper|'))
    const sDetail = await get(`/api/comps/${encodeURIComponent(sniper.id)}`)
    // Sniper boards are all level 9 with no level 8 ones, so its core is the baseline: its flex units are added at 9.
    assert.ok(sDetail.addAt9.length > 0 && sDetail.addAt9.length <= 6, `${sDetail.addAt9.length} adds`)
    for (const a of sDetail.addAt9) {
      assert.ok(a.games >= 10 && a.baseGames >= 10 && !sniper.units.some((u: any) => u.id === a.unit.id))
      const row = stat2("SELECT games, sum_place FROM stat_comp_flex_adds WHERE bucket='ALL' AND comp_key=? AND character_id=?", sniper.id, a.unit.id)
      const nine = stat2("SELECT SUM(games) games, SUM(sum_place) sum_place FROM stat_comp_levels WHERE bucket='ALL' AND comp_key=? AND level >= 9", sniper.id)
      near(a.placeChange, row.sum_place / row.games - (nine.sum_place - row.sum_place) / (nine.games - row.games), 'add at 9 place change')
      near(a.freq, row.games / nine.games, 'share of level 9 boards')
    }
    ok('comp page: swap options with place change against the core unit, add at 9 with its minimums')

    // Your name for a comp (PRD 2 C14): shown in the list at once, gone again when the override is deleted.
    const path = `/api/comps/${encodeURIComponent(vanguard.id)}`
    assert.equal((await post(`${path}/rename`, { name: 'My frontline' })).override.name, 'My frontline')
    assert.equal((await get('/api/comps')).rows.find((r: any) => r.id === vanguard.id).name, 'My frontline')
    assert.equal((await call(`${path}/override`, { method: 'DELETE' })).status, 200)
    assert.equal((await get('/api/comps')).rows.find((r: any) => r.id === vanguard.id).name, vanguard.name)
    assert.equal((await get('/api/settings/comps')).method, 'baseline')
    ok('comps: rename shows in the list and can be undone; the comp method defaults to the baseline')

    // Tier list (PRD 2 C3): every comp graded by the formula in config/tier-list.json, worked out again here.
    const cfg = JSON.parse(readFileSync(join(root, 'config', 'tier-list.json'), 'utf8'))
    const all = (await get('/api/comps?min=1')).rows as any[]
    const z = (xs: number[]) => {
      const m = xs.reduce((a, b) => a + b, 0) / xs.length
      const sd = Math.sqrt(xs.reduce((a, b) => a + (b - m) ** 2, 0) / xs.length)
      return xs.map((x) => (sd > 1e-12 ? (x - m) / sd : 0))
    }
    const [zp, zt, zw] = [z(all.map((r) => r.avgPlace)), z(all.map((r) => r.top4Rate)), z(all.map((r) => r.winRate))]
    const finals = all.map((r, i) => (cfg.weights.avgPlace * -zp[i] + cfg.weights.top4Rate * zt[i] + cfg.weights.winRate * zw[i]) * (r.games / (r.games + cfg.confidenceGames)))
    all.forEach((r, i) => near(r.score, finals[i], `score of ${r.id}`))
    const ranked = all.map((r, i) => ({ r, s: finals[i] })).sort((a, b) => b.s - a.s || b.r.games - a.r.games || (a.r.id < b.r.id ? -1 : 1))
    let run = 0
    const lines = cfg.tiers.map((t: any, i: number) => ({ tier: t.tier, line: i === cfg.tiers.length - 1 ? ranked.length : Math.floor((ranked.length * (run += t.percent) + 50) / 100) }))
    ranked.forEach(({ r }, rank) => assert.equal(r.tier, lines.find((l: any) => rank < l.line).tier, `tier of ${r.id} at rank ${rank}`))
    assert.deepEqual(ranked.map(({ r }) => r.tier), ['A', 'B', 'B', 'C', 'D'], 'five comps: 0.4, 1.25, 2.75, 4.25 round to 0, 1, 3, 4')
    ok(`tier letters: score from avg place, top 4 and win rate with a games confidence term, cut-offs from config (${ranked.map(({ r }) => r.tier).join('')})`)

    // Trends (C4): the last 3 days against the 3-day rows; this world has one patch, so nothing to compare with last patch.
    const days = (await get('/api/comps?min=1&window=3d')).rows as any[]
    for (const r of all) {
      const d = days.find((x) => x.id === r.id)
      near(r.d3Place, d.avgPlace, `last 3 days of ${r.id}`); near(r.d3Delta, d.avgPlace - r.avgPlace, `3-day delta of ${r.id}`)
      assert.equal(r.prevPlace, null); assert.equal(r.patchDelta, null)
      assert.equal(r.rising, r.d3Delta <= -cfg.risingPlaces + 1e-9)
      assert.equal(d.d3Place, null, 'the 3-day rows do not compare with themselves')
    }
    ok('trend: last patch and last 3 days on each row, rising as defined in config')

    // Filters (C11): each one narrows to the comps that have it, and they combine.
    const { options } = await get('/api/comps?min=1')
    const has = (r: any, q: Record<string, string>) =>
      (!q.unit || [...r.units, ...r.carries].some((u: any) => u.id === q.unit)) &&
      (!q.trait || r.trait?.id === q.trait || r.traits.some((t: any) => t.id === q.trait)) &&
      (!q.item || r.units.some((u: any) => u.items.some((i: any) => i.id === q.item))) &&
      (!q.playstyle || r.play.kind === q.playstyle) && (!q.rising || r.rising)
    const ids = (rows: any[]) => rows.map((r) => r.id).sort()
    const asks: Record<string, string>[] = [
      { unit: options.units[0].id }, { trait: options.traits[0].id }, { item: options.items[0].id }, { playstyle: options.playstyles[0] },
      { rising: '1' }, { unit: vanguard.carries[0].id, playstyle: vanguard.play.kind }, { unit: vanguard.carries[0].id, playstyle: 'nope' },
      { trait: vanguard.trait.id, item: vanguard.units.find((u: any) => u.items.length).items[0].id, unit: vanguard.units[0].id },
    ]
    let narrowed = 0
    for (const q of asks) {
      const got = await get(`/api/comps?min=1&${new URLSearchParams(q)}`)
      assert.deepEqual(ids(got.rows), ids(all.filter((r) => has(r, q))), JSON.stringify(q))
      assert.deepEqual(got.options, options, 'the choices do not shrink with the filters')
      if (got.rows.length < all.length) narrowed++
    }
    assert.ok(narrowed >= 5, `the filters should narrow the list (${narrowed})`)
    assert.equal((await get(`/api/comps?min=1&unit=${vanguard.carries[0].id}&playstyle=nope`)).rows.length, 0)
    assert.deepEqual(ids((await get('/api/comps?min=1&unit=&trait=&rising=0')).rows), ids(all), 'empty filters filter nothing')
    assert.ok(options.units.length >= 5 && options.traits.length >= 5 && options.items.length > 0 && options.playstyles.length >= 2)
    ok('filters by unit, trait, item, playstyle and rising narrow the list, and combine')

    // The URL holds the filters (src/ui/tierFilters.ts), next to anything else in it.
    const picked = { ...NO_FILTERS, unit: 'TFT99_Unit1', trait: 'TFT99_Sniper', rising: true }
    const search = writeFilters('?data=worst', picked)
    assert.equal(search, '?data=worst&unit=TFT99_Unit1&trait=TFT99_Sniper&rising=1')
    assert.deepEqual(readFilters(search), picked)
    assert.equal(writeFilters(search, NO_FILTERS), '?data=worst')
    assert.equal(writeFilters('', NO_FILTERS), '')
    assert.equal(filterQuery({ ...NO_FILTERS, item: 'A|B', playstyle: 'fast' }), 'item=A%7CB&playstyle=fast')
    assert.deepEqual(readFilters('?unit=a&rising=yes'), { ...NO_FILTERS, unit: 'a' })
    ok('filter state round-trips through the page URL')

    // The dev stress switch covers the new fields.
    const worst = await get('/api/comps?data=worst')
    assert.deepEqual(worst.rows.slice(0, 3).map((r: any) => [r.tier, r.patchDelta]), [['S', -3.21], ['D', 3.4], [null, null]])
    assert.deepEqual((await get('/api/comps?data=empty')).rows, [])
    assert.ok((await get('/api/comps?data=empty')).options.units.length > 0)
    assert.equal((await get('/api/comps?data=one')).rows.length, 1)
    assert.equal((await get('/api/comps?data=many&unit=' + vanguard.carries[0].id)).rows.length, 1000)
    ok('stress modes cover tier letters, trends and filters')

    // Playstyle (C10) from the data: reroll, Fast 8 and Fast 9 are kinds of their own, and filterable.
    const kind = (trait: string) => comps.rows.find((r: any) => r.id.startsWith(`${trait}|`)).play.kind
    assert.deepEqual(['TFT99_Vanguard', 'TFT99_Sniper', 'TFT99_Brawler'].map(kind), ['fast8', 'fast9', 'reroll'])
    assert.ok(['fast8', 'fast9', 'reroll'].every((k) => options.playstyles.includes(k)), options.playstyles.join())
    assert.deepEqual(ids((await get('/api/comps?min=1&playstyle=fast9')).rows), ids(all.filter((r) => r.play.label === 'Fast 9')))
    ok('playstyle tags: reroll, Fast 8 and Fast 9 from the average level and the carry, each a filter')

    // Difficulty (C10): yours, set and cleared over HTTP, on each row, and a filter (with "none" for untagged).
    const setDifficulty = (id: string, level: unknown) => call(`/api/comps/${encodeURIComponent(id)}/difficulty`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ level }) })
    const hard = await setDifficulty(vanguard.id, 'hard')
    assert.equal(hard.status, 200); assert.deepEqual(hard.body.difficulty, { [vanguard.id]: 'hard' })
    assert.deepEqual((await get('/api/lists')).difficulty, { [vanguard.id]: 'hard' })
    const tagged = (await get('/api/comps?min=1&difficulty=hard')).rows
    assert.deepEqual(ids(tagged), [vanguard.id]); assert.equal(tagged[0].difficulty, 'hard')
    assert.deepEqual(ids((await get('/api/comps?min=1&difficulty=none')).rows), ids(all.filter((r) => r.id !== vanguard.id)))
    assert.equal((await get(`/api/comps?min=1&difficulty=hard&playstyle=fast9`)).rows.length, 0, 'combines with the other filters')
    assert.equal((await setDifficulty(vanguard.id, 'impossible')).status, 400)
    assert.equal((await setDifficulty(vanguard.id, 3)).status, 400)
    assert.deepEqual((await get('/api/lists')).difficulty, { [vanguard.id]: 'hard' }, 'a refused level changes nothing')
    assert.deepEqual((await setDifficulty(vanguard.id, null)).body.difficulty, {})
    assert.deepEqual((await get('/api/comps?min=1&difficulty=hard')).rows, [])
    assert.ok((await get('/api/comps?min=1')).rows.every((r: any) => r.difficulty === null))
    assert.equal(writeFilters('?data=worst', { ...NO_FILTERS, difficulty: 'hard' }), '?data=worst&difficulty=hard')
    assert.equal(readFilters('?difficulty=none').difficulty, 'none')
    assert.equal((await get('/api/comps?data=worst')).rows[0].difficulty, 'hard', 'stress gives rows a difficulty')
    ok('difficulty: set, filtered on and cleared, kept by comp id, refused when it is not easy, medium or hard')

    // Compare (C12): the page URL holds both ids, and picking two from the tier list opens it.
    assert.equal(compareHash('A|B', 'C/D'), '#/compare/A%7CB/C%2FD')
    assert.deepEqual(readCompare(compareHash('A|B', 'C/D')), ['A|B', 'C/D'])
    assert.deepEqual(readCompare('#/compare'), ['', '']); assert.deepEqual(readCompare('#/compare/A'), ['A', ''])
    assert.equal(readCompare('#/comps'), null); assert.deepEqual(readCompare('#/compare/%E0%A4%A/x'), ['%E0%A4%A', 'x'])
    assert.deepEqual(pickForCompare(null, 'A'), { waiting: 'A', open: null })
    assert.deepEqual(pickForCompare('A', 'A'), { waiting: null, open: null }, 'picking the same comp again drops it')
    assert.deepEqual(pickForCompare('A', 'B|C'), { waiting: null, open: '#/compare/A/B%7CC' })
    const [left, right] = readCompare(compareHash(vanguard.id, sniper.id))!
    assert.deepEqual([(await get(`/api/comps/${encodeURIComponent(left)}`)).comp.id, (await get(`/api/comps/${encodeURIComponent(right)}`)).comp.id], [vanguard.id, sniper.id])
    ok('compare: both comp ids round-trip through the URL and load')

    // Stress modes reach the flex lists.
    const sid = encodeURIComponent(sniper.id)
    const sw = await get(`/api/comps/${sid}?data=worst`)
    assert.equal(sw.addAt9.length, 6); assert.equal(sw.swaps.length, 8); assert.equal(sw.swaps[0].games, 12_345_678)
    assert.deepEqual([(await get(`/api/comps/${sid}?data=empty`)).addAt9, (await get(`/api/comps/${sid}?data=empty`)).swaps], [[], []])
    assert.equal((await get(`/api/comps/${sid}?data=one`)).swaps.length, 1)
    ok('stress modes cover add at 9 and swaps')

    const units = await get('/api/units')
    assert.equal(units.rows.length, 20)
    const unitDetail = await get(`/api/units/${UNITS[9].id}?min=1`)
    assert.equal(unitDetail.unit.ability.text, 'Deal 100/150/900 damage.\nStun for 1 seconds.')
    near(unitDetail.stars.reduce((n: number, x: any) => n + x.share, 0), 1, 'star shares')
    near(unitDetail.itemCounts.reduce((n: number, x: any) => n + x.share, 0), 1, 'item-count shares')
    assert.ok(unitDetail.items.length > 0 && unitDetail.builds.length > 0 && unitDetail.comps.length > 0 && unitDetail.trend.length === 1)
    ok('unit detail: ability, star and item-count breakdowns, items, builds, comps, trend')

    const traits = await get('/api/traits?bucket=MASTER%2B&window=3d')
    assert.equal(traits.bucket, 'MASTER+'); assert.equal(traits.window, '3d'); assert.equal(traits.rows.length, 7)
    const items = await get('/api/items')
    assert.equal(items.rows.length, 10); assert.ok(items.rows[0].holders.length > 0)
    assert.equal(items.rows.find((r: any) => r.id === ITEMS[0].id).type, 'craftable')
    const itemDetail = await get(`/api/items/${ITEMS[9].id}?min=1`)
    assert.ok(itemDetail.holders.length > 0); assert.equal(itemDetail.item.name, 'Thing 9')
    assert.equal((await get(`/api/items/${ITEMS[0].id}`)).recipe.length, 2)
    ok('traits, items and item detail')

    const pairQ = await get(`/api/explore?units=${UNITS[17].id},${UNITS[16].id}`)
    const both = boards.filter((b: any) => [17, 16].every((i) => b.units.some((x: any) => x.character_id === UNITS[i].id)))
    assert.equal(pairQ.summary.games, both.length); near(pairQ.summary.avgPlace, avg(both), 'explorer avg place')
    assert.ok(pairQ.partners.length > 0 && pairQ.items.length > 0)
    const withTrait = await get(`/api/explore?units=${UNITS[12].id}&traits=TFT99_Sniper`)
    assert.equal(withTrait.summary.games, boards.filter((b: any) => b.arch === 3 && b.units.some((x: any) => x.character_id === UNITS[12].id)).length)
    ok('explorer answers "boards with these units and traits" and matches a recount')

    assert.deepEqual((await post('/api/lists/mark', { key: vanguard.id, fav: true })).favs, [vanguard.id])
    assert.equal((await post('/api/lists/pin', { key: vanguard.id })).pinned, vanguard.id)
    assert.equal((await post('/api/lists/pin', { key: null })).pinned, null)
    const saved = await post('/api/teams', { name: 'Test team', board: [{ id: UNITS[1].id, hex: 3 }] })
    assert.equal(saved.teams[0].name, 'Test team')
    assert.equal((await call(`/api/teams/${saved.teams[0].id}`, { method: 'DELETE' })).body.teams.length, 0)
    assert.equal((await get('/api/static')).units.length, 20)
    ok('favourites, pin, saved teams and static data')

    const me = await get('/api/me')
    assert.equal(me.name, 'Tester'); assert.equal(me.tier, 'CHALLENGER'); assert.equal(me.lp, 512)
    const mine = idsByPlayer.get('na1-chal-0')!.slice(0, 20)
    assert.equal(me.recent.length, Math.min(10, mine.length))
    const history = await get('/api/me/matches')
    assert.equal(history.matches.length, mine.length)
    assert.equal(history.matches[0].lobby.length, 8); assert.ok(history.matches[0].mine.units[0].name.startsWith('Unit '))
    assert.ok(history.matches.some((m: any) => m.queue === 'Normal') || mine.every((id) => matches.get(id).info.queue_id === 1100))
    ok('summoner card and match history')

    const setAccount = (body: unknown) => call('/api/me/account', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    assert.equal((await setAccount({ riotId: 'NoTag', platform: 'na1' })).status, 400)
    assert.equal((await setAccount({ riotId: 'Tester#NA1', platform: 'mars' })).status, 400)
    const missing = (await setAccount({ riotId: "Nobody#XX1", platform: "na1" })).body
    assert.equal(missing.ok, false); assert.match(missing.error, /No Riot account called Nobody#XX1/)
    assert.equal((await get('/api/me')).riotId, 'Nobody#XX1')
    assert.equal((await setAccount({ riotId: "Tester # NA1", platform: "na1" })).body.ok, true)
    const back = await get('/api/me')
    assert.equal(back.riotId, 'Tester#NA1'); assert.equal(back.name, 'Tester'); assert.equal(back.tier, 'CHALLENGER')
    ok('Riot ID and server can be set from the app, with clear errors for typos')

    assert.equal((await call('/api/icon/unit/Nope')).status, 404)
  } finally { api.kill() }

  console.log('\nAll checks passed.')
} finally {
  await fake.close()
  await cdragon.close()
  rmSync(dir, { recursive: true, force: true })
}
