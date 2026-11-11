import { ArrowDown, ArrowUp, CaretDown, EyeSlash, PushPin } from '@phosphor-icons/react'
import { motion, useReducedMotion } from 'motion/react'
import { useEffect, useMemo, useState } from 'react'
import { count, pct, useApi, useFilters, useMarks, type Comp, type CompOptions, type Entity, type Marks, type Scope } from '../api'
import { useAccordion, useAutoHeight, type AccordionHeaderProps, type AccordionPanelProps } from '../interior/accordion'
import { ContextMenu, type ContextMenuItem } from '../interior/context-menu'
import { CopyButton } from '../interior/copy-button'
import { Button, Change, Dist, Empty, FavButton, GradeChip, Icon, IconButton, PlaceRail, Search, Select, Status, Tag, TraitChip, UnitChip } from '../parts'
import { anyFilter, filterQuery, NO_FILTERS, PLAYSTYLES, readFilters, writeFilters, type TierFilters } from '../tierFilters'
import tierConfig from '../../../config/tier-list.json'

type Detail = Scope & {
  comp: Comp | null
  levels: { level: number; games: number; share: number; avgPlace: number }[]
  builds: { unit: Entity; items: (Entity & { games: number; freq: number; avgPlace: number; placeChange: number })[] }[]
}

