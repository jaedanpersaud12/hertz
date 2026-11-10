/**
 * Downloads names, costs, abilities, trait breakpoints and icon paths for the current set from Community Dragon.
 *
 *   npm run static              current live patch
 *   npm run static -- --pbe     test server data
 *
 * Run it again after each patch or new set.
 */
import { fileURLToPath } from 'node:url'
import { openDb, type DB } from '../db'

const CDRAGON = 'https://raw.communitydragon.org'

/** "ASSETS/Characters/X/Icon.TFT_Set18.tex" -> a PNG URL on Community Dragon. */
export function iconUrl(path: string | null | undefined, channel = 'latest'): string | null {
  if (!path) return null
  return `${CDRAGON}/${channel}/game/${path.toLowerCase().replace(/\.(tex|dds)$/, '.png')}`
}

type CdVar = { name: string; value: number[] | null }
type CdUnit = {
  apiName: string; name: string; cost: number; traits: string[]; role?: string | null
  tileIcon?: string; squareIcon?: string; icon?: string
  ability?: { name?: string; desc?: string; variables?: CdVar[] }
  stats?: Record<string, number | null>
}
type CdTrait = { apiName: string; name: string; desc?: string; icon?: string; effects?: { minUnits: number; style: number }[] }
type CdItem = { apiName: string; name: string; desc?: string; icon?: string; composition?: string[]; effects?: Record<string, number> }
export type CdData = { items: CdItem[]; sets: Record<string, { name: string; champions: CdUnit[]; traits: CdTrait[] }> }

const COMPONENTS = /^TFT_Item_(BFSword|RecurveBow|NeedlesslyLargeRod|TearOfTheGoddess|ChainVest|NegatronCloak|GiantsBelt|SparringGloves|Spatula|FryingPan)$/

/** Sort an item into the groups the Items page filters by. */
export function itemType(id: string, composition: string[] = []): string {
  if (/radiant/i.test(id)) return 'radiant'
  if (/artifact|ornn/i.test(id)) return 'artifact'
  if (/emblem/i.test(id)) return 'emblem'
  if (/support/i.test(id)) return 'support'
  if (COMPONENTS.test(id)) return 'component'
  if (composition.length === 2) return 'craftable'
  return 'other'
}

/** Ability text arrives with markup and @Variable@ placeholders. Make it readable. */
export function cleanText(desc: string | undefined, variables: CdVar[] = []): string {
  if (!desc) return ''
  const byName = new Map(variables.map((v) => [v.name.toLowerCase(), v.value]))
  const fmt = (n: number) => String(Math.round(n * 100) / 100)
  return desc
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/@([A-Za-z0-9_.:]+?)(\*100)?@/g, (_, raw: string, pct?: string) => {
      const value = byName.get(raw.toLowerCase()) ?? byName.get(raw.toLowerCase().replace(/^modified/, ''))
      if (!value) return '…'
      // Index 1 to 3 are the one-, two- and three-star values.
      const stars = [value[1], value[2], value[3]].filter((v) => typeof v === 'number').map((v) => fmt(pct ? v * 100 : v))
      return new Set(stars).size === 1 ? stars[0] : stars.join('/')
    })
    .replace(/%i:[^%]+%/g, '')
    .replace(/&nbsp;/g, ' ')
    .replace(/[ \t]+/g, ' ')
    .replace(/ ?\( ?\)/g, '')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

export function storeStatic(db: DB, data: CdData, channel = 'latest') {
  const setNumber = Math.max(...Object.keys(data.sets).map(Number).filter(Number.isFinite))
  const set = data.sets[String(setNumber)]
  const ins = db.prepare('INSERT OR REPLACE INTO static_data (kind, id, name, icon, data) VALUES (?,?,?,?,?)')

  db.transaction(() => {
    db.exec('DELETE FROM static_data')
    for (const u of set.champions) {
      if (!u.apiName || !u.name) continue
      ins.run('unit', u.apiName, u.name, iconUrl(u.tileIcon ?? u.squareIcon ?? u.icon, channel), JSON.stringify({
        cost: u.cost,
        traits: u.traits ?? [],
        role: u.role ?? null,
        ability: u.ability?.name ? { name: u.ability.name, text: cleanText(u.ability.desc, u.ability.variables) } : null,
        stats: u.stats ?? null,
      }))
    }
    for (const t of set.traits) {
      if (!t.apiName || !t.name) continue
      ins.run('trait', t.apiName, t.name, iconUrl(t.icon, channel), JSON.stringify({
        breakpoints: (t.effects ?? []).map((e) => ({ units: e.minUnits, style: e.style })),
      }))
    }
    for (const i of data.items) {
      if (!i.apiName || !i.name) continue
      ins.run('item', i.apiName, i.name, iconUrl(i.icon, channel), JSON.stringify({
        composition: i.composition ?? [],
        type: itemType(i.apiName, i.composition),
        text: cleanText(i.desc, Object.entries(i.effects ?? {}).map(([name, v]) => ({ name, value: [v, v, v, v] }))),
      }))
    }
  })()
  return { setNumber, setName: set.name, units: set.champions.length, traits: set.traits.length, items: data.items.length }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const channel = process.argv.includes('--pbe') ? 'pbe' : 'latest'
  const url = `${CDRAGON}/${channel}/cdragon/tft/en_us.json`
  console.log(`Downloading ${url} ...`)
  const res = await fetch(url)
  if (!res.ok) throw new Error(`Community Dragon answered ${res.status}`)
  const db = openDb()
  const r = storeStatic(db, (await res.json()) as CdData, channel)
  db.close()
  console.log(`Stored set ${r.setNumber} (${r.setName}): ${r.units} units, ${r.traits} traits, ${r.items} items.`)
}
