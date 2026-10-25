# Lighting approximations

Approximations that stand core lights in for light a destination cannot draw.

- `fill-ambient-lights` (`FillAmbientLights`, `FillGrid`): `core.environment`'s
  ambient as a grid of shadowless point lights. Lights stand at the centres of a
  lattice from the caller's anchor, `spacing` apart, over the caller's bounds: in
  every cell meeting them (`FillCells::Meeting`, covering a play space) or only
  where the centre is inside them (`FillCells::Inside`, an interior, whose lights
  then all stand inside it). Each is of the ambient's mean colour, with intensity
  `level * (spacing / 2)^2` so a surface half a spacing away gets the ambient's
  level, and range `reach`. A light nearer a placed surface than `clearance` is
  left out (counted), which keeps an interior's lights off its walls, floor and
  ceiling. Every value is the program's: what determines the output is not the
  library's to default. Reports: the mean-colour reduction of a gradient, the
  lights placed and left out, and the approximation itself (a point light falls
  off and shines from a place; ambient does neither). The lights are in
  `core.light`'s units, so a destination's light calibration applies to them as
  to the scene's own lights. The pass consumes the ambient (takes it out of
  `core.environment`, the sun kept): the lights stand for it, so a writer does not
  report it as light it leaves out.
- `emissive-lights` (`EmissiveLights`, `EmissivePolicy`): emissive surfaces as point lights, for a source that draws
  lamps, bulbs and signs with emissive materials and has no lights of its own. A placed prototype's triangle
  primitives of materials that emit (`pbr.material` `emissive` times its texture times `emissive_strength`) are split
  into pieces (triangles joined at one position within `weld`); a piece's colour is the texture's area-weighted mean
  over its triangles (each at its own coordinates, so a piece across a wrap seam keeps its colour; for a material the
  source animates, the texture's mean) and a piece under `min_luminance` is left out;
  its intensity is the radiance's largest channel (colour x strength x `nits_per_strength`) times its area times
  `area_factor` and the placement's scale squared, in candela, its range `sqrt(intensity / e_min)` kept within the
  range options. A piece up to `big` metres is a bulb or fixture, one light each, except that a prototype with `string` or
  more of them is a string kept as `per_string` lights (consecutive groups along it, each at its pieces' mean position
  with their total intensity); a longer piece is one light per `strip` metres along its longest axis. More lights than
  `max_total` is an error, never a silent drop. Every option is required: the units (what a unit of strength is in
  cd/m2), the thinning and the thresholds are the program's. Reported: pieces seen, left out, folded, strips split,
  lights made, as an approximation. A world is read whole or in parts (a light is made from, and kept with, its
  placement). With `spots`, a light whose pieces are flat is a spot along their normal with `spots`' cones, or with
  `cones`' for a prototype whose name starts with one of its prefixes (a fixture that focuses its light); `down` makes
  each placement of a named prototype one spot straight down (its lights folded into one: a street lamp whose panels
  face sideways under a shade); `scale` multiplies the intensity of named prototypes. Not read: a surface's two sides, an animation's timing, a time-of-day
  switch of the emission (a light is always on; a destination's own time-of-day mask applies).
- `scale-emission` (`ScaleEmission`): how bright emissive surfaces draw, apart from the lights made of them. Each
  emissive material's strength times a factor stated by the name of the mesh drawing it (`prototypes`) or else of the
  material (`materials`); a material drawn at several factors is copied once per factor (`NAME xFACTOR`), the original
  keeping the unscaled primitives or else the most common factor. Run after `emissive-lights`.
- `SurfaceGrid`: the world's placed triangles (`placed_triangles`) bucketed in
  cubic cells within a box of interest, for "is a surface nearer than `d`" queries,
  by meshops' point-triangle distance.

## Known limit: parts

`fill-ambient-lights` runs on a whole map and refuses a part that is not the whole
(`core.part_scope` with `whole: false`). A parts version keeps the one map-anchored
lattice and needs, per part, the region it owns (so each lattice light is placed by
exactly one part) and context reaching `clearance` past that region (so the
clearance search sees every surface near its lights). Core's parts state their
context's area but not the region a part owns. Trigger to build it: a destination
that writes fill lights in parts. Every current user writes whole maps.
