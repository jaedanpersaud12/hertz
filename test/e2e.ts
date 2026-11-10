/**
 * End-to-end check against a fake Riot API running on this machine.
 * Covers: ladder seeding, rate limiting, retries, match storage, comp grouping, stats maths and the app's API.
 *
 *   npm test
 */
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Database from 'better-sqlite3'

import { ARCHETYPES, idsByPlayer, ITEMS, matches, rankedMatches, startFakeRiot, UNITS, archetypeOf } from './world'

const run = (script: string, args: string[], env: Record<string, string>) =>
  new Promise<string>((resolve, reject) => {
    const child = spawn(process.execPath, ['--import', 'tsx', script, ...args], { env: { ...process.env, ...env } })
    let out = ''
    child.stdout.on('data', (d) => (out += d))
    child.stderr.on('data', (d) => (out += d))
    child.on('exit', (code) => (code === 0 ? resolve(out) : reject(new Error(`${script} exited ${code}\n${out}`))))
  })

const dir = mkdtempSync(join(tmpdir(), 'tft-local-test-'))
const dbPath = process.env.TEST_KEEP_DB ?? join(dir, 'tft.db') // set TEST_KEEP_DB to keep the fixture database for a look around
const fake = await startFakeRiot()
const { seen } = fake
const mockPort = fake.port
const env = {
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
    await run('src/stats/recluster.ts', [], { ...env, DB_PATH: oldPath })
    const upgraded = new Database(oldPath)
    assert.equal((upgraded.prepare('SELECT comp_key FROM participants').get() as any).comp_key, 'Big|A')
    assert.equal((upgraded.prepare("SELECT COUNT(*) n FROM pragma_table_info('stat_units') WHERE name = 'avg_star'").get() as any).n, 1)
    upgraded.close()
    ok('an older database is upgraded in place and its boards get comp labels')
  }

  // 1. Crawl
  console.log('Crawling the fake API...')
  const t0 = Date.now()
  await run('src/crawler/crawl.ts', ['--once'], env)
  console.log(`  took ${((Date.now() - t0) / 1000).toFixed(1)} s, ${seen.total} requests`)

  const db = new Database(dbPath)
  const count = (sql: string) => (db.prepare(sql).get() as { n: number }).n
  assert.equal(count('SELECT COUNT(*) n FROM matches'), rankedMatches.length)
  ok(`stored all ${rankedMatches.length} ranked matches, skipped ${matches.size - rankedMatches.length} normal games`)
  assert.equal(count('SELECT COUNT(*) n FROM participants'), rankedMatches.length * 8)
  assert.equal(count('SELECT COUNT(*) n FROM players'), 300)
  ok('seeded 300 players from apex and paged ladders')
  assert.equal(seen.overLimit, 0, `client broke the rate limit ${seen.overLimit} times`)
  ok('never exceeded the rate limit the server announced')
  assert.ok(seen.forced429 === 1 && seen.forced500 > 0)
  ok(`recovered from a forced 429 and ${seen.forced500} server errors`)
  assert.equal(count("SELECT COUNT(*) n FROM matches WHERE patch = '99.3'"), rankedMatches.length)
  ok('parsed the patch number')

  // 2. Crawl again: nothing new should be fetched or duplicated
  const before = seen.total
  await run('src/crawler/crawl.ts', ['--once'], env)
  assert.equal(count('SELECT COUNT(*) n FROM matches'), rankedMatches.length)
  assert.equal(seen.total, before)
  ok('a second run made no requests and stored no duplicates')

  // 3. Stats, checked against a straight recount of the fixtures
  await run('src/stats/build.ts', [], env)
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

  const master = stat("SELECT matches FROM stat_meta WHERE bucket='MASTER+'").matches
  const all = stat("SELECT matches FROM stat_meta WHERE bucket='ALL'").matches
  assert.ok(master > 0 && master < all)
  assert.equal(stat("SELECT matches FROM stat_meta WHERE bucket='ALL@3d'").matches, all)
  ok(`rank and time filters work (${master} of ${all} matches are Master+)`)

  // Riot's real match data has no usable version, so patches are dated from a calendar.
  {
    const { toCalendar, patchAt, patchFor } = await import('../src/patches')
    const cal = toCalendar([
      { name: '16.21', mtime: 'Wed, 21 Oct 2026 07:15:23 GMT' }, { name: 'latest', mtime: 'Wed, 04 Nov 2026 07:47:21 GMT' },
      { name: '16.22', mtime: 'Wed, 04 Nov 2026 07:47:21 GMT' }, { name: 'pbe', mtime: 'Thu, 27 Feb 2025 06:58:53 GMT' },
    ])
    assert.deepEqual(cal.map((p) => p.patch), ['16.21', '16.22'])
    assert.equal(patchAt(Date.parse('2026-11-10T18:00:00Z'), cal), '16.22')
    assert.equal(patchAt(Date.parse('2026-11-03T18:00:00Z'), cal), '16.21')
    assert.equal(patchAt(Date.parse('2026-09-01T00:00:00Z'), cal), '16.21')
    assert.equal(patchAt(Date.now(), []), 'unknown')
    assert.equal(patchFor('Linux Version 15.9.1 [PUBLIC] <Releases/15.9>', 0), '15.9')
    ok('matches without a version are dated to a patch from the calendar')
  }

  // 4. Static data
  const { storeStatic, cleanText, itemType } = await import('../src/static/fetch')
  const icon = (n: string) => `ASSETS/Characters/${n}/HUD/${n}_Square.TFT_Set99.tex`
  const traitNames = [...ARCHETYPES.map((a) => a.trait), 'TFT99_Shared', 'TFT99_Loner']
  storeStatic(db as any, {
    items: ITEMS.map((i, n) => ({ apiName: i.id, name: `Thing ${n}`, icon: `ASSETS/Maps/TFT/Icons/Items/Thing${n}.TFT_Set99.tex`, composition: n < 6 ? ['TFT_Item_BFSword', 'TFT_Item_ChainVest'] : [] })),
    sets: {
      '98': { name: 'Old', champions: [], traits: [] },
      '99': {
        name: 'Test Set',
        traits: traitNames.map((t) => ({ apiName: t, name: t.replace('TFT99_', ''), effects: [{ minUnits: 2, style: 1 }, { minUnits: 4, style: 2 }, { minUnits: 6, style: 4 }] })),
        champions: UNITS.map((unit, n) => ({
          apiName: unit.id, name: `Unit ${n}`, cost: unit.cost, role: 'Attack Carry', tileIcon: icon(`TFT99_Unit${n}`),
          traits: ARCHETYPES.filter((a) => a.core.includes(n)).map((a) => a.trait.replace('TFT99_', '')).concat(n % 2 ? ['Shared'] : []),
          ability: { name: 'Big Hit', desc: 'Deal <physicalDamage>@Damage@ (%i:scaleAD%)</physicalDamage> damage.<br>Stun for @StunDuration@ seconds.', variables: [{ name: 'Damage', value: [0, 100, 150, 900] }, { name: 'StunDuration', value: [1, 1, 1, 1] }] },
          stats: { hp: 800, mana: 60, initialMana: 0, damage: 55, armor: 40, magicResist: 40, attackSpeed: 0.75, range: 4 },
        })),
      },
    },
  })
  assert.equal(stat("SELECT icon FROM static_data WHERE kind='unit' AND id='TFT99_Unit19'").icon,
    'https://raw.communitydragon.org/latest/game/assets/characters/tft99_unit19/hud/tft99_unit19_square.tft_set99.png')
  assert.equal(cleanText('Deal <b>@Damage@</b> (%i:scaleAD%) damage.<br>Stun for @Stun*100@%.', [{ name: 'Damage', value: [0, 100, 150, 900] }, { name: 'Stun', value: [0.5, 0.5, 0.5, 0.5] }]),
    'Deal 100/150/900 damage.\nStun for 50%.')
  assert.deepEqual(['TFT5_Item_BloodthirsterRadiant', 'TFT_Item_Artifact_Fishbones', 'TFT99_Item_VanguardEmblemItem', 'TFT_Item_BFSword', 'TFT_Item_X'].map((i) => itemType(i)),
    ['radiant', 'artifact', 'emblem', 'component', 'other'])
  ok('static data picks the newest set, builds icon URLs, cleans ability text and sorts items into types')
  db.close()

  // 5. The app's API
  const api = spawn(process.execPath, ['--import', 'tsx', 'src/server/index.ts', '--dev'], { env: { ...process.env, ...env } })
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

    assert.equal((await call('/api/icon/unit/Nope')).status, 404)
  } finally { api.kill() }

  console.log('\nAll checks passed.')
} finally {
  await fake.close()
  rmSync(dir, { recursive: true, force: true })
}
