import { ArrowDown, ArrowUp, CaretDown, EyeSlash, PushPin } from '@phosphor-icons/react'
import { motion, useReducedMotion } from 'motion/react'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { count, num, pct, useApi, useFilters, useMarks, type Comp, type CompOptions, type Difficulty, type Entity, type Marks, type Scope } from '../api'
import { useAccordion, useAutoHeight, type AccordionHeaderProps, type AccordionPanelProps } from '../interior/accordion'
import { ContextMenu, type ContextMenuItem } from '../interior/context-menu'
import { CopyButton } from '../interior/copy-button'
import { Button, Change, Empty, FavButton, GradeChip, Icon, IconButton, PlaceRail, placeTone, Search, Select, Status, Tag, TraitChip, UnitChip } from '../parts'
import { anyFilter, DIFFICULTIES, filterQuery, NO_FILTERS, PLAYSTYLES, readFilters, UNTAGGED, writeFilters, type TierFilters } from '../tierFilters'
import tierConfig from '../../../config/tier-list.json'
import { CompOverrides } from '../CompOverrides'
import { CompareTray, useComparePick } from '../CompCompare'
import { Flex, Variants, type AddAt9, type Swap } from '../CompFlex'

type Rated = { games: number; freq: number; avgPlace: number; placeChange: number }
type Band = {
  band: number; label: string; games: number; avgPlace: number; commonLevel: number | null; avgRound: number | null
  levels: { level: number; games: number; share: number }[]
}
type Detail = Scope & {
  comp: Comp | null
  levels: { level: number; games: number; share: number; avgPlace: number }[]
  builds: { unit: Entity; items: (Entity & Rated)[] }[]
  levelling: Band[]
  carries: { unit: Entity; games: number; builds: (Rated & { items: Entity[] })[]; items: (Entity & Rated)[] }[]
  holders: { carry: Entity; games: number; top4: number; units: (Entity & { games: number; top4: number; top4Share: number; avgPlace: number; sharedTraits: string[]; sameRole: boolean })[] }[]
  special: (Rated & { item: Entity; holder: Entity })[]
  addAt9: AddAt9[]
  swaps: Swap[]
}

/** A section's own empty state: what is missing and what to do about it. */
const None = ({ children }: { children: ReactNode }) => <p className="muted small detail-none">{children}</p>
const MORE = 'Collect more matches, or pick a wider rank filter or the whole patch.'

/** C5: one stacked bar per placement band, a segment per final level. */
function Levelling({ bands }: { bands: Band[] }) {
  return (
    <section className="detail-levelling">
      <h4>Levelling <Tag kind="level">Estimated</Tag></h4>
      <p className="muted small">The level boards ended on, by where they placed. Riot only reports the final level, so when to level is an estimate.</p>
      {bands.length ? (
        <ul className="bands">
          {bands.map((b) => (
            <li key={b.band}>
              <span className="band-label">{b.label}<span className="muted small">{count(b.games, 'game')}</span></span>
              <span className="band-bar" role="img" aria-label={b.levels.map((l) => `level ${l.level} ${pct(l.share, 0)}`).join(', ') || 'level not reported'}>
                {b.levels.map((l) => (
                  <span key={l.level} className={l.level === b.commonLevel ? 'common' : ''} style={{ flexGrow: l.share }} title={`Level ${l.level}: ${pct(l.share)} (${count(l.games, 'game')})`}>
                    {l.share >= 0.12 ? l.level : ''}
                  </span>
                ))}
              </span>
              <span className="band-facts">
                <span>{b.commonLevel != null ? `Usually ${b.commonLevel}` : 'Level not reported'}</span>
                <span className="muted small">{b.avgRound != null ? `Last round ${Math.round(b.avgRound)}` : 'Last round not reported'}</span>
              </span>
            </li>
          ))}
        </ul>
      ) : <None>No levels recorded for this comp yet. {MORE}</None>}
    </section>
  )
}

const ItemRow = ({ e, change }: { e: Entity; change: Rated }) => (
  <li>
    <Icon e={e} size={22} plain /> <span className="carry-item" title={e.name}>{e.name}</span>
    <span className="muted">{pct(change.freq, 0)}</span>
    <Change value={change.placeChange} />
  </li>
)

