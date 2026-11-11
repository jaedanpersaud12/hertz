import { CaretLeft } from '@phosphor-icons/react'
import { useState } from 'react'
import { count, num, pct, useApi, useFilters, type BoardUnit, type Entity, type Rates, type Scope } from '../api'
import { Tabs } from '../interior/tabs'
import { ValueFlash } from '../interior/value-flash'
import { Button, Change, Dist, Empty, Fact, Games, GradeChip, Icon, PlaceRail, placeTone, rateCols, Search, Segmented, Select, Status, Table, Tag, UnitChip } from '../parts'
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
  /** PRD 3 U1: star 1 to 3 by 0 to 3 items, all twelve cells. */
  grid: GridCell[]
  /** PRD 3 U4: the units it places best alongside (200 shared boards or more), place change against its own average. */
  teammates: (Entity & { games: number; avgPlace: number; placeChange: number })[]
  /** The last 3 days of this patch, for the trend (null when the window is already 3 days, or nothing was played). */
  recent: ({ window: string; label: string } & Rates) | null
  /** False for summons and units that cannot hold items: no builds or item pairs. */
  holdsItems: boolean
}
type GridCell = { star: number; items: number; games: number; avgPlace: number | null; top4Rate: number | null }
type BuildRow = { id: string; items: Entity[]; games: number; share: number; avgPlace: number; placeChange: number | null }
type Builds = Scope & { comp: string | null; compGames: number; minCompGames: number; fallback: boolean; holdsItems: boolean; rows: BuildRow[] }
type PairRow = Entity & { games: number; avgPlace: number; placeChange: number; copies: number }
type Pairs = Scope & { have: Entity[]; base: { games: number; avgPlace: number; placeChange: number } | null; holdsItems: boolean; rows: PairRow[] }

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
    builds: <BuildsPanel id={id} comps={data.comps} holds={data.holdsItems} />,
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
    teammates: (
      <Table flush label="Teammates" rows={data.teammates} initial="change" cols={[
        { key: 'name', label: 'Unit', sort: (r) => r.name, render: (r) => <a className="name" href={unitLink(r)}><Icon e={r} size={28} /><span>{r.name}</span></a> },
        { key: 'change', label: 'Place change', sort: (r) => r.placeChange, render: (r) => <Change value={r.placeChange} />, align: 'right' },
        { key: 'place', label: 'Avg place together', sort: (r) => r.avgPlace, render: (r) => <PlaceRail value={r.avgPlace} />, width: '170px' },
        { key: 'games', label: 'Shared boards', sort: (r) => r.games, render: (r) => num(r.games), align: 'right' },
      ]} empty="No unit shares 200 boards with this one at these filters. Pick a wider rank filter or the whole patch." />
    ),
    trend: (
      <Table flush label="By patch" rows={[
        ...(data.recent ? [{ ...data.recent, patch: `${data.patch}, last 3 days`, id: 'recent' }] : []),
        ...data.trend.map((t) => ({ ...t, id: t.patch })),
      ]} initial="patch" desc cols={[
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
            <StarGrid cells={data.grid} min={f.min} />
          </div>

          <NextItem id={id} holds={data.holdsItems} />

          {best || popular ? (
            <p className="recommend">
              {best ? <>Best build with enough games: <span className="inline-icons">{best.items.map((i, n) => <Icon key={n} e={i} size={22} plain />)}</span> <Change value={best.placeChange} />. </> : null}
              {popular && popular.id !== best?.id ? <>Most played: <span className="inline-icons">{popular.items.map((i, n) => <Icon key={n} e={i} size={22} plain />)}</span> in {num(popular.games)} games.</> : null}
            </p>
          ) : null}

          <Tabs label={`${u.name} details`} value={tab} onValueChange={setTab}
            items={[{ value: 'builds', label: 'Builds' }, { value: 'items', label: 'Items' }, { value: 'teammates', label: 'Teammates' }, { value: 'comps', label: 'Comps' }, { value: 'trend', label: 'By patch' }]}
            renderPanel={(value) => panels[value]} />
        </>
      )}
    </>
  )
}

