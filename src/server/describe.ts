import type { DB } from '../db'
import { itemType } from '../static/fetch'

export type Kind = 'unit' | 'trait' | 'item'
export type Entity = { id: string; name: string; icon: string | null; [extra: string]: unknown }

/** "TFT18_KogMaw" -> "Kog Maw", for when Community Dragon data has not been downloaded yet. */
const prettify = (id: string) =>
  id.replace(/^TFT\d*_?/i, '').replace(/^(Item|Augment)_/i, '').replace(/([a-z])([A-Z])/g, '$1 $2').replace(/_/g, ' ')

/** Turns ids from match data into something showable: a name, an icon path and a few facts. */
export function makeDescribe(db: DB) {
  const row = db.prepare('SELECT name, icon, data FROM static_data WHERE kind = ? AND id = ? COLLATE NOCASE') // match data and set data sometimes differ in letter case
  const cache = new Map<string, Entity>()
  const count = db.prepare('SELECT COUNT(*) + COALESCE(SUM(LENGTH(name)), 0) n FROM static_data')
  let stamp = -1
  let checked = 0

  return function describe(kind: Kind, id: string, full = false): Entity {
    // Drop the cache when static data is re-downloaded. Checked at most every two seconds.
    if (Date.now() - checked > 2000) {
      checked = Date.now()
      const now = (count.get() as { n: number }).n
      if (now !== stamp) { cache.clear(); stamp = now }
    }

    const key = `${kind}|${id}|${full ? 1 : 0}`
    const hit = cache.get(key)
    if (hit) return hit

    const s = row.get(kind, id) as { name: string; icon: string | null; data: string } | undefined
    const data = s ? JSON.parse(s.data) : {}
    const out: Entity = { id, name: s?.name ?? prettify(id), icon: s?.icon ? `/api/icon/${kind}/${encodeURIComponent(id)}` : null }
    if (kind === 'unit') {
      out.cost = data.cost ?? null
      out.traits = data.traits ?? []
      if (full) Object.assign(out, { role: data.role ?? null, ability: data.ability ?? null, stats: data.stats ?? null })
    }
    if (kind === 'trait') out.breakpoints = data.breakpoints ?? []
    if (kind === 'item') {
      out.type = data.type ?? itemType(id)
      if (full) Object.assign(out, { composition: data.composition ?? [], text: data.text ?? '' })
    }
    cache.set(key, out)
    return out
  }
}
export type Describe = ReturnType<typeof makeDescribe>
