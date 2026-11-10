import { CaretLeft } from '@phosphor-icons/react'
import { useState } from 'react'
import { count, num, pct, useApi, useFilters, type Entity, type Rates, type Scope } from '../api'
import { ValueFlash } from '../interior/value-flash'
import { Change, Empty, Fact, Games, GradeChip, Icon, rateCols, Search, Segmented, Status, Table } from '../parts'
import { gradeByChange } from '../tiers'

type ItemRow = Entity & Rates & { placeChange: number | null; holders: Entity[] }

const TYPES: [string, string][] = [
  ['craftable', 'Craftable'], ['radiant', 'Radiant'], ['artifact', 'Artifact'], ['emblem', 'Emblem'],
  ['support', 'Support'], ['component', 'Component'], ['other', 'Other'],
]

export function Items() {
  const f = useFilters()
  const { data, error, loading } = useApi<Scope & { rows: ItemRow[] }>('/api/items', f)
  const [search, setSearch] = useState('')
  const [type, setType] = useState('')
  if (error || !data) return <Status error={error} loading={loading} />
  const present = TYPES.filter(([k]) => data.rows.some((r) => r.type === k))
  const rows = data.rows.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase()) && (!type || r.type === type))
  return (
    <>
      <div className="toolbar">
        <Search value={search} onChange={setSearch} placeholder="Search items" />
        {present.length > 1 ? (
          <Segmented label="Item type" value={type} onChange={setType}
            options={[{ value: '', label: 'All' }, ...present.map(([value, label]) => ({ value, label }))]} />
        ) : null}
        <span className="toolbar-note">{count(rows.length, 'item')}</span>
      </div>
      {rows.length ? (
        <Table label="Items" rows={rows} initial="change" cols={[
          { key: 'name', label: 'Item', sort: (r) => r.name, render: (r) => <a className="name" href={`#/items/${encodeURIComponent(r.id)}`}><Icon e={r} plain /><span>{r.name}</span></a> },
          { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByChange(r.placeChange ?? 0)} />, width: '60px' },
          { key: 'change', label: 'Place change', sort: (r) => r.placeChange ?? 0, render: (r) => <Change value={r.placeChange} />, align: 'right' },
          ...rateCols<ItemRow>(),
          { key: 'holders', label: 'Most used on', render: (r) => <span className="holders">{r.holders.map((h) => <a key={h.id} href={`#/units/${encodeURIComponent(h.id)}`}><Icon e={h} size={26} /></a>)}</span>, width: '170px' },
        ]} />
      ) : <Empty scope={data} what="No item" />}
    </>
  )
}

type Detail = Scope & {
  item: Entity & Partial<Rates> & { placeChange?: number | null; text?: string }
  recipe: Entity[]
  prev: { patch: string; avgPlace: number; frequency: number } | null
  holders: (Entity & Rates & { placeChange: number | null })[]
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
