import { CaretLeft } from '@phosphor-icons/react'
import { useState } from 'react'
import { count, num, pct, useApi, useFilters, type BoardUnit, type Entity, type Rates, type Scope } from '../api'
import { Tabs } from '../interior/tabs'
import { ValueFlash } from '../interior/value-flash'
import { Change, Dist, Empty, Fact, Games, GradeChip, Icon, PlaceRail, rateCols, Search, Segmented, Status, Table, Tag, UnitChip } from '../parts'
import { gradeByChange, gradeByPlace } from '../tiers'

type UnitRow = Entity & Rates & { avgStar: number }
const unitLink = (r: Entity) => `#/units/${encodeURIComponent(r.id)}`
const itemLink = (r: Entity) => `#/items/${encodeURIComponent(r.id)}`

export function Units() {
  const f = useFilters()
  const { data, error, loading } = useApi<Scope & { rows: UnitRow[] }>('/api/units', f)
  const [search, setSearch] = useState('')
  const [cost, setCost] = useState(0)
  if (error || !data) return <Status error={error} loading={loading} />
  const costs = [...new Set(data.rows.map((r) => r.cost).filter((c): c is number => !!c))].sort()
  const rows = data.rows.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase()) && (!cost || r.cost === cost))
  return (
    <>
      <div className="toolbar">
        <Search value={search} onChange={setSearch} placeholder="Search units" />
        {costs.length ? (
          <Segmented label="Cost" value={cost} onChange={setCost}
            options={[{ value: 0, label: 'All' }, ...costs.map((c) => ({ value: c, label: `${c} cost` }))]} />
        ) : null}
        <span className="toolbar-note">{count(rows.length, 'unit')}</span>
      </div>
      {rows.length ? (
        <Table label="Units" rows={rows} initial="place" cols={[
          { key: 'name', label: 'Unit', sort: (r) => r.name, render: (r) => <a className="name" href={unitLink(r)}><Icon e={r} /><span>{r.name}</span></a> },
          { key: 'cost', label: 'Cost', sort: (r) => r.cost ?? 0, render: (r) => r.cost ? <span className={`cost-text-${r.cost}`}>{r.cost}</span> : <span className="muted">–</span>, align: 'right', width: '70px' },
          { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByPlace(r.avgPlace)} />, width: '60px' },
          ...rateCols<UnitRow>(),
          { key: 'star', label: 'Avg stars', sort: (r) => r.avgStar, render: (r) => r.avgStar.toFixed(2), align: 'right' },
        ]} />
      ) : <Empty scope={data} what="No unit" />}
    </>
  )
}

type ItemStat = Entity & Rates & { placeChange: number | null }
type BuildStat = { id: string; items: Entity[] } & Rates & { placeChange: number | null }
type Detail = Scope & {
  unit: Entity & Partial<Rates> & {
    avgStar?: number; role?: string | null
    ability?: { name: string; text: string } | null
    stats?: Record<string, number | null> | null
  }
  prev: { patch: string; avgPlace: number; frequency: number } | null
  stars: { value: number; games: number; share: number; avgPlace: number }[]
  itemCounts: { value: number; games: number; share: number; avgPlace: number }[]
  items: ItemStat[]
  builds: BuildStat[]
  comps: { id: string; name: string; games: number; avgPlace: number; pickRate: number; units: BoardUnit[]; play: { kind: string; label: string } }[]
  trend: ({ patch: string } & Rates)[]
}

const STAT_LABELS: [string, string][] = [
  ['hp', 'Health'], ['mana', 'Mana'], ['damage', 'Attack damage'], ['attackSpeed', 'Attack speed'],
  ['armor', 'Armor'], ['magicResist', 'Magic resist'], ['range', 'Range'],
]

