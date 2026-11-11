import { CaretLeft } from '@phosphor-icons/react'
import { useState } from 'react'
import { count, num, pct, useApi, useFilters, type Entity, type Rates, type Scope } from '../api'
import { ValueFlash } from '../interior/value-flash'
import { Change, Empty, Fact, Games, GradeChip, Icon, rateCols, Search, Segmented, Status, Table } from '../parts'
import { gradeByChange } from '../tiers'

type Holder = Entity & { games: number; placeChange: number | null }
type ItemRow = Entity & Rates & { placeChange: number | null; holders: Entity[]; bestHolders?: Holder[] }

/** The type tabs (PRD 3 U6). Always shown, so a tab with no items says so instead of disappearing. Components and
 * other items are under All. */
const TABS: [string, string][] = [
  ['craftable', 'Craftable'], ['radiant', 'Radiant'], ['artifact', 'Artifact'], ['support', 'Support'], ['emblem', 'Emblem'],
]
const COMPONENTS = 'components'

export function Items() {
  const f = useFilters()
  const [type, setType] = useState('')
  // A type tab asks for that type only: it also hides items gone from the set data on the newest patch.
  const { data, error, loading } = useApi<Scope & { rows: ItemRow[] }>(type === COMPONENTS ? null : type ? `/api/items?type=${type}` : '/api/items', f)
  const [search, setSearch] = useState('')
  const tabs = (
    <Segmented label="Item type" value={type} onChange={setType}
      options={[{ value: '', label: 'All' }, ...TABS.map(([value, label]) => ({ value, label })), { value: COMPONENTS, label: 'Components' }]} />
  )
  if (type === COMPONENTS) return <><div className="toolbar">{tabs}</div><Components /></>
  if (error || !data) return <><div className="toolbar">{tabs}</div><Status error={error} loading={loading} /></>
  const rows = data.rows.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase()) && (!type || r.type === type))
  const tabName = TABS.find(([k]) => k === type)?.[1]
  return (
    <>
      <div className="toolbar">
        <Search value={search} onChange={setSearch} placeholder="Search items" />
        {tabs}
        <span className="toolbar-note">{count(rows.length, 'item')}</span>
      </div>
      {rows.length ? (
        <Table label="Items" rows={rows} initial="change" cols={[
          { key: 'name', label: 'Item', sort: (r) => r.name, render: (r) => <a className="name" href={`#/items/${encodeURIComponent(r.id)}`}><Icon e={r} plain /><span>{r.name}</span></a> },
          { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByChange(r.placeChange ?? 0)} />, width: '60px' },
          { key: 'change', label: 'Place change', sort: (r) => r.placeChange ?? 0, render: (r) => <Change value={r.placeChange} />, align: 'right' },
          ...rateCols<ItemRow>(),
          { key: 'best', label: 'Best holders', render: (r) => <HolderIcons hs={r.bestHolders ?? []} />, width: '170px' },
          { key: 'holders', label: 'Most used on', render: (r) => <span className="holders">{r.holders.map((h) => <a key={h.id} href={`#/units/${encodeURIComponent(h.id)}`}><Icon e={h} size={26} /></a>)}</span>, width: '170px' },
        ]} />
      ) : search.trim() || !tabName ? <Empty scope={data} what="No item" /> : <Empty scope={data} what={`No ${tabName.toLowerCase()} item`} />}
    </>
  )
}

/** Up to five units, best place change first. The tooltip says by how much. */
function HolderIcons({ hs }: { hs: Holder[] }) {
  if (!hs.length) return <span className="muted small">Too few games</span>
  return (
    <span className="holders">
      {hs.map((h) => (
        <a key={h.id} href={`#/units/${encodeURIComponent(h.id)}`}
          title={`${h.name}: ${h.placeChange == null ? 'no' : `${h.placeChange > 0 ? '+' : ''}${h.placeChange.toFixed(2)}`} place change over ${num(h.games)} games`}>
          <Icon e={h} size={26} />
        </a>
      ))}
    </span>
  )
}

