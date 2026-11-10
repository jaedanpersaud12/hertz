/**
 * TFT Local's own artwork. One idea runs through all of it: the hex.
 * TFT is played on hexes, so every badge here is a cut hexagonal gem, lit from the top.
 * Everything is inline SVG: sharp at any size, no image files, recoloured from one palette below.
 */
import { useId, type ReactNode } from 'react'
import type { Grade } from './tiers'

// Pointy-top hexagon on a 32-unit canvas, and the two facets that make it read as a cut stone.
const HEX = 'M16 2 28.1 9v14L16 30 3.9 23V9z'
const TOP = 'M16 2 28.1 9 16 16 3.9 9z'
const BASE = 'M3.9 23 16 16l12.1 7L16 30z'
const RING = 'M16 .2 29.7 8.1v15.8L16 31.8 2.3 23.9V8.1z'
const SPARK = 'M0-4.2C.5-1.4 1.4-.5 4.2 0 1.4.5.5 1.4 0 4.2-.5 1.4-1.4.5-4.2 0-1.4-.5-.5-1.4 0-4.2z'

export const PALETTE = {
  S: ['#ff9d8a', '#e2475f'], A: ['#f6cf6b', '#d18a1f'], B: ['#dfe07a', '#9aa52c'], C: ['#9fdc8d', '#3f9a5a'], D: ['#5b5b66', '#3a3a42'],
  gold: ['#f6cf6b', '#d18a1f'], jade: ['#7fe0b0', '#2f9a6c'], steel: ['#4a4a55', '#2c2c33'],
  bronze: ['#d6a274', '#96633a'], silver: ['#e3e8ef', '#8f9aab'], prismatic: ['#9fe9dc', '#b38cf0'], unique: ['#f7b071', '#d9682a'],
  violet: ['#a594ff', '#5a34f0'],
} as const
type Tone = keyof typeof PALETTE

/** The gem every badge is cut from. */
function Gem({ size, tone, hollow, ring, spark, children, label }: {
  size: number; tone: Tone; hollow?: boolean; ring?: boolean; spark?: boolean; children?: ReactNode; label?: string
}) {
  const id = useId()
  const [from, to] = PALETTE[tone]
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : true}
      style={{ flex: 'none', display: 'inline-block', verticalAlign: 'middle', overflow: 'visible' }}>
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor={from} /><stop offset="1" stopColor={to} /></linearGradient>
      </defs>
      {ring ? <path d={RING} fill="none" stroke={from} strokeWidth="1" strokeLinejoin="round" opacity=".55" /> : null}
      {hollow ? (
        <path d={HEX} fill="rgb(255 255 255 / .04)" stroke={from} strokeWidth="1.5" strokeLinejoin="round" />
      ) : (
        <>
          <path d={HEX} fill={`url(#${id})`} stroke={to} strokeWidth="1" strokeLinejoin="round" />
          <path d={TOP} fill="#fff" opacity=".2" />
          <path d={BASE} fill="#000" opacity=".16" />
        </>
      )}
      {children}
      {spark ? <path d={SPARK} transform="translate(26.5 5.5)" fill="#fff" /> : null}
    </svg>
  )
}

const Glyph = ({ children, fill = '#17171a', size = 15 }: { children: ReactNode; fill?: string; size?: number }) => (
  <text x="16" y="16.6" textAnchor="middle" dominantBaseline="central" fontSize={size} fontWeight="700" fill={fill}
    style={{ fontFamily: 'var(--font-sans)', fontVariantNumeric: 'tabular-nums' }}>{children}</text>
)

/**
 * Tier badge. The cut gets richer with the tier, so the grade reads without relying on colour alone:
 * S has a halo ring and a spark, A a halo ring, B and C are plain stones, D is an empty setting.
 */
export function TierBadge({ grade, size = 26 }: { grade: Grade; size?: number }) {
  return (
    <Gem size={size} tone={grade} hollow={grade === 'D'} ring={grade === 'S' || grade === 'A'} spark={grade === 'S'} label={`${grade} tier`}>
      <Glyph fill={grade === 'D' ? '#9a9aa6' : '#17171a'}>{grade}</Glyph>
    </Gem>
  )
}

/** Placement medal: gold, silver, bronze, then jade for fourth (still top four) and dull steel for the bottom four. */
export function PlaceMedal({ place, size = 30 }: { place: number; size?: number }) {
  const tone: Tone = place === 1 ? 'gold' : place === 2 ? 'silver' : place === 3 ? 'bronze' : place === 4 ? 'jade' : 'steel'
  return (
    <Gem size={size} tone={tone} ring={place === 1} spark={place === 1} label={`Placed ${place}`}>
      <Glyph fill={tone === 'steel' ? '#a9a9b4' : '#17171a'}>{place}</Glyph>
    </Gem>
  )
}

