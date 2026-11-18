/**
 * Each table's own settings (PRD 3 U9): which columns are hidden and how it is sorted, kept per table id in
 * local storage so a reload keeps them. Plain functions, so test/e2e.ts can check them.
 * Storage may be missing, full, blocked or hold anything at all: every read falls back to the table's defaults.
 */
export type SortDirection = 'asc' | 'desc'
export type TableSort = { columnId: string; direction: SortDirection }

/** `sort` left out means "the table's default"; null means the player chose the original order. */
export type TablePrefs = { hidden: string[]; sort?: TableSort | null }

/** What a table tells the store about one of its columns. */
export type PrefColumn = { id: string; sortable: boolean }

export type PrefStore = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

export const NO_PREFS: TablePrefs = { hidden: [] }
export const tableKey = (id: string) => `tft-local-table:${id}`

function browserStore(): PrefStore | null {
  try { return globalThis.localStorage ?? null } catch { return null }
}

const isSort = (s: unknown): s is TableSort =>
  !!s && typeof s === 'object' && typeof (s as TableSort).columnId === 'string' && ((s as TableSort).direction === 'asc' || (s as TableSort).direction === 'desc')

/** What is stored for table `id`, checked for shape only. Anything unreadable gives NO_PREFS. */
export function readTablePrefs(id: string, store: PrefStore | null = browserStore()): TablePrefs {
  try {
    const raw: unknown = JSON.parse(store?.getItem(tableKey(id)) ?? 'null')
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return NO_PREFS
    const { hidden, sort } = raw as Record<string, unknown>
    const prefs: TablePrefs = { hidden: Array.isArray(hidden) ? hidden.filter((h): h is string => typeof h === 'string') : [] }
    if (sort === null) prefs.sort = null
    else if (isSort(sort)) prefs.sort = { columnId: sort.columnId, direction: sort.direction }
    return prefs
  } catch {
    return NO_PREFS
  }
}

/** Keeps table `id`'s settings. Back to defaults (nothing hidden, default sort) clears the entry. Never throws. */
export function writeTablePrefs(id: string, prefs: TablePrefs, store: PrefStore | null = browserStore()) {
  try {
    if (!prefs.hidden.length && prefs.sort === undefined) store?.removeItem(tableKey(id))
    else store?.setItem(tableKey(id), JSON.stringify(prefs.sort === undefined ? { hidden: prefs.hidden } : prefs))
  } catch { /* private mode or a full store: the setting just won't outlive the page */ }
}

/**
 * The settings with anything that no longer fits these columns dropped: hidden columns that are gone, the first
 * (name) column, which can't be hidden, and a sort on a column that is gone or can't be sorted.
 */
export function fitTablePrefs(prefs: TablePrefs, columns: PrefColumn[]): TablePrefs {
  const ids = new Set(columns.slice(1).map((c) => c.id))
  const hidden = [...new Set(prefs.hidden.filter((h) => ids.has(h)))]
  const fit: TablePrefs = { hidden }
  if (prefs.sort === null) fit.sort = null
  else if (prefs.sort && columns.some((c) => c.id === prefs.sort!.columnId && c.sortable)) fit.sort = prefs.sort
  return fit
}

/** The sort the table should show: the saved one when its column is showing, else the default when that is. */
export function tableSort(prefs: TablePrefs, columns: PrefColumn[], fallback: TableSort): TableSort | null {
  const shown = (id: string) => !prefs.hidden.includes(id) && columns.some((c) => c.id === id && c.sortable)
  if (prefs.sort === null) return null
  if (prefs.sort && shown(prefs.sort.columnId)) return prefs.sort
  return shown(fallback.columnId) ? fallback : null
}
