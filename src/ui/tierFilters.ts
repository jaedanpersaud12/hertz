/**
 * The tier list's filters (PRD 2 C11) and where they live: the page URL's query string, next to the dev
 * stress switch's ?data=, so a reload or a shared link keeps them. Plain functions, so test/e2e.ts can check them.
 */
export type TierFilters = { unit: string; trait: string; item: string; playstyle: string; difficulty: string; rising: boolean }

export const NO_FILTERS: TierFilters = { unit: '', trait: '', item: '', playstyle: '', difficulty: '', rising: false }
const TEXT = ['unit', 'trait', 'item', 'playstyle', 'difficulty'] as const

/** The filters in a query string such as location.search. Anything else in it is ignored. */
export function readFilters(search: string): TierFilters {
  const p = new URLSearchParams(search)
  const f = { ...NO_FILTERS }
  for (const k of TEXT) f[k] = p.get(k) ?? ''
  f.rising = p.get('rising') === '1'
  return f
}

/** `search` with the filters set (and unset ones removed), other parameters kept. With a leading "?" when not empty. */
export function writeFilters(search: string, f: TierFilters): string {
  const p = new URLSearchParams(search)
  for (const k of TEXT) {
    if (f[k]) p.set(k, f[k])
    else p.delete(k)
  }
  if (f.rising) p.set('rising', '1')
  else p.delete('rising')
  const s = p.toString()
  return s ? `?${s}` : ''
}

/** The filters as /api/comps query parameters, set ones only: "" when there are none. */
export function filterQuery(f: TierFilters): string {
  return writeFilters('', f).replace(/^\?/, '')
}

export const anyFilter = (f: TierFilters) => TEXT.some((k) => f[k] !== '') || f.rising

/** The playstyle tag's kinds, as the filter names them. */
export const PLAYSTYLES: Record<string, string> = { reroll: 'Reroll', fast8: 'Fast 8', fast9: 'Fast 9', level: 'Standard levelling' }

/** Your difficulty tag (PRD 2 C10), easiest first, and the filter value for comps you have not tagged. */
export const DIFFICULTIES: Record<string, string> = { easy: 'Easy', medium: 'Medium', hard: 'Hard' }
export const UNTAGGED = 'none'