/** A trait's unit count on a stone in its tier's metal. Style 0 means the trait is not active yet. */
export function TraitGem({ count, style, size = 24 }: { count: number; style: number; size?: number }) {
  const tone = ({ 1: 'bronze', 2: 'silver', 3: 'unique', 4: 'gold', 5: 'prismatic' } as Record<number, Tone>)[style]
  return (
    <Gem size={size} tone={tone ?? 'steel'} hollow={!tone} spark={style === 5}>
      <Glyph size={14} fill={tone ? '#17171a' : '#9a9aa6'}>{count}</Glyph>
    </Gem>
  )
}

/** The app's mark: a violet stone with one cell of the board cut into it. */
export function LogoMark({ size = 24 }: { size?: number }) {
  return (
    <Gem size={size} tone="violet" spark>
      <path d="M16 10.5 20.8 13.2v5.6L16 21.5l-4.8-2.7v-5.6z" fill="none" stroke="#fff" strokeWidth="1.8" strokeLinejoin="round" />
    </Gem>
  )
}

/** For empty states: a patch of the board with a single stone waiting on it. */
export function BoardArt({ width = 132 }: { width?: number }) {
  const cell = (cx: number, cy: number, key: string, lit?: boolean) => (
    <g key={key} transform={`translate(${cx - 16} ${cy - 16})`}>
      {lit ? (
        <><path d={HEX} fill="url(#board-art)" stroke="#5a34f0" strokeLinejoin="round" /><path d={TOP} fill="#fff" opacity=".22" /><path d={BASE} fill="#000" opacity=".16" /></>
      ) : <path d={HEX} fill="rgb(255 255 255 / .025)" stroke="rgb(255 255 255 / .13)" strokeLinejoin="round" />}
    </g>
  )
  return (
    <svg width={width} viewBox="0 0 132 84" aria-hidden style={{ display: 'block' }}>
      <defs><linearGradient id="board-art" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#a594ff" /><stop offset="1" stopColor="#5a34f0" /></linearGradient></defs>
      {[0, 1, 2, 3].map((n) => cell(18 + n * 26, 18, `a${n}`))}
      {[0, 1, 2, 3].map((n) => cell(31 + n * 26, 41, `b${n}`, n === 1))}
      {[0, 1, 2, 3].map((n) => cell(18 + n * 26, 64, `c${n}`))}
      <path d={SPARK} transform="translate(70 28) scale(1.3)" fill="#fff" />
    </svg>
  )
}

/** Every piece of artwork on one page, at working size and enlarged. Open it at #/identity. */
export function IdentitySheet() {
  const row = { display: 'flex', gap: 22, alignItems: 'center', flexWrap: 'wrap' as const, margin: '10px 0 30px' }
  const grades: Grade[] = ['S', 'A', 'B', 'C', 'D']
  return (
    <div>
      <div className="page-head"><div><h1>Identity</h1><p className="page-sub">Every badge is a cut hexagonal stone, lit from the top. Colours live in PALETTE in src/ui/art.tsx.</p></div></div>
      <h3>Mark</h3>
      <div style={row}><LogoMark size={96} /><LogoMark size={48} /><LogoMark size={24} /><LogoMark size={16} />
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 10, fontSize: 28, fontWeight: 600, letterSpacing: '-0.02em' }}><LogoMark size={36} /> Hertz</span></div>
      <h3>Tier badges</h3>
      <div style={row}>{grades.map((g) => <TierBadge key={g} grade={g} size={72} />)}</div>
      <div style={row}>{grades.map((g) => <TierBadge key={g} grade={g} />)}{grades.map((g) => <TierBadge key={g + 's'} grade={g} size={20} />)}</div>
      <h3>Placement medals</h3>
      <div style={row}>{[1, 2, 3, 4, 5, 6, 7, 8].map((p) => <PlaceMedal key={p} place={p} size={56} />)}</div>
      <div style={row}>{[1, 2, 3, 4, 5, 6, 7, 8].map((p) => <PlaceMedal key={p} place={p} size={22} />)}</div>
      <h3>Trait stones</h3>
      <div style={row}>{[[2, 1], [4, 2], [1, 3], [6, 4], [10, 5], [1, 0]].map(([c, s]) => <TraitGem key={s} count={c} style={s} size={56} />)}
        <span className="muted small">bronze, silver, unique, gold, prismatic, not active</span></div>
      <div style={row}>{[[2, 1], [4, 2], [1, 3], [6, 4], [10, 5], [1, 0]].map(([c, s]) => <TraitGem key={s} count={c} style={s} size={20} />)}</div>
      <h3>Empty state</h3>
      <div style={row}><BoardArt width={220} /><BoardArt /></div>
    </div>
  )
}
