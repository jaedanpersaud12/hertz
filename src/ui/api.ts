import { createContext, useCallback, useContext, useEffect, useState } from 'react'

// ---- Shapes the local API returns ---------------------------------------------
export type Entity = { id: string; name: string; icon: string | null; cost?: number | null; traits?: string[]; type?: string }
export type Rates = { games: number; avgPlace: number; winRate: number; top4Rate: number; frequency: number }
export type Scope = {
  patch: string; bucket: string; window: string; min: number; comps: number; matches: number
  builtAt: number | null; patches: string[]; prevPatch: string | null
}
export type Meta = Scope & {
  totals: { matches: number; comps: number; players: number; queued: number; newest: number | null; staticRows: number }
  byPatch: { patch: string; matches: number }[]
  buckets: string[]
  windows: { key: string; label: string }[]
}
export type TraitChipData = Entity & { units: number; style: number }
export type BoardUnit = Entity & { star?: number; freq?: number; items?: (Entity & { freq?: number })[] }
export type Comp = Rates & {
  id: string; name: string; trait: Entity | null; carries: Entity[]
  units: (BoardUnit & { star: number; freq: number; avgItems: number })[]
  flex: (Entity & { freq: number })[]; traits: TraitChipData[]
  avgLevel: number; pickRate: number; play: { kind: string; label: string }
}
export type Marks = { favs: string[]; hidden: string[]; pinned: string | null }

// ---- Filters shared by every stats page ---------------------------------------
export type Filters = { patch: string; bucket: string; window: string; min: number }
export const FiltersCtx = createContext<Filters>({ patch: '', bucket: 'ALL', window: '', min: 10 })
export const useFilters = () => useContext(FiltersCtx)

const query = (f: Filters) =>
  `bucket=${encodeURIComponent(f.bucket)}&window=${f.window}&min=${f.min}${f.patch ? `&patch=${encodeURIComponent(f.patch)}` : ''}`

/** Dev only: which stress dataset the switch at the bottom of the app has picked (see crates/tft-api/src/stress.rs). */
export const STRESS = import.meta.env.DEV ? new URLSearchParams(location.search).get('data') : null
const stressed = (url: string) => (STRESS ? `${url}${url.includes('?') ? '&' : '?'}data=${STRESS}` : url)

const OFFLINE = 'Cannot reach the local API. Is "npm run dev" still running?'

/** GET a local API path. Pass filters to scope it by patch, rank and time window. */
export function useApi<T>(path: string | null, f?: Filters) {
  const [state, setState] = useState<{ data: T | null; error: string | null; loading: boolean }>({ data: null, error: null, loading: !!path })
  const [tick, setTick] = useState(0)
  const url = path ? (f ? `${path}${path.includes('?') ? '&' : '?'}${query(f)}` : path) : null

  useEffect(() => {
    if (!url) { setState({ data: null, error: null, loading: false }); return }
    let live = true
    setState((s) => ({ ...s, loading: true }))
    fetch(stressed(url))
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((data) => live && setState({ data, error: null, loading: false }))
      .catch(() => live && setState({ data: null, error: OFFLINE, loading: false }))
    return () => { live = false }
  }, [url, tick])

  const reload = useCallback(() => setTick((t) => t + 1), [])
  return { ...state, reload }
}

export async function send<T>(path: string, body?: unknown, method = 'POST'): Promise<T> {
  const r = await fetch(path, { method, headers: { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) })
  if (!r.ok) throw new Error(OFFLINE)
  return r.json()
}

// ---- Favourites, hidden comps and the pinned comp ------------------------------
export type MarksApi = Marks & {
  setFav: (key: string, fav: boolean) => void
  toggleFav: (key: string) => void
  toggleHidden: (key: string) => void
  pin: (key: string | null) => void
}
export const MarksCtx = createContext<MarksApi>({ favs: [], hidden: [], pinned: null, setFav() {}, toggleFav() {}, toggleHidden() {}, pin() {} })
export const useMarks = () => useContext(MarksCtx)

export function useMarksState(): MarksApi {
  const [marks, setMarks] = useState<Marks>({ favs: [], hidden: [], pinned: null })
  useEffect(() => { fetch(stressed('/api/lists')).then((r) => r.json()).then(setMarks).catch(() => {}) }, [])
  return {
    ...marks,
    setFav: (key, fav) => send<Marks>('/api/lists/mark', { key, fav }).then(setMarks).catch(() => {}),
    toggleFav: (key) => send<Marks>('/api/lists/mark', { key, fav: !marks.favs.includes(key) }).then(setMarks).catch(() => {}),
    toggleHidden: (key) => send<Marks>('/api/lists/mark', { key, hidden: !marks.hidden.includes(key) }).then(setMarks).catch(() => {}),
    pin: (key) => send<Marks>('/api/lists/pin', { key }).then(setMarks).catch(() => {}),
  }
}

// ---- Formatting ----------------------------------------------------------------
export const pct = (v: number, digits = 1) => `${(v * 100).toFixed(digits)}%`
export const num = (v: number) => v.toLocaleString('en-US')
/** "1 unit", "2 units", "1,284 entries". */
export const count = (n: number, one: string, many = `${one}s`) => `${num(n)} ${n === 1 ? one : many}`
export const ago = (ms: number | null | undefined) => {
  if (!ms) return 'never'
  const m = Math.round((Date.now() - ms) / 60000)
  return m < 1 ? 'just now' : m < 90 ? `${m} min ago` : m < 2880 ? `${Math.round(m / 60)} hours ago` : `${Math.round(m / 1440)} days ago`
}
