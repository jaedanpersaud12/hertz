/**
 * Two comps side by side (PRD 2 C12): board, stats, items and trend in two columns. The URL holds both ids
 * (see ./compare.ts). Comps are picked from the tier list (right-click a comp, "Compare"), or here.
 */
import { ArrowsLeftRight, X } from '@phosphor-icons/react'
import { useEffect, useState, type ReactNode } from 'react'
import { count, pct, useApi, useFilters, useMarks, type Comp, type Entity, type Scope } from './api'
import { compareHash, pickForCompare, readCompare } from './compare'
import { Button, Change, GradeChip, Icon, IconButton, PlaceRail, Select, Status, Tag, UnitChip } from './parts'
import { DIFFICULTIES } from './tierFilters'

type Rated = { games: number; freq: number; avgPlace: number; placeChange: number }
type Detail = Scope & {
  comp: Comp | null
  carries: { unit: Entity; games: number; builds: (Rated & { items: Entity[] })[]; items: (Entity & Rated)[] }[]
}

// ---- Picking from the tier list ---------------------------------------------------
const PICK_KEY = 'hertz-compare-pick'
const PICK_EVENT = 'compare-pick'
const readPick = () => { try { return sessionStorage.getItem(PICK_KEY) } catch { return null } }

/** The comp waiting for a second one, and `pick` to add one (which opens the view once there are two). */
export function useComparePick(): [string | null, (id: string | null) => void] {
  const [waiting, setWaiting] = useState(readPick)
  useEffect(() => {
    const on = () => setWaiting(readPick())
    addEventListener(PICK_EVENT, on)
    return () => removeEventListener(PICK_EVENT, on)
  }, [])
  const pick = (id: string | null) => {
    const next = id === null ? { waiting: null, open: null } : pickForCompare(readPick(), id)
    try {
      if (next.waiting) sessionStorage.setItem(PICK_KEY, next.waiting)
      else sessionStorage.removeItem(PICK_KEY)
    } catch { /* storage off: the pick lasts until this page reloads */ }
    setWaiting(next.waiting)
    dispatchEvent(new Event(PICK_EVENT))
    if (next.open) location.hash = next.open
  }
  return [waiting, pick]
}

/** The bar under the tier list while one comp waits for a second. */
export function CompareTray({ comps }: { comps: Comp[] }) {
  const [waiting, pick] = useComparePick()
  if (!waiting) return null
  const name = comps.find((c) => c.id === waiting)?.name ?? 'a comp not in this list'
  return (
    <div className="compare-tray" role="status">
      <span>Comparing <b>{name}</b>. Right-click another comp and choose Compare.</span>
      <Button size="sm" variant="ghost" onClick={() => pick(null)}>Cancel</Button>
    </div>
  )
}

// ---- The view ----------------------------------------------------------------------
function useHashIds(): [string, string] {
  const none: [string, string] = ['', '']
  const [ids, setIds] = useState<[string, string]>(() => readCompare(location.hash) ?? none)
  useEffect(() => {
    const on = () => setIds(readCompare(location.hash) ?? none)
    addEventListener('hashchange', on)
    return () => removeEventListener('hashchange', on)
  }, [])
  return ids
}

const better = (a: number | undefined, b: number | undefined, lower: boolean) =>
  a != null && b != null && Math.abs(a - b) > 1e-9 && (lower ? a < b : a > b)

function Stat({ label, children, win }: { label: string; children: ReactNode; win?: boolean }) {
  return <div className={win ? 'win' : undefined}><dt>{label}</dt><dd>{children}{win ? <span className="sr-only"> (better)</span> : null}</dd></div>
}

