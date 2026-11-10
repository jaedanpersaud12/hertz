/**
 * Re-labels every stored board with a comp, using the current rules in src/cluster.ts.
 * Run it after changing those rules, or once on a database made before comps existed.
 *
 *   npm run recluster
 */
import { compKey, type KeyTrait, type KeyUnit } from '../cluster'
import { openDb } from '../db'

const db = openDb()
const total = (db.prepare('SELECT COUNT(*) n FROM participants').get() as { n: number }).n
const maxId = (db.prepare('SELECT COALESCE(MAX(id), 0) n FROM participants').get() as { n: number }).n
const unitsOf = db.prepare('SELECT participant_id, character_id, star, rarity, n_items FROM units WHERE participant_id BETWEEN ? AND ?')
const traitsOf = db.prepare('SELECT participant_id, name, num_units, tier_current, style FROM traits WHERE participant_id BETWEEN ? AND ?')
const update = db.prepare('UPDATE participants SET comp_key = ? WHERE id = ?')

const STEP = 20_000
let done = 0
for (let from = 1; from <= maxId; from += STEP) {
  const to = from + STEP - 1
  const units = new Map<number, KeyUnit[]>()
  const traits = new Map<number, KeyTrait[]>()
  for (const u of unitsOf.all(from, to) as (KeyUnit & { participant_id: number })[]) {
    units.set(u.participant_id, [...(units.get(u.participant_id) ?? []), u])
  }
  for (const t of traitsOf.all(from, to) as (KeyTrait & { participant_id: number })[]) {
    traits.set(t.participant_id, [...(traits.get(t.participant_id) ?? []), t])
  }
  db.transaction(() => {
    for (const [id, us] of units) update.run(compKey(us, traits.get(id) ?? []), id)
  })()
  done += units.size
  console.log(`${done.toLocaleString('en-US')} of ${total.toLocaleString('en-US')} boards`)
}
db.close()
console.log('Done. Run "npm run stats" to rebuild the numbers.')