/** C6: per carry, its most played builds and its best single items. */
function Carries({ carries }: { carries: Detail['carries'] }) {
  return (
    <section className="grow">
      <h4>Carry items</h4>
      <p className="muted small">From boards where a unit holds three items. Place change against the carry's average with any three items (builds) or on the board at all (single items). Negative is better.</p>
      {carries.length ? (
        <div className="carry-grid">
          {carries.map((c) => (
            <div key={c.unit.id} className="carry">
              <div className="carry-head"><Icon e={c.unit} size={26} /> <span className="carry-name"><span className="carry-item">{c.unit.name}</span><span className="muted small">{count(c.games, 'game')} with three items</span></span></div>
              <h5>Top builds</h5>
              {c.builds.length ? (
                <ul>
                  {c.builds.map((b) => (
                    <li key={b.items.map((i) => i.id).join('|')} className="build">
                      <span className="build-items" title={b.items.map((i) => i.name).join(', ')}>{b.items.map((i, n) => <Icon key={n} e={i} size={22} plain />)}</span>
                      <span className="muted">{pct(b.freq, 0)}</span>
                      <Change value={b.placeChange} />
                    </li>
                  ))}
                </ul>
              ) : <None>No build has 5 games yet.</None>}
              <h5>Best single items</h5>
              {c.items.length ? <ul>{c.items.map((i) => <ItemRow key={i.id} e={i} change={i} />)}</ul> : <None>No item has 10 games yet.</None>}
            </div>
          ))}
        </div>
      ) : <None>No carry holds three items often enough to show builds. {MORE}</None>}
    </section>
  )
}

/** C7: cheaper units that can hold a carry's items early. */
function Holders({ holders }: { holders: Detail['holders'] }) {
  const listed = holders.filter((h) => h.units.length)
  return (
    <section className="grow">
      <h4>Hold items early on</h4>
      <p className="muted small">Cheaper units that share a trait or role with the carry, by how often they were on its top 4 boards.</p>
      {listed.length ? (
        <div className="carry-grid">
          {listed.map((h) => (
            <div key={h.carry.id} className="carry">
              <div className="carry-head"><Icon e={h.carry} size={26} /> <span className="carry-item">For {h.carry.name}</span></div>
              <ul>
                {h.units.map((u) => {
                  const why = u.sharedTraits.length ? `Shares ${u.sharedTraits.join(', ')}` : u.sameRole ? 'Same role' : 'Cheaper unit'
                  return (
                    <li key={u.id} className="holder" title={`${why}. On ${count(u.top4, 'top 4 board')} of ${count(h.top4, 'top 4 board')}, average place ${u.avgPlace.toFixed(2)}.`}>
                      <Icon e={u} size={22} /> <span className="carry-item">{u.name} <span className="muted small">{why}</span></span>
                      <span className="muted">{pct(u.top4Share, 0)}</span>
                      <span className={`tone-${placeTone(u.avgPlace)}`}>{u.avgPlace.toFixed(2)}</span>
                    </li>
                  )
                })}
              </ul>
            </div>
          ))}
        </div>
      ) : <None>No cheaper unit shares a trait or role with the carry yet. {MORE}</None>}
    </section>
  )
}

/** C9: emblems, artifacts and radiant items on this comp. */
function Special({ rows }: { rows: Detail['special'] }) {
  return (
    <section className="grow">
      <h4>Emblems, artifacts and radiants</h4>
      <p className="muted small">Each on its holder, with 30 games or more. Place change against this comp's average.</p>
      {rows.length ? (
        <ul className="special">
          {rows.map((r) => (
            <li key={`${r.item.id}|${r.holder.id}`}>
              <Icon e={r.item} size={22} plain /> <span className="carry-item" title={r.item.name}>{r.item.name}</span>
              <span className="special-holder"><Icon e={r.holder} size={20} /> <span className="carry-item" title={r.holder.name}>{r.holder.name}</span></span>
              <span className="muted">{num(r.games)}</span>
              <Change value={r.placeChange} />
            </li>
          ))}
        </ul>
      ) : <None>No emblem, artifact or radiant item has 30 games on this comp yet.</None>}
    </section>
  )
}