export function Unit({ id }: { id: string }) {
  const f = useFilters()
  const { data, error, loading } = useApi<Detail>(`/api/units/${encodeURIComponent(id)}`, f)
  const [tab, setTab] = useState('builds')
  if (error || !data) return <Status error={error} loading={loading} />
  const u = data.unit
  const played = !!u.games

  // "Best" needs enough games to mean something; "most played" is simply the top of the list.
  const solid = data.builds.filter((b) => b.games >= Math.max(f.min, 20))
  const best = [...solid].sort((a, b) => (a.placeChange ?? 0) - (b.placeChange ?? 0))[0]
  const popular = [...data.builds].sort((a, b) => b.games - a.games)[0]
  const emptyHint = `Nothing with ${num(f.min)} games yet. Lower the minimum games.`

  const panels: Record<string, React.ReactNode> = {
    builds: (
      <Table flush label="Three-item builds" rows={data.builds} initial="games" desc empty={emptyHint} cols={[
        { key: 'build', label: 'Build', render: (r) => <span className="holders">{r.items.map((i, n) => <Icon key={n} e={i} size={30} plain />)}</span> },
        { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByChange(r.placeChange ?? 0)} />, width: '60px' },
        { key: 'change', label: 'Place change', sort: (r) => r.placeChange ?? 0, render: (r) => <Change value={r.placeChange} />, align: 'right' },
        { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => r.avgPlace.toFixed(2), align: 'right' },
        { key: 'win', label: 'Win rate', sort: (r) => r.winRate, render: (r) => pct(r.winRate), align: 'right' },
        { key: 'games', label: 'Games', sort: (r) => r.games, align: 'right', render: (r) => <Games r={r} /> },
      ]} />
    ),
    items: (
      <Table flush label="Items" rows={data.items} initial="change" empty={emptyHint} cols={[
        { key: 'name', label: 'Item', sort: (r) => r.name, render: (r) => <a className="name" href={itemLink(r)}><Icon e={r} size={28} plain /><span>{r.name}</span></a> },
        { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByChange(r.placeChange ?? 0)} />, width: '60px' },
        { key: 'change', label: 'Place change', sort: (r) => r.placeChange ?? 0, render: (r) => <Change value={r.placeChange} />, align: 'right' },
        { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => r.avgPlace.toFixed(2), align: 'right' },
        { key: 'win', label: 'Win rate', sort: (r) => r.winRate, render: (r) => pct(r.winRate), align: 'right' },
        { key: 'games', label: 'Games', sort: (r) => r.games, align: 'right', render: (r) => <Games r={r} /> },
      ]} />
    ),
    comps: (
      <Table flush label="Comps with this unit" rows={data.comps} initial="games" desc rowHeight={60} empty="This unit is not a regular in any comp at these filters." cols={[
        { key: 'name', label: 'Comp', sort: (r) => r.name, render: (r) => <span className="name"><span>{r.name}</span> <Tag kind={r.play.kind}>{r.play.label}</Tag></span>, width: 'minmax(220px, 1.4fr)' },
        { key: 'units', label: 'Board', render: (r) => <span className="mini-board flex-nowrap gap-2">{r.units.map((x) => <Icon key={x.id} e={x} size={26} />)}</span>, width: 'minmax(300px, 2fr)' },
        { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => <PlaceRail value={r.avgPlace} />, width: '150px' },
        { key: 'pick', label: 'Pick rate', sort: (r) => r.pickRate, render: (r) => r.pickRate.toFixed(2), align: 'right', width: '110px' },
        { key: 'games', label: 'Games', sort: (r) => r.games, render: (r) => num(r.games), align: 'right', width: '90px' },
      ]} />
    ),
    trend: (
      <Table flush label="By patch" rows={data.trend.map((t) => ({ ...t, id: t.patch }))} initial="patch" desc cols={[
        { key: 'patch', label: 'Patch', sort: (r) => r.patch, render: (r) => r.patch },
        { key: 'place', label: 'Avg place', render: (r) => <PlaceRail value={r.avgPlace} />, width: '150px' },
        { key: 'win', label: 'Win rate', render: (r) => pct(r.winRate), align: 'right' },
        { key: 'freq', label: 'Played in', render: (r) => pct(r.frequency), align: 'right' },
        { key: 'games', label: 'Games', render: (r) => num(r.games), align: 'right' },
      ]} />
    ),
  }

  return (
    <>
      <a className="back" href="#/units"><CaretLeft size={12} /> All units</a>
      <header className="detail-head">
        <Icon e={u} size={68} />
        <div className="detail-title">
          <h1>{u.name}</h1>
          <p className="muted">
            {u.cost ? <span className={`cost-text-${u.cost}`}>{u.cost} cost</span> : null}
            {[u.role, ...(u.traits ?? [])].filter(Boolean).map((t) => <span key={t!}>{t}</span>)}
            {!u.cost && !(u.traits ?? []).length ? <span>{u.id}</span> : null}
          </p>
        </div>
        {played ? (
          <dl className="facts">
            <Fact label="Avg place" sub={data.prev ? <Change value={u.avgPlace! - data.prev.avgPlace} suffix={`vs ${data.prev.patch}`} /> : null}>
              <ValueFlash className="!px-0 !py-0 !text-[22px] !font-semibold tracking-[-0.02em]" value={u.avgPlace!} format={(v) => v.toFixed(2)} label="Average place" />
            </Fact>
            <Fact label="Win rate"><ValueFlash className="!px-0 !py-0 !text-[22px] !font-semibold tracking-[-0.02em]" value={u.winRate! * 100} format={(v) => `${v.toFixed(1)}%`} label="Win rate" /></Fact>
            <Fact label="Top 4"><ValueFlash className="!px-0 !py-0 !text-[22px] !font-semibold tracking-[-0.02em]" value={u.top4Rate! * 100} format={(v) => `${v.toFixed(1)}%`} label="Top 4 rate" /></Fact>
            <Fact label="Played in" sub={data.prev ? <span className="muted small">{pct(data.prev.frequency)} in {data.prev.patch}</span> : null}>{pct(u.frequency!)}</Fact>
          </dl>
        ) : null}
      </header>

      {u.ability || u.stats ? (
        <div className="cards">
          {u.ability ? (
            <section className="card grow">
              <h3>{u.ability.name}</h3>
              <p className="ability">{u.ability.text || 'No description in the downloaded data.'}</p>
              <p className="muted small mt-2">Numbers separated by slashes are the one-, two- and three-star values.</p>
            </section>
          ) : null}
          {u.stats ? (
            <section className="card">
              <h3>Base stats</h3>
              <dl className="kv">
                {STAT_LABELS.filter(([k]) => u.stats![k] != null).map(([k, label]) => (
                  <div key={k}><dt>{label}</dt><dd>{k === 'mana' && u.stats!.initialMana != null ? `${u.stats!.initialMana} / ${u.stats![k]}` : Number(u.stats![k]).toFixed(k === 'attackSpeed' ? 2 : 0)}</dd></div>
                ))}
              </dl>
            </section>
          ) : null}
        </div>
      ) : null}

      {!played ? <Empty scope={data} what="No data for this unit" /> : (
        <>
          <div className="cards">
            <section className="card grow">
              <h3>Star level</h3>
              <Dist rows={data.stars.map((s) => ({ label: '★'.repeat(s.value), share: s.share, avgPlace: s.avgPlace, games: s.games }))} />
            </section>
            <section className="card grow">
              <h3>Items held</h3>
              <Dist rows={[...data.itemCounts].reverse().map((s) => ({ label: s.value === 1 ? '1 item' : `${s.value} items`, share: s.share, avgPlace: s.avgPlace, games: s.games }))} />
            </section>
          </div>

          {best || popular ? (
            <p className="recommend">
              {best ? <>Best build with enough games: <span className="inline-icons">{best.items.map((i, n) => <Icon key={n} e={i} size={22} plain />)}</span> <Change value={best.placeChange} />. </> : null}
              {popular && popular.id !== best?.id ? <>Most played: <span className="inline-icons">{popular.items.map((i, n) => <Icon key={n} e={i} size={22} plain />)}</span> in {num(popular.games)} games.</> : null}
            </p>
          ) : null}

          <Tabs label={`${u.name} details`} value={tab} onValueChange={setTab}
            items={[{ value: 'builds', label: 'Builds' }, { value: 'items', label: 'Items' }, { value: 'comps', label: 'Comps' }, { value: 'trend', label: 'By patch' }]}
            renderPanel={(value) => panels[value]} />
        </>
      )}
    </>
  )
}
