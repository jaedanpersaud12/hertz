# TFT Local identity

## The idea
TFT is played on hexes, so everything is a **cut hexagonal stone, lit from the top**: a pointy-top
hexagon with a lighter top facet and a darker base facet. Richer tiers get a halo ring and a four-point
spark. Nothing else decorates the interface.

- Surfaces: near-black `#0f0f11`, panels `#18181b`
- Accent: violet `#6c47ff` to `#a594ff` (the app's own stone)
- Result colours: coral S, gold A, olive B, green C, empty setting D; gold, silver, bronze, jade (4th) and steel for placements
- Type: Geist, semibold, tight tracking

Built so far, all hand-drawn SVG in `src/ui/art.tsx` (see `sheet.png`, or open `#/identity` in the app):
logo mark and favicon, tier badges, placement medals, trait stones, empty-state board.

## Prompts for larger art
Attach `sheet.png` as the reference image every time, so new pieces match.

Shared style line, append to each prompt:
> faceted hexagonal gemstone style, pointy-top hexagon, light from above, flat vector shading with two
> facets, dark near-black background, violet #6c47ff accent, no text, no characters, centred, crisp edges

### Quiver (SVG)
1. **Rank emblems, one per rank (Iron to Challenger):** "A rank emblem built from a single cut hexagonal
   stone in [iron grey / bronze / silver / gold / teal platinum / emerald / blue diamond / purple master /
   red grandmaster / pale gold challenger], with [0 to 3] small wing-like facets growing from its sides as
   the rank rises, and a four-point spark on the top right from Master up."
2. **Playstyle glyphs, 16px:** "A tiny one-colour icon of [a double chevron pointing up for fast levelling /
   two dice for reroll / a single arrow up for level], drawn inside a hexagon outline."
3. **Item-type glyphs:** "A one-colour hexagon icon containing [an anvil / a sunburst / a crown / a shield /
   a puzzle piece] for craftable, radiant, artifact, emblem and support items."
4. **Section icons for the tabs:** "A one-colour 20px icon of [stacked hexes / a single hex with a star /
   a hex with a sword / three linked hexes / a hex with a magnifier] in 1.5px line style."

### Higgsfield (raster, for big moments only)
5. **App icon, 1024px:** "A single large violet hexagonal gemstone with one smaller hexagon cut into its
   centre, floating over a faint grid of dark hex outlines, soft violet rim light, studio product shot."
6. **First-run hero, 1600x600:** "A dark hex-tiled board receding into shadow, a handful of glowing cut
   hexagonal stones in coral, gold, jade and violet resting on it, shallow depth of field, moody, no characters."
7. **First-place moment:** "A gold hexagonal gemstone bursting with a few thin light rays and small
   four-point sparks on a near-black background."

Do not use Riot's champion art, rank crests or item icons in generated art, and do not imitate another
app's badges: these should be recognisably ours.
