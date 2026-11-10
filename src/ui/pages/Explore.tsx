import { X } from '@phosphor-icons/react'
import { useState } from 'react'
import { num, pct, useApi, useFilters, type Entity, type Rates, type Scope } from '../api'
import { Change, Fact, Icon, Status, Table } from '../parts'

export type StaticData = {
  loaded: boolean
  units: Entity[]
  traits: (Entity & { breakpoints?: { units: number; style: number }[] })[]
  items: Entity[]
}

/** Type-to-search multi-select over a fixed list of units or traits. */
export function Picker({ options, selected, onChange, placeholder, max, kind }: {
  options: Entity[]; selected: string[]; onChange: (ids: string[]) => void; placeholder: string; max: number; kind: 'unit' | 'trait'
}) {
  const [q, setQ] = useState('')
  const [open, setOpen] = useState(false)
  const byId = new Map(options.map((o) => [o.id, o]))
  const matches = options.filter((o) => !selected.includes(o.id) && o.name.toLowerCase().includes(q.trim().toLowerCase())).slice(0, 8)
  const add = (id: string) => { onChange([...selected, id]); setQ(''); setOpen(false) }
  return (
    <div className="picker">
      {selected.map((id) => {
        const e = byId.get(id) ?? { id, name: id, icon: null }
        return (
          <span key={id} className="picked">
            <Icon e={e} size={22} plain={kind === 'trait'} /> {e.name}
            <button aria-label={`Remove ${e.name}`} onClick={() => onChange(selected.filter((x) => x !== id))}><X size={12} /></button>
          </span>
        )
      })}
      {selected.length < max ? (
        <span className="picker-input">
          <input value={q} placeholder={placeholder} aria-label={placeholder}
            onChange={(e) => { setQ(e.target.value); setOpen(true) }}
            onFocus={() => setOpen(true)} onMouseDown={() => setOpen(true)} onBlur={() => setOpen(false)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && matches[0]) add(matches[0].id)
              if (e.key === 'Escape') setOpen(false)
              if (e.key === 'Backspace' && !q && selected.length) onChange(selected.slice(0, -1))
            }} />
          {open && matches.length ? (
            <ul className="picker-menu">
              {matches.map((o) => (
                // mousedown, so the pick lands before the input loses focus
                <li key={o.id} onMouseDown={(e) => { e.preventDefault(); add(o.id) }}>
                  <Icon e={o} size={22} plain={kind === 'trait'} /> {o.name}
                </li>
              ))}
            </ul>
          ) : null}
        </span>
      ) : null}
    </div>
  )
}

type Row = Entity & Rates & { placeChange: number }
type Result = Scope & { summary: (Rates & { games: number }) | { games: 0 } | null; partners: Row[]; items: Row[]; itemsOn?: Entity; ms: number }

export function Explore() {
  const f = useFilters()
  const statics = useApi<StaticData>('/api/static')
  const [units, setUnits] = useState<string[]>([])
  const [traits, setTraits] = useState<string[]>([])
  const path = units.length ? `/api/explore?units=${units.map(encodeURIComponent).join(',')}&traits=${traits.map(encodeURIComponent).join(',')}` : null
  const { data, error, loading } = useApi<Result>(path, f)
  const s = data?.summary

  return (
    <>
      <div className="explore-form">
        <label>Units on the board<Picker kind="unit" options={statics.data?.units ?? []} selected={units} onChange={setUnits} placeholder="Add a unit" max={4} /></label>
        <label>Active traits<Picker kind="trait" options={statics.data?.traits ?? []} selected={traits} onChange={setTraits} placeholder={units.length ? 'Add a trait' : 'Pick a unit first'} max={3} /></label>
      </div>

      {!units.length ? <div className="empty"><h2>Pick at least one unit</h2><p>Up to four units and three traits. Answers come straight from the stored boards, so a big database can take a few seconds.</p></div> : null}
      <Status error={error} loading={loading && !!path && !data} />

      {s && 'avgPlace' in s && s.games > 0 && data ? (
        <>
          <dl className="facts wide">
            <Fact label="Boards found">{num(s.games)}</Fact>
            <Fact label="Avg place">{s.avgPlace.toFixed(2)}</Fact>
            <Fact label="Win rate">{pct(s.winRate)}</Fact>
            <Fact label="Top 4">{pct(s.top4Rate)}</Fact>
            <Fact label="Share of all boards">{pct(s.frequency, 2)}</Fact>
          </dl>
          <div className="split">
            <section>
              <h3>Also on these boards</h3>
              <p className="muted small">Place change compares boards that add the unit with all {num(s.games)} boards found.</p>
              <Table label="Units also on these boards" rows={data.partners} initial="games" desc rowHeight={44} minWidth={420} cols={[
                { key: 'name', label: 'Unit', sort: (r) => r.name, render: (r) => <a className="name" href={`#/units/${encodeURIComponent(r.id)}`}><Icon e={r} size={26} /><span>{r.name}</span></a> },
                { key: 'freq', label: 'On', sort: (r) => r.frequency, render: (r) => pct(r.frequency, 0), align: 'right', width: '64px' },
                { key: 'change', label: 'Change', sort: (r) => r.placeChange, render: (r) => <Change value={r.games >= 5 ? r.placeChange : null} />, align: 'right', width: '84px' },
                { key: 'games', label: 'Games', sort: (r) => r.games, render: (r) => num(r.games), align: 'right', width: '84px' },
              ]} />
            </section>
            <section>
              <h3>Items on {data.itemsOn?.name ?? 'the first unit'}</h3>
              <p className="muted small">Within the boards found.</p>
              <Table label="Items on the first unit" rows={data.items} initial="games" desc rowHeight={44} minWidth={420} empty="No items recorded on this unit in these boards." cols={[
                { key: 'name', label: 'Item', sort: (r) => r.name, render: (r) => <a className="name" href={`#/items/${encodeURIComponent(r.id)}`}><Icon e={r} size={26} plain /><span>{r.name}</span></a> },
                { key: 'freq', label: 'On', sort: (r) => r.frequency, render: (r) => pct(r.frequency, 0), align: 'right', width: '64px' },
                { key: 'change', label: 'Change', sort: (r) => r.placeChange, render: (r) => <Change value={r.games >= 5 ? r.placeChange : null} />, align: 'right', width: '84px' },
                { key: 'games', label: 'Games', sort: (r) => r.games, render: (r) => num(r.games), align: 'right', width: '84px' },
              ]} />
            </section>
          </div>
          <p className="muted small mt-3">Answered in {data.ms} ms.</p>
        </>
      ) : null}
      {s && s.games === 0 ? <div className="empty"><h2>No boards match</h2><p>Remove a unit or trait, or widen the rank and time filters.</p></div> : null}
    </>
  )
}