function Column({ d, other, error, loading }: { d: Detail | null; other: Comp | null; error: string | null; loading: boolean }) {
  const marks = useMarks()
  if (error || !d) return <div className="compare-col"><Status error={error} loading={loading} /></div>
  const c = d.comp
  if (!c) {
    return (
      <div className="compare-col">
        <div className="empty-note"><b>Not listed here</b><span className="muted">This comp has no row for this patch, rank and period. Pick another, or widen the filters at the top.</span></div>
      </div>
    )
  }
  const diff = marks.difficulty[c.id]
  const o = other ?? undefined
  return (
    <div className="compare-col">
      <div className="compare-head">
        {c.tier ? <GradeChip grade={c.tier} /> : <span className="grade grade-none" title="Not graded yet">–</span>}
        <span className="comp-name">{c.name}</span>
        <span className="comp-tags">
          <Tag kind={c.play.kind}>{c.play.label}</Tag>
          {diff ? <Tag kind="difficulty">{DIFFICULTIES[diff]}</Tag> : null}
        </span>
      </div>
      <section>
        <h4>Board</h4>
        <div className="comp-units">{c.units.map((u) => <UnitChip key={u.id} u={u} size={36} />)}</div>
      </section>
      <section>
        <h4>Stats</h4>
        <dl className="compare-stats">
          <Stat label="Avg place" win={better(c.avgPlace, o?.avgPlace, true)}><PlaceRail value={c.avgPlace} /></Stat>
          <Stat label="Top 4" win={better(c.top4Rate, o?.top4Rate, false)}>{pct(c.top4Rate)}</Stat>
          <Stat label="Win rate" win={better(c.winRate, o?.winRate, false)}>{pct(c.winRate)}</Stat>
          <Stat label="Pick rate">{c.pickRate.toFixed(2)}</Stat>
          <Stat label="Games">{count(c.games, 'game')}</Stat>
        </dl>
      </section>
      <section>
        <h4>Items</h4>
        {d.carries?.length ? d.carries.map((k) => (
          <div key={k.unit.id} className="compare-carry">
            <div className="carry-head"><Icon e={k.unit} size={24} /> <span className="carry-item">{k.unit.name}</span></div>
            {k.builds[0] ? (
              <p className="compare-build" title={k.builds[0].items.map((i) => i.name).join(', ')}>
                {k.builds[0].items.map((i, n) => <Icon key={n} e={i} size={22} plain />)}
                <span className="muted small">Top build, {pct(k.builds[0].freq, 0)}</span> <Change value={k.builds[0].placeChange} />
              </p>
            ) : <p className="muted small">No build has 5 games yet.</p>}
            {k.items.slice(0, 3).map((i) => (
              <p key={i.id} className="compare-item"><Icon e={i} size={20} plain /> <span className="carry-item">{i.name}</span> <Change value={i.placeChange} /></p>
            ))}
          </div>
        )) : <p className="muted small">No carry holds three items often enough to show builds.</p>}
      </section>
      <section>
        <h4>Trend</h4>
        <dl className="compare-stats">
          <Stat label="Since last patch" win={better(c.patchDelta ?? undefined, o?.patchDelta ?? undefined, true)}>
            {c.patchDelta == null ? <span className="muted">New this patch</span> : <Change value={c.patchDelta} />}
          </Stat>
          <Stat label="Last 3 days" win={better(c.d3Delta ?? undefined, o?.d3Delta ?? undefined, true)}>
            {c.d3Delta == null ? <span className="muted">Nothing to compare with</span> : <Change value={c.d3Delta} />}
          </Stat>
        </dl>
      </section>
    </div>
  )
}

export function Compare() {
  const f = useFilters()
  const [a, b] = useHashIds()
  const list = useApi<Scope & { rows: Comp[] }>('/api/comps', f)
  const da = useApi<Detail>(a ? `/api/comps/${encodeURIComponent(a)}` : null, f)
  const db = useApi<Detail>(b ? `/api/comps/${encodeURIComponent(b)}` : null, f)
  const set = (x: string, y: string) => { location.hash = compareHash(x, y) }

  const rows = list.data?.rows ?? []
  const choices = (current: string) => {
    const opts = [{ value: '', label: 'Pick a comp' }, ...rows.map((c) => ({ value: c.id, label: c.name }))]
    return current && !rows.some((c) => c.id === current) ? [...opts, { value: current, label: da.data?.comp?.id === current ? da.data.comp.name : db.data?.comp?.id === current ? db.data.comp.name : 'A comp not in this list' }] : opts
  }

  return (
    <>
      <div className="toolbar compare-bar">
        <Select label="First comp" prefix="First:" value={a} options={choices(a)} onChange={(x) => set(x, b)} />
        <IconButton label="Swap sides" onClick={() => set(b, a)}><ArrowsLeftRight size={15} /></IconButton>
        <Select label="Second comp" prefix="Second:" value={b} options={choices(b)} onChange={(y) => set(a, y)} />
        <span className="toolbar-note"><a href="#/comps" className="link"><X size={12} /> Back to the tier list</a></span>
      </div>
      {!a || !b ? (
        <div className="empty">
          <h2>Pick two comps to compare</h2>
          <p>Choose them above, or on the tier list right-click a comp, choose Compare, then do the same on a second one.</p>
          <div className="mt-4"><Button onClick={() => { location.hash = '/comps' }}>Go to the tier list</Button></div>
        </div>
      ) : a === b ? (
        <div className="empty">
          <h2>That is the same comp twice</h2>
          <p>Pick a different comp for one of the sides.</p>
        </div>
      ) : (
        <div className="compare">
          <Column d={da.data} other={db.data?.comp ?? null} error={da.error} loading={da.loading} />
          <Column d={db.data} other={da.data?.comp ?? null} error={db.error} loading={db.loading} />
        </div>
      )}
    </>
  )
}
