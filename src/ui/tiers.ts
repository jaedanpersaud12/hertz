/**
 * Letter grades and colour rules. These cut-offs are our own choice, not MetaTFT's formula: change them freely.
 * Average place runs 1 (best) to 8, and 4.5 is exactly average.
 */
export type Grade = 'S' | 'A' | 'B' | 'C' | 'D'

const grade = (value: number, cuts: [number, number, number, number]): Grade =>
  value <= cuts[0] ? 'S' : value <= cuts[1] ? 'A' : value <= cuts[2] ? 'B' : value <= cuts[3] ? 'C' : 'D'

export const gradeByPlace = (avgPlace: number) => grade(avgPlace, [4.2, 4.4, 4.6, 4.8])
export const gradeComp = (avgPlace: number) => grade(avgPlace, [4.15, 4.35, 4.55, 4.75])
export const gradeByChange = (placeChange: number) => grade(placeChange, [-0.3, -0.15, 0, 0.15])

/**
 * Riot's "style" number on a trait, mapped to the colour of its hexagon in game.
 * If trait colours look off by one step with live data, this table is the place to fix it.
 */
export const TRAIT_STYLE: Record<number, string> = { 1: 'bronze', 2: 'silver', 3: 'unique', 4: 'gold', 5: 'prismatic' }