/** U1: average place and games for each star level and item count. Cells under the minimum games are dimmed. */
function StarGrid({ cells, min }: { cells: GridCell[]; min: number }) {
  if (!cells.length) return null
  return (
    <section className="card grow">
      <h3>Stars by items</h3>
      <table className="star-grid" aria-label="Average place by star level and items held">
        <thead><tr><th scope="col"><span className="sr-only">Star level</span></th>{[0, 1, 2, 3].map((n) => <th key={n} scope="col">{n === 1 ? '1 item' : `${n} items`}</th>)}</tr></thead>
        <tbody>
          {[1, 2, 3].map((star) => (
            <tr key={star}>
              <th scope="row" aria-label={`${star} star`}>{'★'.repeat(star)}</th>
              {[0, 1, 2, 3].map((n) => {
                const c = cells.find((x) => x.star === star && x.items === n)
                const thin = !c || c.games < min
                return (
                  <td key={n} className={thin ? 'thin' : undefined} title={c?.games ? `${num(c.games)} games${thin ? ', under the minimum' : ''}` : 'Not played'}>
                    {c && c.avgPlace != null ? <><b className={`tone-${placeTone(c.avgPlace)}`}>{c.avgPlace.toFixed(2)}</b><span className="muted small">{num(c.games)}</span></> : <span className="muted">–</span>}
                  </td>
                )
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  )
}

/** U2: three-item builds by place change, on all boards or in one comp. A comp where the unit held three items on
 * fewer than 50 boards falls back to all boards, and says so. */
function BuildsPanel({ id, comps, holds }: { id: string; comps: Detail['comps']; holds: boolean }) {
  const f = useFilters()
  const [comp, setComp] = useState('')
  const { data, error, loading } = useApi<Builds>(`/api/units/${encodeURIComponent(id)}/builds${comp ? `?comp=${encodeURIComponent(comp)}` : ''}`, f)
  if (!holds) return <p className="muted px-4 py-6">This unit cannot hold items, so it has no builds.</p>
  const picker = comps.length ? (
    <div className="panel-bar">
      <Select label="Comp" prefix="Boards:" value={comp} onChange={setComp}
        options={[{ value: '', label: 'All boards' }, ...comps.map((c) => ({ value: c.id, label: c.name, hint: `${num(c.games)} games` }))]} />
      {data?.fallback ? <span className="muted small">Under {num(data.minCompGames)} games with three items in this comp ({num(data.compGames)}), so these builds are from all boards.</span> : null}
    </div>
  ) : null
  if (error || !data) return <>{picker}<div className="px-4 py-4"><Status error={error} loading={loading} /></div></>
  return (
    <>
      {picker}
      <Table flush label="Three-item builds" rows={data.rows} initial="change" empty={`No build has ${num(f.min)} games yet. Lower the minimum games${comp ? ', or pick all boards' : ''}.`} cols={[
        { key: 'build', label: 'Build', render: (r) => <span className="holders">{r.items.map((i, n) => <Icon key={n} e={i} size={30} plain />)}</span> },
        { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByChange(r.placeChange ?? 0)} />, width: '60px' },
        { key: 'change', label: 'Place change', sort: (r) => r.placeChange ?? 0, render: (r) => <Change value={r.placeChange} />, align: 'right' },
        { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => r.avgPlace.toFixed(2), align: 'right' },
        { key: 'games', label: 'Games', sort: (r) => r.games, align: 'right', render: (r) => <>{num(r.games)} <span className="muted small">{pct(r.share)}</span></> },
      ]} />
    </>
  )
}

/** U3: pick up to two items the unit already holds; the list shows the best item to add. Each pick is one click. */
function NextItem({ id, holds }: { id: string; holds: boolean }) {
  const f = useFilters()
  const [have, setHave] = useState<string[]>([])
  const { data, error, loading } = useApi<Pairs>(holds ? `/api/units/${encodeURIComponent(id)}/pairs${have.length ? `?have=${have.map(encodeURIComponent).join(',')}` : ''}` : null, f)
  if (!holds) return null
  const step = have.length === 0 ? 'Pick the first item it holds' : have.length === 1 ? 'Pick the second item it holds' : 'Best third item'
  const full = have.length >= 2
  return (
    <section className="card next-item">
      <div className="next-head">
        <h3>Item completer</h3>
        <span className="held" aria-live="polite">
          {have.map((h, n) => {
            const e = data?.have.find((x) => x.id === h) ?? { id: h, name: h, icon: null }
            return <Button key={`${h}-${n}`} size="sm" onClick={() => setHave(have.filter((_, i) => i !== n))} aria-label={`Remove ${e.name}`}><Icon e={e} size={18} plain /> {e.name} ×</Button>
          })}
          {have.length ? <Button size="sm" variant="ghost" onClick={() => setHave([])}>Start over</Button> : null}
        </span>
        <span className="muted small next-step">{step}{data?.base ? <>: holding these, avg place {data.base.avgPlace.toFixed(2)} (<Change value={data.base.placeChange} />) in {num(data.base.games)} games</> : null}</span>
      </div>
      {error || !data ? <Status error={error} loading={loading} /> : (
        <Table flush label={step} rows={data.rows} initial="change" rowHeight={44} minWidth={480}
          empty={have.length ? `Nothing to add with ${num(f.min)} games yet. Remove an item, or lower the minimum games.` : `No item has ${num(f.min)} games on this unit yet. Lower the minimum games.`} cols={[
          { key: 'name', label: full ? 'Add as third item' : 'Item', sort: (r) => r.name, render: (r) => full
            ? <span className="name"><Icon e={r} size={26} plain /><span>{r.name}{r.copies > 1 ? <b className="copies"> x{r.copies}</b> : null}</span></span>
            : <button type="button" className="name pick" onClick={() => setHave([...have, r.id])} title={`Add ${r.name} to the items it holds`}><Icon e={r} size={26} plain /><span>{r.name}{r.copies > 1 ? <b className="copies"> x{r.copies}</b> : null}</span></button> },
          { key: 'change', label: 'Place change', sort: (r) => r.placeChange, render: (r) => <Change value={r.placeChange} />, align: 'right' },
          { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => r.avgPlace.toFixed(2), align: 'right' },
          { key: 'games', label: 'Games', sort: (r) => r.games, render: (r) => num(r.games), align: 'right' },
        ]} />
      )}
    </section>
  )
}
