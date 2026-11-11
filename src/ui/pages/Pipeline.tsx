import { useEffect, useMemo, useState } from 'react'
import { ago, count, num, pct, useApi } from '../api'
import { Button, Fact, Status, Table, Tag, type Col } from '../parts'

// ---- Shapes from the Rust API (crates/tft-api/src/engine.rs) --------------------------
type RegionRow = { region: string; label: string; crawling: boolean; matchesLastHour: number; last24h: number[]; queued: number }
type BucketRow = { bucket: string; sampledMatches: number; inRightBucket: number; real: { bucket: string; matches: number }[] }
export type Health = {
  regions: RegionRow[]
  lastProblem: { at: number; region: string; level: string; message: string } | null
  statsBuiltAt: number | null
  statsAgeMinutes: number | null
  newestMatchAt: number | null
  rankAccuracy: { sampledMatches: number; players: number; share: number | null; sampleRate: number; perBucket: BucketRow[] }
}
type Option = { value: string; label: string }
export type CrawlSettings = {
  platforms: string[]; tiers: string[]; saved: boolean
  defaults: { platforms: string[]; tiers: string[] }
  serverOptions: (Option & { route: string })[]
  tierOptions: Option[]
}

/** Stored rank buckets are cumulative ("Diamond+"); a lobby is filed under the narrowest one its rank reaches. */
const BUCKET_LABEL: Record<string, string> = {
  ALL: 'Below Platinum', 'PLATINUM+': 'Platinum and Emerald', 'DIAMOND+': 'Diamond', 'MASTER+': 'Master and up',
}
const ROUTE_LABEL: Record<string, string> = { americas: 'Americas', europe: 'Europe', asia: 'Asia', sea: 'Southeast Asia' }
const REFRESH_MS = 60_000

const minutes = (m: number) => (m < 1 ? 'Under a minute' : m < 90 ? `${m} min` : m < 2880 ? `${Math.round(m / 60)} hours` : `${Math.round(m / 1440)} days`)

function Retry({ text, onRetry }: { text: string; onRetry: () => void }) {
  return (
    <div className="error pipeline-retry">
      <span>{text}</span>
      <Button size="sm" onClick={onRetry}>Try again</Button>
    </div>
  )
}

/** Matches stored in each of the last 24 hours. Static bars: this redraws every minute, so it does not animate. */
function Day({ hours }: { hours: number[] }) {
  const most = Math.max(1, ...hours)
  return (
    <span className="day-bars" role="img" aria-label={`${num(hours.reduce((a, b) => a + b, 0))} matches in the last 24 hours`}>
      {hours.map((n, i) => (
        <span key={i} title={`${num(n)} ${n === 1 ? 'match' : 'matches'}, ${i === 23 ? 'last hour' : `${23 - i}–${24 - i} hours ago`}`}
          className={n ? '' : 'none'} style={{ height: n ? `${Math.max(8, (n / most) * 100)}%` : undefined }} />
      ))}
    </span>
  )
}

const regionCols: Col<RegionRow & { id: string }>[] = [
  { key: 'server', label: 'Server', sort: (r) => r.label, width: 'minmax(170px, 1.6fr)', render: (r) => (
    <span className="server-cell">
      <span className="server-name">{r.label}</span>
      <span className="muted small">{r.region}{r.crawling ? '' : ', not chosen'}</span>
    </span>
  ) },
  { key: 'hour', label: 'Last hour', sort: (r) => r.matchesLastHour, align: 'right', width: 'minmax(90px, 0.8fr)', render: (r) => num(r.matchesLastHour) },
  { key: 'day', label: 'Last 24 hours', width: 'minmax(150px, 1.4fr)', render: (r) => <Day hours={r.last24h} /> },
  { key: 'queued', label: 'Waiting', sort: (r) => r.queued, align: 'right', width: 'minmax(90px, 0.8fr)', render: (r) => num(r.queued) },
]

