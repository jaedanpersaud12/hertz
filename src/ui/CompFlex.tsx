/**
 * The comp page's flex sections: units added at level 9 and swaps for core units (PRD 2 C8), and the comp's
 * variants (C2, from /api/comps/:id/variants). Place change is against the baseline the API names; negative is better.
 */
import type { ReactNode } from 'react'
import { count, num, pct, useApi, useFilters, type Entity } from './api'
import { Change, Icon, Status } from './parts'

type Flexed = { games: number; freq: number; avgPlace: number; placeChange: number; baseGames: number }
export type AddAt9 = Flexed & { unit: Entity }
export type Swap = Flexed & { out: Entity; in: Entity }

const None = ({ children }: { children: ReactNode }) => <p className="muted small detail-none">{children}</p>
const MORE = 'Collect more matches, or pick a wider rank filter or the whole patch.'

/** C8: "Add at level 9" and "Replace X with Y", each with its place change. */
export function Flex({ adds, swaps }: { adds: AddAt9[]; swaps: Swap[] }) {
  return (
    <section className="grow flex-lists">
      <div>
        <h4>Add at level 9</h4>
        <p className="muted small">Units the comp adds on boards of level 9 or more, beyond what it plays at 8. Place change against its other level 9 boards.</p>
        {adds.length ? (
          <ul className="flex-list">
            {adds.map((a) => (
              <li key={a.unit.id} title={`On ${count(a.games, 'level 9 board')} (${pct(a.freq, 0)}), compared with ${count(a.baseGames, 'board')} without it`}>
                <Icon e={a.unit} size={24} /> <span className="carry-item">{a.unit.name}</span>
                <span className="muted">{pct(a.freq, 0)}</span>
                <Change value={a.placeChange} />
              </li>
            ))}
          </ul>
        ) : <None>No unit is added at level 9 on 10 boards or more. {MORE}</None>}
      </div>
      <div>
        <h4>Swap options</h4>
        <p className="muted small">When a core unit is missing, what the board plays instead. Place change against boards that kept the core unit.</p>
        {swaps.length ? (
          <ul className="flex-list swaps">
            {swaps.map((s) => (
              <li key={`${s.out.id}>${s.in.id}`} title={`${s.in.name} on ${count(s.games, 'board')} without ${s.out.name} (${pct(s.freq, 0)}), compared with ${count(s.baseGames, 'board')} that kept ${s.out.name}`}>
                <span className="swap" aria-label={`Replace ${s.out.name} with ${s.in.name}`}>
                  <Icon e={s.out} size={24} /> <span className="swap-arrow" aria-hidden>→</span> <Icon e={s.in} size={24} />
                  <span className="carry-item">{s.in.name} <span className="muted small">for {s.out.name}</span></span>
                </span>
                <span className="muted">{num(s.games)}</span>
                <Change value={s.placeChange} />
              </li>
            ))}
          </ul>
        ) : <None>No swap for a core unit has 10 boards yet. {MORE}</None>}
      </div>
    </section>
  )
}

type Variant = { id: string; units: Entity[]; carries: Entity[]; games: number; avgPlace: number; pickRate: number }
/** Variants listed, and the fewest games one needs. */
export const VARIANTS_SHOWN = 4
export const VARIANT_MIN_GAMES = 50

/** C2: the comp's variants, by games, with average place and pick rate. */
export function Variants({ id }: { id: string }) {
  const f = useFilters()
  const { data, error, loading } = useApi<{ variants: Variant[] }>(`/api/comps/${encodeURIComponent(id)}/variants`, f)
  const rows = (data?.variants ?? []).filter((v) => v.games >= VARIANT_MIN_GAMES).slice(0, VARIANTS_SHOWN)
  return (
    <section className="grow">
      <h4>Variants</h4>
      <p className="muted small">Versions of this comp that play different flex units, with {VARIANT_MIN_GAMES} games or more.</p>
      {error || !data ? <Status error={error ? 'Could not load the variants. Check that Hertz is running, then reopen this comp.' : null} loading={loading} />
        : rows.length ? (
          <ul className="variants">
            {rows.map((v) => (
              <li key={v.id}>
                <span className="variant-units" title={v.units.map((u) => u.name).join(', ')}>{v.units.slice(0, 10).map((u, n) => <Icon key={`${u.id}${n}`} e={u} size={22} />)}</span>
                <span className="muted" title="Pick rate: boards per lobby">{v.pickRate.toFixed(2)}</span>
                <span title={count(v.games, 'game')}>{v.avgPlace.toFixed(2)}</span>
              </li>
            ))}
          </ul>
        ) : <None>No variant has {VARIANT_MIN_GAMES} games yet. Variants are found when comps are grouped by the units they share (comp method "clusters"). {MORE}</None>}
    </section>
  )
}
