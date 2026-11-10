import { CaretDown, EyeSlash, PushPin } from '@phosphor-icons/react'
import { motion, useReducedMotion } from 'motion/react'
import { useEffect, useMemo, useState } from 'react'
import { count, pct, useApi, useFilters, useMarks, type Comp, type Entity, type Marks, type Scope } from '../api'
import { useAccordion, useAutoHeight, type AccordionHeaderProps, type AccordionPanelProps } from '../interior/accordion'
import { ContextMenu, type ContextMenuItem } from '../interior/context-menu'
import { CopyButton } from '../interior/copy-button'
import { Change, Dist, Empty, FavButton, GradeChip, Icon, IconButton, PlaceRail, Search, Select, Status, Tag, TraitChip, UnitChip } from '../parts'
import { gradeComp } from '../tiers'

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
          <GradeChip grade={gradeComp(comp.avgPlace)} />
          <div className="comp-id">
            <span className="comp-name">{comp.name}</span>
            <span className="comp-tags">
              <Tag kind={comp.play.kind}>{comp.play.label}</Tag>
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
  { value: 'place', label: 'Avg place', get: (c: Comp) => c.avgPlace },
  { value: 'pick', label: 'Pick rate', get: (c: Comp) => -c.pickRate },
  { value: 'win', label: 'Win rate', get: (c: Comp) => -c.winRate },
]
const OPEN_KEY = 'tft-local-open-comp'

export function Comps() {
  const f = useFilters()
  const marks = useMarks()
  const { data, error, loading } = useApi<Scope & Marks & { rows: Comp[] }>('/api/comps', f)
  const [search, setSearch] = useState('')
  const [sort, setSort] = useState('place')
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
      setSearch(''); setFavOnly(false); setOpen([id])
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
      {rows.length ? (
        <ul className="comps">
          {rows.map((c) => (
            <CompRow key={c.id} comp={c} open={acc.isOpen(c.id)} onToggle={() => acc.toggle(c.id)} header={acc.headerProps(c.id)} panel={acc.panelProps(c.id)} exitOnHide={!showHidden} />
          ))}
        </ul>
      ) : <Empty scope={data} what="No comp" />}
    </>
  )
}