/** The Data page's live view of the crawler: throughput, queue, problems, freshness and rank-filter accuracy. */
export function PipelineHealth({ reloadKey }: { reloadKey: number }) {
  const { data, error, loading, reload } = useApi<Health>('/api/health')
  useEffect(() => { const t = setInterval(reload, REFRESH_MS); return () => clearInterval(t) }, [reload])
  useEffect(() => { if (reloadKey) reload() }, [reloadKey, reload])
  const rows = useMemo(() => (data?.regions ?? []).map((r) => ({ ...r, id: r.region })), [data])

  if (!data) {
    return (
      <>
        <h3>Pipeline health</h3>
        {error ? <Retry text="Could not load the crawler's health. Check that the app's server is running, then try again." onRetry={reload} />
          : <Status error={null} loading={loading} />}
      </>
    )
  }
  const lastHour = rows.reduce((a, r) => a + r.matchesLastHour, 0)
  const waiting = rows.reduce((a, r) => a + r.queued, 0)
  const p = data.lastProblem
  return (
    <>
      <h3>Pipeline health</h3>
      <dl className="facts wide">
        <Fact label="Matches in the last hour">{num(lastHour)}</Fact>
        <Fact label="Waiting to download">{num(waiting)}</Fact>
        <Fact label="Stats age" sub={<span className="muted">newest match {ago(data.newestMatchAt)}</span>}>
          {data.statsAgeMinutes == null ? 'Not built' : minutes(data.statsAgeMinutes)}
        </Fact>
      </dl>

      {p ? (
        <div className="problem">
          <div className="problem-head">
            <Tag kind={p.level === 'error' ? 'error' : 'level'}>{p.level === 'error' ? 'Error' : 'Warning'}</Tag>
            <span className="small">Last problem {ago(p.at)}{p.region ? `, ${ROUTE_LABEL[p.region] ?? p.region}` : ''}</span>
          </div>
          <p className="problem-text">{p.message}</p>
        </div>
      ) : <p className="muted small mb-3">No problems logged. Warnings and errors from the crawler show up here.</p>}

      {rows.length ? (
        <Table rows={rows} cols={regionCols} initial="hour" desc label="Matches per server" rowHeight={52} minWidth={560} />
      ) : (
        <div className="empty-note">
          <p>Nothing collected yet.</p>
          <p className="muted small">Run <code>npm run crawl</code> and leave it running. Each server you choose under What to crawl gets a row here.</p>
        </div>
      )}
      <p className="muted small mt-2">Updates every minute. Waiting is matches found but not downloaded yet.</p>

      <RankAccuracy acc={data.rankAccuracy} />
    </>
  )
}

function RankAccuracy({ acc }: { acc: Health['rankAccuracy'] }) {
  const rows = acc.perBucket.filter((b) => b.sampledMatches > 0)
  const every = acc.sampleRate > 0 ? Math.round(1 / acc.sampleRate) : 0
  const rate = every === 1 ? 'every one of the' : every > 1 ? `about 1 in ${num(every)}` : null
  return (
    <>
      <h3>Rank filter accuracy</h3>
      <p className="lede">
        Each match is filed under the rank of the player the crawler found it through, and the rank filters use that.
        To check it, the crawler looks up all eight players in {rate ?? 'a small share of'} matches. A lobby is in the
        right bucket when its middle player (the 5th of 8 by rank) is in the same bucket it was filed under.
      </p>
      {acc.sampledMatches === 0 ? (
        <div className="empty-note">
          <p>No lobbies checked yet.</p>
          <p className="muted small">
            {acc.sampleRate > 0
              ? 'Leave the crawler running. Checked lobbies show up here after its next rounds.'
              : <>Checking is turned off. Set <code>RANK_SAMPLE_RATE</code> in .env to a share above 0, such as 0.01, and restart the crawler.</>}
          </p>
        </div>
      ) : (
        <>
          <dl className="facts wide">
            <Fact label="In the right bucket">{acc.share == null ? '–' : pct(acc.share, 0)}</Fact>
            <Fact label="Lobbies checked">{num(acc.sampledMatches)}</Fact>
            <Fact label="Players looked up">{num(acc.players)}</Fact>
          </dl>
          <ul className="accuracy">
            {rows.map((b) => {
              const at = acc.perBucket.findIndex((x) => x.bucket === b.bucket)
              const lower = b.real.slice(0, at).reduce((a, r) => a + r.matches, 0)
              const higher = b.real.slice(at + 1).reduce((a, r) => a + r.matches, 0)
              const share = b.inRightBucket / b.sampledMatches
              return (
                <li key={b.bucket}>
                  <span className="accuracy-name">Filed as {BUCKET_LABEL[b.bucket] ?? b.bucket}</span>
                  <span className="bar" aria-hidden><span style={{ width: `${share * 100}%` }} /></span>
                  <span className="accuracy-share">{pct(share, 0)} <span className="muted small">of {num(b.sampledMatches)}</span></span>
                  <span className="muted small accuracy-miss">
                    {lower || higher ? `Really ${[lower ? `${num(lower)} lower` : '', higher ? `${num(higher)} higher` : ''].filter(Boolean).join(', ')}` : 'All in this bucket'}
                  </span>
                </li>
              )
            })}
          </ul>
          {acc.sampledMatches < 50 ? <p className="muted small mt-2">Only {count(acc.sampledMatches, 'lobby', 'lobbies')} so far, so these shares can still move a lot.</p> : null}
        </>
      )}
    </>
  )
}

