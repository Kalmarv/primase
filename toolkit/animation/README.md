# Animation

The arithmetic behind primase's rig components. The meanings are core's (`core.clip`, `core.skeleton`, `core.skin`,
`core.morphs`): this crate evaluates them. Everything is computed in f64 and rounded once to f32. Each function states
its exact rule.

- `curve::sample`: a channel's value at a time. Step holds; Linear interpolates, rotations by slerp along the shorter
  arc; Hermite is glTF's `CUBICSPLINE` (per key in-tangent, value, out-tangent, tangents per second times the segment's
  length), rotations componentwise then normalised. The end values hold outside the keys.
- `pose::evaluate`: a clip at a time on a skeleton, over a base pose. Each joint channel replaces its joint's
  translation, rotation or scale; looping maps the time into `[0, duration]`; morph and control values and root
  motion are returned. A joint a clip drives needs a TRS form (`pose::Trs::of`: exact for an unsheared, unmirrored
  rest). Additive clips are refused, not evaluated at rest.
- `pose::model_transforms`: each joint's transform in the skeleton's space, its local composed with its parents'.
- `deform::apply_morphs`: morph targets at their weights, piecewise linear through `(0, 0)` and each frame,
  extrapolated along the first and last segments; dense or sparse deltas; normals and tangents renormalised when
  moved.
- `deform::skin`: linear blend skinning. The vertex matrix is `sum_i w_i J_i B_i`; positions go through it, normals
  through the inverse transpose of its linear part, tangents through the linear part (both renormalised, the
  tangent's w kept). Weights are used as stored, except under `EngineNormalizes`, where they are divided by their sum.
- `quat`: unit quaternion products, slerp and the rotation of a matrix (Shepperd's method).

Checks: hand-worked fixtures in each module's tests. Blender 4.3.2's importer is an independent consumer of the same
deformation (eight influences, linear blend skinning), verified against hand-worked values by a sentinel glTF kept with
the private oracle tools.