type Build = Entity & { weight: number; games: number }
type ComponentRow = Entity & { best: number; items: Build[] }

/** PRD 3 U5: what each component builds, best first, weighted by how those items do on the S and A comps. */
function Components() {
  const f = useFilters()
  const { data, error, loading } = useApi<Scope & { basis: 'top' | 'all'; tiers: string[]; rows: ComponentRow[] }>('/api/items/components', f)
  if (error || !data) return <Status error={error} loading={loading} />
  if (!data.rows.length) {
    if (!data.patches.length) return <Empty scope={data} />
    return (
      <div className="empty">
        <h2>No component data for these filters</h2>
        <p>Recipes come from the set data. Run <code>npm run refresh</code>, then <code>npm run stats</code>. If that is done, lower the minimum games or pick a wider rank filter.</p>
      </div>
    )
  }
  const tiers = data.tiers.join(' and ')
  return (
    <>
      <p className="muted small mb-3">
        {data.basis === 'top'
          ? `Each item is ranked by its place change on the units of ${tiers} tier comps. Negative is better. Take the component whose best item fits your board.`
          : `No ${tiers} tier comps are listed for these filters, so items are ranked by place change across all boards. Negative is better.`}
      </p>
      <div className="component-grid">
        {data.rows.map((c) => (
          <section key={c.id} className="card component-card" aria-label={c.name}>
            <h3 className="component-head"><Icon e={c} size={30} plain /><span>{c.name}</span></h3>
            <p className="component-key muted small"><span /><span>Place change</span><span>Games</span></p>
            <ol className="component-items">
              {c.items.map((it) => (
                <li key={it.id}>
                  <a className="name" href={`#/items/${encodeURIComponent(it.id)}`}><Icon e={it} size={24} plain /><span>{it.name}</span></a>
                  <Change value={it.weight} />
                  <span className="muted small component-games">{num(it.games)}</span>
                </li>
              ))}
            </ol>
          </section>
        ))}
      </div>
    </>
  )
}

type EmblemLevel = { level: number; units: number | null; games: number; wins: number; top4: number; avgPlace: number; winRate: number; top4Rate: number }
type Emblem = { trait: Entity | null; basis: 'wins' | 'top4'; breakpoint: EmblemLevel | null; levels: EmblemLevel[] }
type Detail = Scope & {
  item: Entity & Partial<Rates> & { placeChange?: number | null; text?: string }
  recipe: Entity[]
  prev: { patch: string; avgPlace: number; frequency: number } | null
  holders: (Entity & Rates & { placeChange: number | null })[]
  emblem?: Emblem
}