const same = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x))

/** Which servers to crawl and which tiers to find players in. Saved in the database; the crawler reads it each round. */
export function CrawlSettingsForm({ onSaved }: { onSaved: () => void }) {
  const { data, error, loading, reload } = useApi<CrawlSettings>('/api/settings/crawl')
  const [platforms, setPlatforms] = useState<string[]>([])
  const [tiers, setTiers] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  useEffect(() => { if (data) { setPlatforms(data.platforms); setTiers(data.tiers) } }, [data])

  if (!data) {
    return (
      <>
        <h3>What to crawl</h3>
        {error ? <Retry text="Could not load the crawl settings. Check that the app's server is running, then try again." onRetry={reload} />
          : <Status error={null} loading={loading} />}
      </>
    )
  }

  const changed = !same(platforms, data.platforms) || !same(tiers, data.tiers)
  const toggle = (list: string[], set: (v: string[]) => void, v: string) => {
    setNote(null)
    setProblem(null)
    set(list.includes(v) ? list.filter((x) => x !== v) : [...list, v])
  }
  const submit = async (path: string, body?: unknown) => {
    setBusy(true)
    setProblem(null)
    setNote(null)
    try {
      const r = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) })
      const b = await r.json().catch(() => null) as (CrawlSettings & { error?: string }) | null
      if (!r.ok || !b) return setProblem(b?.error ?? 'The settings were not saved. Try again.')
      setPlatforms(b.platforms)
      setTiers(b.tiers)
      setNote(body === undefined ? 'Back to the defaults from .env. The crawler uses them from its next round.' : 'Saved. The crawler uses this from its next round.')
      reload()
      onSaved()
    } catch {
      setProblem('The app could not reach its own server. Check that it is still running, then try again.')
    } finally {
      setBusy(false)
    }
  }
  // Keep the order of the option lists, whatever order the boxes were ticked in.
  const ordered = (picked: string[], options: Option[]) => options.map((o) => o.value).filter((v) => picked.includes(v))
  const routes = [...new Set(data.serverOptions.map((s) => s.route))]
  const label = (opts: Option[], vs: string[]) => vs.map((v) => opts.find((o) => o.value === v)?.label ?? v).join(', ')

  return (
    <>
      <h3>What to crawl</h3>
      <form className="crawl-settings" onSubmit={(e) => { e.preventDefault(); submit('/api/settings/crawl', { platforms: ordered(platforms, data.serverOptions), tiers: ordered(tiers, data.tierOptions) }) }}>
        <p className="muted small">
          Changes apply on the crawler's next round, with no restart.{' '}
          {data.saved ? 'These choices were made here and replace PLATFORMS and TIERS in .env.' : 'Using PLATFORMS and TIERS from .env until you save.'}
        </p>
        <fieldset>
          <legend>Servers <span className="muted small">{count(platforms.length, 'chosen', 'chosen')}</span></legend>
          <div className="pick-groups">
            {routes.map((route) => (
              <div key={route} className="pick-group">
                <span className="muted small">{ROUTE_LABEL[route] ?? route}</span>
                {data.serverOptions.filter((s) => s.route === route).map((s) => (
                  <label key={s.value} className="check">
                    <input type="checkbox" checked={platforms.includes(s.value)} onChange={() => toggle(platforms, setPlatforms, s.value)} /> {s.label}
                  </label>
                ))}
              </div>
            ))}
          </div>
        </fieldset>
        <fieldset>
          <legend>Tiers <span className="muted small">the crawler finds matches through players in these tiers</span></legend>
          <div className="pick-tiers">
            {data.tierOptions.map((t) => (
              <label key={t.value} className="check">
                <input type="checkbox" checked={tiers.includes(t.value)} onChange={() => toggle(tiers, setTiers, t.value)} /> {t.label}
              </label>
            ))}
          </div>
        </fieldset>
        {problem ? <p className="error small" role="alert">{problem}</p> : null}
        {note ? <p className="small" role="status">{note}</p> : null}
        <div className="account-actions">
          <Button variant="primary" type="submit" disabled={busy || !changed}>{busy ? 'Saving' : 'Save'}</Button>
          {changed ? <Button variant="ghost" disabled={busy} onClick={() => { setPlatforms(data.platforms); setTiers(data.tiers); setProblem(null) }}>Undo changes</Button> : null}
          <Button variant="ghost" disabled={busy || !data.saved} onClick={() => submit('/api/settings/crawl/reset')}
            title={`Defaults: ${label(data.serverOptions, data.defaults.platforms)}; ${label(data.tierOptions, data.defaults.tiers)}`}>
            Reset to defaults
          </Button>
        </div>
      </form>
    </>
  )
}
