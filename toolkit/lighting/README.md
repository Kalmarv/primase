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
  to the scene's own lights.
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