function CompDetail({ comp }: { comp: Comp }) {
  const f = useFilters()
  const { data, error, loading } = useApi<Detail>(`/api/comps/${encodeURIComponent(comp.id)}`, f)
  if (error || !data) return <div className="comp-detail"><Status error={error} loading={loading} /></div>
  const boardText = `${comp.name}: ${comp.units.map((u) => `${u.name}${u.items?.length ? ` (${u.items.map((i) => i.name).join(', ')})` : ''}`).join(', ')}`
  return (
    <div className="comp-detail">
      <Levelling bands={data.levelling ?? []} />
      <Carries carries={data.carries ?? []} />
      <Holders holders={data.holders ?? []} />
      <Flex adds={data.addAt9 ?? []} swaps={data.swaps ?? []} />
      <Special rows={data.special ?? []} />
      <Variants id={comp.id} />

      {comp.flex.length ? (
        <section>
          <h4>Other units played</h4>
          <p className="muted small">On the board in 15% to 50% of games.</p>
          <div className="flex-units">
            {comp.flex.map((u) => <span key={u.id} className="flex-unit"><UnitChip u={u} size={34} /><span className="muted small">{pct(u.freq, 0)}</span></span>)}
          </div>
        </section>
      ) : null}
      <div className="detail-copy"><CopyButton className="ibtn" value={boardText} label="Copy board as text" copiedLabel="Copied" /></div>
      <CompOverrides comp={comp} />
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
  const difficulty = marks.difficulty[comp.id] ?? null
  const [waiting, pick] = useComparePick()
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
    { id: 'compare', label: waiting === comp.id ? 'Stop comparing this comp' : waiting ? 'Compare with the comp you picked' : 'Compare with another comp' },
    { id: 'sep-d', type: 'separator' },
    ...Object.entries(DIFFICULTIES).map(([level, label]) => ({ id: `difficulty:${level}`, label: difficulty === level ? `Difficulty: ${label} (set)` : `Difficulty: ${label}` })),
    ...(difficulty ? [{ id: 'difficulty:', label: 'Clear difficulty' }] : []),
    { id: 'sep', type: 'separator' },
    { id: 'carry', label: `Open ${comp.carries[0]?.name ?? 'main carry'}` },
  ]
  const onMenu = (id: string) => {
    if (id === 'fav') marks.toggleFav(comp.id)
    if (id === 'pin') marks.pin(pinned ? null : comp.id)
    if (id === 'hide') toggleHidden()
    if (id === 'compare') pick(comp.id)
    if (id.startsWith('difficulty:')) {
      const level = id.slice('difficulty:'.length)
      marks.setDifficulty(comp.id, level && level !== difficulty ? (level as Difficulty) : null)
    }
    if (id === 'carry' && comp.carries[0]) location.hash = `/units/${encodeURIComponent(comp.carries[0].id)}`
  }

  return (
    <li className={`comp${leaving ? ' leaving' : ''}${waiting === comp.id ? ' comparing' : ''}`} id={`comp-${comp.id}`}>
      <ContextMenu items={menu} onSelect={onMenu} label={`Actions for ${comp.name}`}>
        <div className="comp-row" onClick={onToggle} style={onToggle ? undefined : { cursor: 'default' }}>
          <span className="comp-grade">
            {comp.tier ? <GradeChip grade={comp.tier} /> : <span className="grade grade-none" title="Not graded yet. Rebuild the stats with npm run stats.">–</span>}
          </span>
          <div className="comp-id">
            <span className="comp-name">{comp.name}</span>
            <span className="comp-tags">
              <Tag kind={comp.play.kind}>{comp.play.label}</Tag>
              {difficulty ? <Tag kind="difficulty">{DIFFICULTIES[difficulty]}</Tag> : null}
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

/** Your difficulty tags (C10) as filter choices, and comps you have not tagged. */
const DIFFICULTY_CHOICES = [
  { value: '', label: 'Any difficulty' },
  ...Object.entries(DIFFICULTIES).map(([value, label]) => ({ value, label })),
  { value: UNTAGGED, label: 'No difficulty set' },
]

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
      <Select label="Any difficulty" value={f.difficulty} onChange={(difficulty) => set({ ...f, difficulty })} options={DIFFICULTY_CHOICES} />
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
  const { data, error, loading, reload } = useApi<Scope & Marks & { rows: Comp[]; options: CompOptions }>(`/api/comps${fq ? `?${fq}` : ''}`, f)
  // The difficulty filter runs on the server, so a tag changed while it is on needs the list again.
  const tags = JSON.stringify(marks.difficulty)
  useEffect(() => { if (tf.difficulty) reload() }, [tags])
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
      <CompareTray comps={data.rows} />
    </>
  )
}
