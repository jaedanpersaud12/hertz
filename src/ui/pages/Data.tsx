import { ChartBar, DownloadSimple, Shapes, Stack } from '@phosphor-icons/react'
import { ago, count, num, type Meta } from '../api'
import { CollapsibleBanner } from '../interior/collapsible-banner'
import { CopyButton } from '../interior/copy-button'
import { TaskSteps } from '../interior/task-steps'
import { Fact } from '../parts'

const COMMANDS = [
  { icon: Shapes, title: 'Names and icons', command: 'npm run static', text: 'Download unit, item and trait data for the current set. Run again after each patch.' },
  { icon: DownloadSimple, title: 'Collect matches', command: 'npm run crawl', text: 'Leave it running. Stop with Ctrl+C and it resumes where it left off.' },
  { icon: ChartBar, title: 'Rebuild stats', command: 'npm run stats', text: 'Recount every number in the app. Safe to run while collecting.' },
  { icon: Stack, title: 'Regroup comps', command: 'npm run recluster', text: 'Re-label stored boards after changing the comp rules.' },
]

export function Data({ meta }: { meta: Meta }) {
  const t = meta.totals
  const most = Math.max(1, ...meta.byPatch.map((p) => p.matches))
  // Setup is three steps in order; "current" is the first one not done yet.
  const done = [t.staticRows > 0, t.matches > 0, !!meta.builtAt]
  const current = done.indexOf(false) === -1 ? 3 : done.indexOf(false)
  return (
    <div className="data-page">
      <div className="command-cards">
        {COMMANDS.map((c) => (
          <section key={c.command} className="command-card">
            <c.icon size={24} weight="duotone" />
            <h4>{c.title}</h4>
            <p>{c.text}</p>
            <div><CopyButton className="ibtn" value={c.command} label={c.command} copiedLabel="Copied" /></div>
          </section>
        ))}
      </div>

      <div className="mt-5">
        <CollapsibleBanner title="Everything here is stored on this machine"
          description="Stats come from your own SQLite file in the data folder. Nothing is sent anywhere except requests to Riot for matches and to Community Dragon for icons." />
      </div>

      <h3>Setup</h3>
      <div className="max-w-[460px]">
        <TaskSteps label="Setup progress" current={current} steps={[
          { id: 'static', label: 'Download names and icons', meta: t.staticRows ? count(t.staticRows, 'entry', 'entries') : 'npm run static' },
          { id: 'crawl', label: 'Collect matches', meta: t.matches ? `${num(t.matches)} stored` : 'npm run crawl' },
          { id: 'stats', label: 'Build stats', meta: meta.builtAt ? `rebuilt ${ago(meta.builtAt)}` : 'npm run stats' },
        ]} />
      </div>

      <h3>Stored</h3>
      <dl className="facts wide">
        <Fact label="Matches">{num(t.matches)}</Fact>
        <Fact label="Boards">{num(t.comps)}</Fact>
        <Fact label="Players tracked">{num(t.players)}</Fact>
        <Fact label="Matches queued">{num(t.queued)}</Fact>
        <Fact label="Newest match">{ago(t.newest)}</Fact>
      </dl>

      <h3>Matches by patch</h3>
      {meta.byPatch.length ? (
        <ul className="bars max-w-[640px]">
          {meta.byPatch.map((p) => (
            <li key={p.patch}>
              <span>{p.patch}</span>
              <span className="bar"><span style={{ width: `${(p.matches / most) * 100}%` }} /></span>
              <span>{num(p.matches)}</span>
            </li>
          ))}
        </ul>
      ) : <p className="muted">No matches yet.</p>}
    </div>
  )
}
