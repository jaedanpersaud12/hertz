/**
 * Comparing two comps side by side (PRD 2 C12). The view lives at #/compare/<first id>/<second id>, so a reload
 * or a shared link opens the same two. Plain functions, so test/e2e.ts can check them.
 */

/** The hash for comparing `a` with `b` (ids are encoded, so ids with "/" or "|" survive). */
export const compareHash = (a: string, b: string) => `#/compare/${encodeURIComponent(a)}/${encodeURIComponent(b)}`

/**
 * The two ids in a hash such as location.hash, or null when it is not the compare view. A missing or empty id is
 * "", so the view can ask for it.
 */
export function readCompare(hash: string): [string, string] | null {
  const parts = hash.replace(/^#?\/?/, '').split('/')
  if (parts[0] !== 'compare') return null
  const id = (s: string | undefined) => {
    try { return s ? decodeURIComponent(s) : '' } catch { return s ?? '' }
  }
  return [id(parts[1]), id(parts[2])]
}

/**
 * Picking from the tier list: the first comp picked waits here until a second one is picked. Returns the hash to
 * open once there are two (null while waiting); picking the waiting comp again drops it.
 */
export function pickForCompare(waiting: string | null, picked: string): { waiting: string | null; open: string | null } {
  if (!waiting) return { waiting: picked, open: null }
  if (waiting === picked) return { waiting: null, open: null }
  return { waiting: null, open: compareHash(waiting, picked) }
}
