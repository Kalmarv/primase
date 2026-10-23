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
- `fit`, `placement` and `retarget_skin`: `retarget-skin` poses a source skeleton onto a target's proportions
  (`fit=positions`: every mapped joint at its target's position, segments take the target's lengths and the mesh
  stretches between them; `fit=lengths`: every segment keeps the source's length and points the target's way, a joint
  with several mapped children is one rigid body turned by Horn's best-fit rotation of its children's directions (a
  hand's palm roll), and listed limbs are two-bone IK to the target's end joint, with the shortfall reported), applies
  the stated joint placements (`placement`), skins the mesh into that pose and rebinds it. `RetargetSkin::report`
  (also the pass's log) measures the fit without changing anything: every segment's source, target and fitted length
  and scale, joints off their targets, the frames of joints with several children (angle, roll about the incoming
  bone), each IK limb's reach, each vertex group's largest displacement, the largest growth of a mesh edge and the
  widest split of vertices that stood together.
- `quat`: unit quaternion products, slerp and the rotation of a matrix (Shepperd's method).

Checks: hand-worked fixtures in each module's tests. Blender 4.3.2's importer is an independent consumer of the same
deformation (eight influences, linear blend skinning), verified against hand-worked values by a sentinel glTF kept with
the private oracle tools.
