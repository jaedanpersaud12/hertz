import { CaretDown } from '@phosphor-icons/react'
import { motion, useReducedMotion } from 'motion/react'
import { useMemo, useState } from 'react'
import { ago, count, useApi, type BoardUnit, type TraitChipData } from '../api'
import { useAccordion, useAutoHeight, type AccordionHeaderProps, type AccordionPanelProps } from '../interior/accordion'
import { LoadingButton } from '../interior/loading-button'
import { LoadMore } from '../interior/load-more'
import { PlaceMedal } from '../art'
import { Status, TraitChip, UnitChip } from '../parts'

type Player = {
  me: boolean; name: string | null; placement: number; level: number; damage: number
  units: (BoardUnit & { star: number })[]; traits: TraitChipData[]
}
type Match = { id: string; date: number; length: number; queue: string; patch: string; mine: Player; lobby: Player[] }

export const placeClass = (p: number) => (p === 1 ? 'first' : p <= 4 ? 'top' : 'bottom')
const PAGE = 8

export function SetupHint() {
  return (
    <div className="empty">
      <h2>Add your Riot ID</h2>
      <p>Put these two lines in <code>.env</code>, then restart the app:</p>
      <pre>RIOT_ID=YourName#TAG{'\n'}MY_PLATFORM=na1</pre>
      <p>Your rank, recent placements and match history then load from Riot using your API key.</p>
    </div>
  )
}

function Game({ m, open, toggle, header, panel }: { m: Match; open: boolean; toggle: () => void; header: AccordionHeaderProps; panel: AccordionPanelProps }) {
  const reduced = useReducedMotion()
  const auto = useAutoHeight()
  return (
    <li>
      <div className="history-row" onClick={toggle}>
        <PlaceMedal place={m.mine.placement} size={32} />
        <div className="history-meta">
          <span>{m.queue}</span>
          <span className="muted small">{ago(m.date)}, patch {m.patch}{m.length >= 60 ? `, ${Math.round(m.length / 60)} min` : ''}, level {m.mine.level}</span>
        </div>
        <div className="comp-board">
          <div className="comp-traits">{m.mine.traits.slice(0, 8).map((t) => <TraitChip key={t.id} t={t} />)}</div>
          <div className="comp-units">{m.mine.units.map((u, n) => <UnitChip key={n} u={u} size={34} />)}</div>
        </div>
        <button {...header} className="btn ghost sm square" aria-label={open ? 'Hide the lobby' : 'Show the lobby'} onClick={(e) => { e.stopPropagation(); toggle() }}>
          <motion.span className="grid" initial={false} animate={{ rotate: open ? 180 : 0 }} transition={reduced ? { duration: 0 } : { type: 'spring', stiffness: 420, damping: 30 }}>
            <CaretDown size={14} />
          </motion.span>
        </button>
      </div>
      <motion.div initial={false} animate={{ height: open ? auto.height : 0 }} style={{ overflow: 'hidden' }}
        transition={reduced || !auto.ready ? { duration: 0 } : { type: 'spring', stiffness: 380, damping: 36 }}>
        <div ref={auto.ref} {...panel}>
          {open ? (
            <table className="lobby">
              <tbody>
                {m.lobby.map((p, n) => (
                  <tr key={n} className={p.me ? 'me' : ''}>
                    <td><PlaceMedal place={p.placement} size={22} /></td>
                    <td className="lobby-name" title={p.name ?? undefined}>{p.name ?? 'Player'}<span className="muted small">level {p.level}, {p.damage} damage</span></td>
                    <td><div className="comp-units">{p.units.map((u, i) => <UnitChip key={i} u={u} size={30} />)}</div></td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null}
        </div>
      </motion.div>
    </li>
  )
}

export function History() {
  const { data, error, loading, reload } = useApi<{ configured: boolean; matches: Match[] }>('/api/me/matches')
  const [open, setOpen] = useState<string[]>([])
  const [shown, setShown] = useState(PAGE)
  const visible = useMemo(() => (data?.matches ?? []).slice(0, shown), [data, shown])
  const entries = useMemo(() => visible.map((m) => ({ id: m.id })), [visible])
  const acc = useAccordion({ items: entries, type: 'single', open, onOpenChange: setOpen })

  if (error || !data) return <Status error={error} loading={loading} />
  if (!data.configured) return <SetupHint />

  const refresh = async () => {
    const r = await fetch('/api/me/refresh', { method: 'POST' }).then((x) => x.json())
    if (r.error) throw new Error(r.error)
    reload()
  }

  return (
    <>
      <div className="toolbar">
        <LoadingButton className="ibtn" onAction={refresh} pendingLabel="Checking Riot" successLabel="Up to date" errorLabel="Could not reach Riot">Check for new games</LoadingButton>
        <span className="toolbar-note">{count(data.matches.length, 'game')} stored</span>
      </div>
      {!data.matches.length ? <div className="empty"><h2>No games yet</h2><p>Press "Check for new games" to load your last 20 from Riot.</p></div> : (
        <>
          <ul className="history">
            {visible.map((m) => (
              <Game key={m.id} m={m} open={acc.isOpen(m.id)} toggle={() => acc.toggle(m.id)} header={acc.headerProps(m.id)} panel={acc.panelProps(m.id)} />
            ))}
          </ul>
          <div className="mt-4">
            <LoadMore auto={false} hasMore={shown < data.matches.length} onLoad={() => setShown((n) => n + PAGE)}
              labels={{ idle: 'Show older games', end: 'That is every stored game' }} />
          </div>
        </>
      )}
    </>
  )
}