function CompDetail({ comp }: { comp: Comp }) {
  const f = useFilters()
  const { data, error, loading } = useApi<Detail>(`/api/comps/${encodeURIComponent(comp.id)}`, f)
  if (error || !data) return <div className="comp-detail"><Status error={error} loading={loading} /></div>
  const boardText = `${comp.name}: ${comp.units.map((u) => `${u.name}${u.items?.length ? ` (${u.items.map((i) => i.name).join(', ')})` : ''}`).join(', ')}`
  return (
    <div className="comp-detail">
      <section>
        <h4>Final level</h4>
        <p className="muted small">Share of games ending at each level, and how they placed. Top 4 in {pct(comp.top4Rate)} of games overall.</p>
        <Dist rows={data.levels.map((l) => ({ label: `Level ${l.level}`, share: l.share, avgPlace: l.avgPlace, games: l.games }))} />
        <div className="mt-4"><CopyButton className="ibtn" value={boardText} label="Copy board as text" copiedLabel="Copied" /></div>
      </section>

      <section className="grow">
        <h4>Items on the carries</h4>
        <p className="muted small">Place change compares boards with the item against this comp's average. Negative is better.</p>
        <div className="carry-grid">
          {data.builds.map((b) => (
            <div key={b.unit.id} className="carry">
              <div className="carry-head"><Icon e={b.unit} size={26} /> {b.unit.name}</div>
              <ul>
                {b.items.map((i) => (
                  <li key={i.id}>
                    <Icon e={i} size={22} plain /> <span className="carry-item" title={i.name}>{i.name}</span>
                    <span className="muted">{pct(i.freq, 0)}</span>
                    <Change value={i.games >= 5 ? i.placeChange : null} />
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </section>

      {comp.flex.length ? (
        <section>
          <h4>Other units played</h4>
          <p className="muted small">On the board in 15% to 50% of games.</p>
          <div className="flex-units">
            {comp.flex.map((u) => <span key={u.id} className="flex-unit"><UnitChip u={u} size={34} /><span className="muted small">{pct(u.freq, 0)}</span></span>)}
          </div>
        </section>
      ) : null}
    </div>
  )
}

/**
 * Trend on a row: an arrow and the change in average place since last patch (up and green is better), and
 * underneath, the last 3 days against the whole patch. A dash when there is nothing to compare with.
 */
function Trend({ comp }: { comp: Comp }) {
  const arrow = (d: number | null) => {
    if (d == null) return null
    const r = Math.round(d * 100) / 100
    return r <= -0.05 ? <ArrowUp size={12} weight="bold" aria-label="better" /> : r >= 0.05 ? <ArrowDown size={12} weight="bold" aria-label="worse" /> : null
  }
  const lastPatch = comp.patchDelta == null ? 'New this patch, nothing to compare with' : `${comp.prevPlace?.toFixed(2)} last patch`
  const days = comp.d3Delta == null ? null : `${comp.d3Place?.toFixed(2)} in the last 3 days`
  return (
    <span className="trend" title={[lastPatch, days].filter(Boolean).join('. ')}>
      <span className="trend-main">
        {comp.patchDelta == null ? <span className="muted">New</span> : <>{arrow(comp.patchDelta)}<Change value={comp.patchDelta} /></>}
      </span>
      {comp.d3Delta != null ? <span className="trend-sub"><span className="muted">3d</span> <Change value={comp.d3Delta} /></span> : null}
    </span>
  )
}

/** One comp. Pass header, panel and onToggle (from the accordion) to make it expandable. */
export function CompRow({ comp, open = false, onToggle, header, panel, exitOnHide }: {
  comp: Comp; open?: boolean; onToggle?: () => void; header?: AccordionHeaderProps; panel?: AccordionPanelProps
  /** Hiding removes this row from the list it is in, so let it fade out first. */
  exitOnHide?: boolean
}) {
  const marks = useMarks()
  const reduced = useReducedMotion()
  const auto = useAutoHeight()
  const [seen, setSeen] = useState(open) // the detail is only fetched once the row has been opened
  useEffect(() => { if (open) setSeen(true) }, [open])

  const fav = marks.favs.includes(comp.id)
  const hidden = marks.hidden.includes(comp.id)
  const pinned = marks.pinned === comp.id
  const [leaving, setLeaving] = useState(false)
  const toggleHidden = () => {
    if (hidden || !exitOnHide) return marks.toggleHidden(comp.id)
    setLeaving(true)
    setTimeout(() => marks.toggleHidden(comp.id), 180) // matches the .comp.leaving transition
  }

  const menu: ContextMenuItem[] = [
    { id: 'fav', label: fav ? 'Remove from favourites' : 'Add to favourites' },
    { id: 'pin', label: pinned ? 'Unpin' : 'Pin to your panel' },
    { id: 'hide', label: hidden ? 'Show this comp again' : 'Hide this comp' },
    { id: 'sep', type: 'separator' },
    { id: 'carry', label: `Open ${comp.carries[0]?.name ?? 'main carry'}` },
  ]
  const onMenu = (id: string) => {
    if (id === 'fav') marks.toggleFav(comp.id)
    if (id === 'pin') marks.pin(pinned ? null : comp.id)
    if (id === 'hide') toggleHidden()
    if (id === 'carry' && comp.carries[0]) location.hash = `/units/${encodeURIComponent(comp.carries[0].id)}`
  }

  return (
    <li className={`comp${leaving ? ' leaving' : ''}`} id={`comp-${comp.id}`}>
      <ContextMenu items={menu} onSelect={onMenu} label={`Actions for ${comp.name}`}>
        <div className="comp-row" onClick={onToggle} style={onToggle ? undefined : { cursor: 'default' }}>
          <span className="comp-grade">
            {comp.tier ? <GradeChip grade={comp.tier} /> : <span className="grade grade-none" title="Not graded yet. Rebuild the stats with npm run stats.">–</span>}
          </span>
          <div className="comp-id">
            <span className="comp-name">{comp.name}</span>
            <span className="comp-tags">
              <Tag kind={comp.play.kind}>{comp.play.label}</Tag>
              {comp.rising ? <Tag kind="rising">Rising</Tag> : null}
              {comp.frequency < 0.01 ? <Tag kind="rare">Rare</Tag> : null}
            </span>
          </div>
          <div className="comp-board">
            <div className="comp-traits">{comp.traits.map((t) => <TraitChip key={t.id} t={t} />)}</div>
            <div className="comp-units">{comp.units.map((u) => <UnitChip key={u.id} u={u} size={36} />)}</div>
          </div>
          <dl className="comp-stats">
            <div><dt>Avg place</dt><dd><PlaceRail value={comp.avgPlace} /></dd></div>
            <div><dt>Pick rate</dt><dd>{comp.pickRate.toFixed(2)}</dd></div>
            <div><dt>Win rate</dt><dd>{pct(comp.winRate)}</dd></div>
            <div><dt>Trend</dt><dd><Trend comp={comp} /></dd></div>
          </dl>
          <div className="comp-actions">
            {/* key: remount if the favourite changes somewhere else, e.g. from the right-click menu */}
            <FavButton key={String(fav)} liked={fav} onChange={(liked) => marks.setFav(comp.id, liked)} />
            <IconButton label={pinned ? 'Unpin' : 'Pin to your panel'} on={pinned} onClick={() => marks.pin(pinned ? null : comp.id)}><PushPin size={15} weight={pinned ? 'fill' : 'regular'} /></IconButton>
            <IconButton label={hidden ? 'Show this comp again' : 'Hide this comp'} on={hidden} onClick={toggleHidden}><EyeSlash size={15} /></IconButton>
            {header ? (
              <button {...header} className="btn ghost sm square" aria-label={open ? 'Hide details' : 'Show details'} onClick={(e) => { e.stopPropagation(); onToggle?.() }}>
                <motion.span className="grid" initial={false} animate={{ rotate: open ? 180 : 0 }} transition={reduced ? { duration: 0 } : { type: 'spring', stiffness: 420, damping: 30 }}>
                  <CaretDown size={14} />
                </motion.span>
              </button>
            ) : null}
          </div>
        </div>
      </ContextMenu>
      {panel ? (
        <motion.div initial={false} animate={{ height: open ? auto.height : 0 }} style={{ overflow: 'hidden' }}
          transition={reduced || !auto.ready ? { duration: 0 } : { type: 'spring', stiffness: 380, damping: 36 }}>
          <div ref={auto.ref} {...panel}>{seen ? <CompDetail comp={comp} /> : null}</div>
        </motion.div>
      ) : null}
    </li>
  )
}

const SORTS = [
  { value: 'tier', label: 'Tier', get: (c: Comp) => (c.score == null ? Infinity : -c.score) },
  { value: 'place', label: 'Avg place', get: (c: Comp) => c.avgPlace },
  { value: 'trend', label: 'Trend', get: (c: Comp) => c.patchDelta ?? c.d3Delta ?? Infinity },
  { value: 'pick', label: 'Pick rate', get: (c: Comp) => -c.pickRate },
  { value: 'win', label: 'Win rate', get: (c: Comp) => -c.winRate },
]
const OPEN_KEY = 'tft-local-open-comp'
const RISING_HINT = `Placing at least ${tierConfig.risingPlaces} better than last patch, or in the last 3 days than across the patch`

/** The tier list's filters, kept in the page URL (see ../tierFilters.ts). */
function useTierFilters(): [TierFilters, (f: TierFilters) => void] {
  const [f, setF] = useState(() => readFilters(location.search))
  const set = (next: TierFilters) => {
    history.replaceState(history.state, '', `${location.pathname}${writeFilters(location.search, next)}${location.hash}`)
    setF(next)
  }
  return [f, set]
}

/** Choices for one filter: "any" first, and the current pick even when this scope no longer lists it. */
function choices(any: string, list: Entity[], current: string) {
  const opts = [{ value: '', label: any }, ...list.map((e) => ({ value: e.id, label: e.name }))]
  return current && !list.some((e) => e.id === current) ? [...opts, { value: current, label: current }] : opts
}

function FilterBar({ options, f, set }: { options: CompOptions | undefined; f: TierFilters; set: (f: TierFilters) => void }) {
  const o = options ?? { units: [], traits: [], items: [], playstyles: [] }
  const plays = o.playstyles.map((k) => ({ id: k, name: PLAYSTYLES[k] ?? k, icon: null }))
  const label = (prefix: string, v: string) => (v ? prefix : undefined)
  return (
    <div className="toolbar filters" role="group" aria-label="Filter comps">
      <Select label="Any unit" prefix={label('Unit:', f.unit)} value={f.unit} onChange={(unit) => set({ ...f, unit })} options={choices('Any unit', o.units, f.unit)} />
      <Select label="Any trait" prefix={label('Trait:', f.trait)} value={f.trait} onChange={(trait) => set({ ...f, trait })} options={choices('Any trait', o.traits, f.trait)} />
      <Select label="Any item" prefix={label('Item:', f.item)} value={f.item} onChange={(item) => set({ ...f, item })} options={choices('Any item', o.items, f.item)} />
      <Select label="Any playstyle" value={f.playstyle} onChange={(playstyle) => set({ ...f, playstyle })} options={choices('Any playstyle', plays, f.playstyle)} />
      <label className="check" title={RISING_HINT}><input type="checkbox" checked={f.rising} onChange={(e) => set({ ...f, rising: e.target.checked })} /> Rising only</label>
      {anyFilter(f) ? <Button size="sm" variant="ghost" onClick={() => set(NO_FILTERS)}>Clear filters</Button> : null}
    </div>
  )
}

export function Comps() {
  const f = useFilters()
  const marks = useMarks()
  const [tf, setTf] = useTierFilters()
  const fq = filterQuery(tf)
  const { data, error, loading } = useApi<Scope & Marks & { rows: Comp[]; options: CompOptions }>(`/api/comps${fq ? `?${fq}` : ''}`, f)
  const [search, setSearch] = useState('')
  const [sort, setSort] = useState('tier')
  const [favOnly, setFavOnly] = useState(false)
  const [showHidden, setShowHidden] = useState(false)
  const [open, setOpen] = useState<string[]>([])

  const rows = useMemo(() => {
    const q = search.trim().toLowerCase()
    const get = SORTS.find((s) => s.value === sort)!.get
    return (data?.rows ?? [])
      .filter((c) => showHidden || !marks.hidden.includes(c.id) || open.includes(c.id))
      .filter((c) => !favOnly || marks.favs.includes(c.id))
      .filter((c) => !q || c.name.toLowerCase().includes(q) || c.units.some((u) => u.name.toLowerCase().includes(q)) || c.traits.some((t) => t.name.toLowerCase().includes(q)))
      .sort((a, b) => get(a) - get(b))
  }, [data, search, sort, favOnly, showHidden, marks.hidden, marks.favs, open])

  const entries = useMemo(() => rows.map((c) => ({ id: c.id })), [rows])
  const acc = useAccordion({ items: entries, type: 'single', open, onOpenChange: setOpen })

  // The search palette can ask for one comp to be opened and scrolled to.
  useEffect(() => {
    const show = () => {
      const id = sessionStorage.getItem(OPEN_KEY)
      if (!id || !data) return
      sessionStorage.removeItem(OPEN_KEY)
      setSearch(''); setFavOnly(false); setTf(NO_FILTERS); setOpen([id])
      setTimeout(() => document.getElementById(`comp-${id}`)?.scrollIntoView({ block: 'center', behavior: 'smooth' }), 60)
    }
    show()
    addEventListener('open-comp', show)
    return () => removeEventListener('open-comp', show)
  }, [data])

  if (error || !data) return <Status error={error} loading={loading} />
  return (
    <>
      <div className="toolbar">
        <Search value={search} onChange={setSearch} placeholder="Search comps, units or traits" />
        <Select label="Sort" prefix="Sort by" value={sort} onChange={setSort} options={SORTS.map(({ value, label }) => ({ value, label }))} />
        <label className="check"><input type="checkbox" checked={favOnly} onChange={(e) => setFavOnly(e.target.checked)} /> Favourites only</label>
        <label className="check"><input type="checkbox" checked={showHidden} onChange={(e) => setShowHidden(e.target.checked)} /> Show hidden{marks.hidden.length ? ` (${marks.hidden.length})` : ''}</label>
        <span className="toolbar-note">{count(rows.length, 'comp')} from {count(data.comps, 'board')}</span>
      </div>
      {data.patches.length ? <FilterBar options={data.options} f={tf} set={setTf} /> : null}
      {rows.length ? (
        <ul className="comps">
          {rows.map((c) => (
            <CompRow key={c.id} comp={c} open={acc.isOpen(c.id)} onToggle={() => acc.toggle(c.id)} header={acc.headerProps(c.id)} panel={acc.panelProps(c.id)} exitOnHide={!showHidden} />
          ))}
        </ul>
      ) : anyFilter(tf) && !data.rows.length && data.patches.length ? (
        <div className="empty">
          <h2>No comps match these filters</h2>
          <p>Each filter you add must hold for a comp to show. Remove one, or clear them all to see every comp.</p>
          <div className="mt-4"><Button onClick={() => setTf(NO_FILTERS)}>Clear filters</Button></div>
        </div>
      ) : <Empty scope={data} what="No comp" />}
    </>
  )
}
