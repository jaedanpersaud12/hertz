/**
 * Which patch a match was played on.
 *
 * Riot's TFT match data no longer carries a usable version (it reads "TFT Unreal Version ?.?.?.?"),
 * so the patch is worked out from when the game was played. Community Dragon publishes one folder per
 * patch, created on release day, and that list is the calendar. Patch numbers are the client's
 * (16.22), not TFT's own set numbering.
 */
import { kvGet, kvSet, type DB } from './db'

export type PatchStart = { patch: string; since: number }
const KEY = 'patch_calendar'
const LISTING = 'https://raw.communitydragon.org/json/'

let calendar: PatchStart[] = []

/** Use the calendar saved by the last refresh. */
export function loadCalendar(db: DB) {
  calendar = kvGet<PatchStart[]>(db, KEY) ?? []
  return calendar
}

/** Download the calendar and save it. Returns false (and keeps the saved one) if the download fails. */
export async function refreshCalendar(db: DB): Promise<boolean> {
  try {
    const res = await fetch(LISTING, { signal: AbortSignal.timeout(15_000) })
    if (!res.ok) throw new Error(String(res.status))
    const found = toCalendar((await res.json()) as { name: string; mtime?: string }[])
    if (!found.length) throw new Error('no patches listed')
    kvSet(db, KEY, found)
    calendar = found
    return true
  } catch {
    loadCalendar(db)
    return false
  }
}

export const toCalendar = (entries: { name: string; mtime?: string }[]): PatchStart[] =>
  entries
    .filter((e) => /^\d+\.\d+$/.test(e.name) && e.mtime && Number.isFinite(Date.parse(e.mtime)))
    .map((e) => ({ patch: e.name, since: Date.parse(e.mtime!) }))
    .sort((a, b) => a.since - b.since)

/** The patch live at a moment in time, or "unknown" before the calendar has been downloaded. */
export function patchAt(when: number, cal: PatchStart[] = calendar): string {
  if (!cal.length) return 'unknown'
  let current = cal[0].patch // games older than the calendar count as its first patch
  for (const p of cal) if (p.since <= when) current = p.patch
  return current
}

/** "<Releases/15.22>" when Riot provides it, otherwise by date. */
export function patchFor(gameVersion: string | undefined, when: number): string {
  const m = gameVersion?.match(/<Releases\/(\d+\.\d+)>/) ?? gameVersion?.match(/Version (\d+\.\d+)/)
  return m ? m[1] : patchAt(when)
}

/** Give a patch to matches stored before the calendar was known. Returns how many were fixed. */
export function relabelUnknown(db: DB): number {
  if (!calendar.length) return 0
  const fix = db.prepare("UPDATE matches SET patch = ? WHERE patch = 'unknown' AND game_datetime >= ?")
  let changed = 0
  db.transaction(() => {
    for (const p of [...calendar].reverse()) changed += fix.run(p.patch, p.since).changes
    changed += db.prepare("UPDATE matches SET patch = ? WHERE patch = 'unknown'").run(calendar[0].patch).changes
  })()
  return changed
}