export function Item({ id }: { id: string }) {
  const f = useFilters()
  const { data, error, loading } = useApi<Detail>(`/api/items/${encodeURIComponent(id)}`, f)
  if (error || !data) return <Status error={error} loading={loading} />
  const it = data.item
  return (
    <>
      <a className="back" href="#/items"><CaretLeft size={12} /> All items</a>
      <header className="detail-head">
        <Icon e={it} size={68} plain />
        <div className="detail-title">
          <h1>{it.name}</h1>
          <p className="muted">
            {data.recipe.length
              ? <span className="recipe">Recipe {data.recipe.map((r, n) => <Icon key={n} e={r} size={22} plain />)}</span>
              : <span>Cannot be crafted</span>}
          </p>
        </div>
        {it.games ? (
          <dl className="facts">
            <Fact label="Avg place" sub={data.prev ? <Change value={it.avgPlace! - data.prev.avgPlace} suffix={`vs ${data.prev.patch}`} /> : null}>
              <ValueFlash className="!px-0 !py-0 !text-[22px] !font-semibold tracking-[-0.02em]" value={it.avgPlace!} format={(v) => v.toFixed(2)} label="Average place" />
            </Fact>
            <Fact label="Place change"><Change value={it.placeChange} /></Fact>
            <Fact label="Win rate">{pct(it.winRate!)}</Fact>
            <Fact label="On boards" sub={data.prev ? <span className="muted small">{pct(data.prev.frequency)} in {data.prev.patch}</span> : null}>{pct(it.frequency!)}</Fact>
          </dl>
        ) : null}
      </header>
      {it.text ? <p className="ability item-text">{it.text}</p> : null}

      {data.emblem && it.games ? <EmblemBreakpoints e={data.emblem} min={f.min} /> : null}
      {!it.games ? <Empty scope={data} what="No data for this item" /> : (
        <>
          <h3>Best holders</h3>
          <p className="muted small mb-3">Place change compares each unit holding this item with that unit overall. Negative is better.</p>
          <Table label="Best holders" rows={data.holders} initial="change"
            empty={`No unit has ${num(f.min)} games with this item yet. Lower the minimum games.`} cols={[
              { key: 'name', label: 'Unit', sort: (r) => r.name, render: (r) => <a className="name" href={`#/units/${encodeURIComponent(r.id)}`}><Icon e={r} size={28} /><span>{r.name}</span></a> },
              { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByChange(r.placeChange ?? 0)} />, width: '60px' },
              { key: 'change', label: 'Place change', sort: (r) => r.placeChange ?? 0, render: (r) => <Change value={r.placeChange} />, align: 'right' },
              { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => r.avgPlace.toFixed(2), align: 'right' },
              { key: 'win', label: 'Win rate', sort: (r) => r.winRate, render: (r) => pct(r.winRate), align: 'right' },
              { key: 'games', label: 'Games', sort: (r) => r.games, align: 'right', render: (r) => <Games r={r} /> },
            ]} />
        </>
      )}
    </>
  )
}

/** PRD 3 U7: the trait level an emblem's trait reaches on the boards that hold it. */
function EmblemBreakpoints({ e, min }: { e: Emblem; min: number }) {
  const winning = e.basis === 'wins' ? 'winning boards (1st place)' : 'top 4 boards'
  const levelName = (l: EmblemLevel) => (l.level === 0 ? 'Not active' : l.units == null ? `Level ${l.level}` : `${l.units} units`)
  return (
    <>
      <h3>{e.trait ? `${e.trait.name} breakpoint` : 'Emblem breakpoint'}</h3>
      {!e.trait ? (
        <p className="muted small mb-3">The set data does not say which trait this emblem gives. Run <code>npm run refresh</code>, then <code>npm run stats</code>.</p>
      ) : !e.breakpoint ? (
        <p className="muted small mb-3">No board holding this emblem has finished in the top 4 with {e.trait.name} active yet. Check again after more matches are collected.</p>
      ) : (
        <>
          <p className="emblem-answer">
            Most often completes <strong>{levelName(e.breakpoint)}</strong> on {winning}
            <span className="muted"> · {num(e.basis === 'wins' ? e.breakpoint.wins : e.breakpoint.top4)} of {num(e.levels.filter((l) => l.level > 0).reduce((n, l) => n + (e.basis === 'wins' ? l.wins : l.top4), 0))}</span>
          </p>
          {e.basis === 'top4' ? <p className="muted small mb-3">Fewer than {num(min)} winning boards hold this emblem, so this uses top 4 boards instead.</p> : null}
        </>
      )}
      {e.levels.length ? (
        <Table label="Emblem breakpoints" rows={e.levels.map((l) => ({ ...l, id: String(l.level) }))} initial="level" cols={[
          { key: 'level', label: e.trait ? e.trait.name : 'Trait', sort: (r) => r.level, render: (r) => levelName(r) },
          { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => r.avgPlace.toFixed(2), align: 'right' },
          { key: 'win', label: 'Win rate', sort: (r) => r.winRate, render: (r) => pct(r.winRate), align: 'right' },
          { key: 'top4', label: 'Top 4', sort: (r) => r.top4Rate, render: (r) => pct(r.top4Rate), align: 'right' },
          { key: 'games', label: 'Games', sort: (r) => r.games, render: (r) => num(r.games), align: 'right' },
        ]} />
      ) : null}
    </>
  )
}
