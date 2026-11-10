import { useEffect, useState } from 'react'
import { ago, send, STRESS, useApi, useFilters, useMarks, type Comp, type Scope } from '../api'
import { HoldToConfirm } from '../interior/hold-to-confirm'
import { Button, Icon, Status } from '../parts'
import { openTeam, type Board } from './Builder'
import { CompRow } from './Comps'
import type { StaticData } from './Explore'

type Team = { id: number; name: string; created: number; board: Board }

export function Lists() {
  const f = useFilters()
  const marks = useMarks()
  // min=1 so a favourite still shows when it is rare at the current filters
  const comps = useApi<Scope & { rows: Comp[] }>('/api/comps', { ...f, min: 1 })
  const statics = useApi<StaticData>('/api/static')
  const [teams, setTeams] = useState<Team[] | null>(null)
  useEffect(() => { fetch(`/api/lists${STRESS ? `?data=${STRESS}` : ''}`).then((r) => r.json()).then((d) => setTeams(d.teams)).catch(() => setTeams([])) }, [])

  if (comps.error || !comps.data) return <Status error={comps.error} loading={comps.loading} />
  const byId = new Map(comps.data.rows.map((c) => [c.id, c]))
  const units = new Map((statics.data?.units ?? []).map((u) => [u.id, u]))
  const label = (key: string) => byId.get(key)?.name ?? key.replace(/TFT\d*_/g, '').replace('|', ' ')
  const favs = marks.favs.map((k) => byId.get(k)).filter((c): c is Comp => !!c)
  const missing = marks.favs.filter((k) => !byId.has(k))

  return (
    <div className="lists">
      <h3>Favourite comps</h3>
      {favs.length ? <ul className="comps">{favs.map((c) => <CompRow key={c.id} comp={c} />)}</ul> : null}
      {missing.length ? (
        <ul className="plain-list">
          {missing.map((k) => (
            <li key={k}>{label(k)} <span className="muted">has no games at the current patch and filters</span>
              <Button size="sm" onClick={() => marks.toggleFav(k)}>Remove</Button></li>
          ))}
        </ul>
      ) : null}
      {!marks.favs.length ? <p className="muted">Press the heart on a comp to keep it here.</p> : null}

      <h3>Saved teams</h3>
      {teams?.length ? (
        <ul className="teams">
          {teams.map((t) => (
            <li key={t.id}>
              <div>
                <span className="team-name">{t.name}</span>
                <span className="muted small">saved {ago(t.created)}</span>
              </div>
              <span className="mini-board">
                {Object.values(t.board).map((id, n) => <Icon key={n} e={units.get(id) ?? { id, name: id.replace(/^TFT\d*_/, ''), icon: null }} size={32} />)}
              </span>
              <span className="row-actions">
                <Button onClick={() => openTeam(t.board)}>Open in builder</Button>
                <HoldToConfirm className="ibtn" confirmLabel="Deleted" duration={700}
                  onConfirm={() => send<{ teams: Team[] }>(`/api/teams/${t.id}`, undefined, 'DELETE').then((d) => setTeams(d.teams))}>
                  Hold to delete
                </HoldToConfirm>
              </span>
            </li>
          ))}
        </ul>
      ) : <p className="muted">Teams you save in the <a href="#/builder">team builder</a> appear here.</p>}

      <h3>Hidden comps</h3>
      {marks.hidden.length ? (
        <ul className="plain-list">
          {marks.hidden.map((k) => <li key={k}>{label(k)} <Button size="sm" onClick={() => marks.toggleHidden(k)}>Show again</Button></li>)}
        </ul>
      ) : <p className="muted">Comps you hide from the Comps page are listed here so you can bring them back.</p>}
    </div>
  )
}
