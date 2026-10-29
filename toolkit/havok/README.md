# primase-havok

Havok data made primase's. The files are read by `havok-formats` (`https://github.com/Kalmarv/havok-formats`); this crate holds what
turns what it reads into geometry and world data:

- `ConvexCore`: a convex shape's stored core as triangles.
- `world`: a Havok skeleton as `core.skeleton`, a spline compressed animation as it is stored (`havok.animation`),
  and the pass `lower-havok-clips` that samples the animations a program asks for into `core.clip`s.
- `cloth`: a model's Havok cloth as it is stored (`havok.cloth`): the tagfile's cloth data read whole, the skeleton
  its transform sets are (each cloth data's one transform set is its file's one skeleton, bone for bone, in all of
  Sekiro's 610 and Elden Ring's 2,692 cloth data), the joints of the model's `core.skeleton` of the same names, and
  its display buffers bound to the model's meshes the reader names (`bind_displays`: the mesh of the buffer's name,
  vertex count and triangle count). `havok.cloth_vertices` on a lowered mesh says which display buffer vertex each
  vertex of a primitive is, and follows the primitive through every edit (a vertex blended from several is refused).
  It is read for a program that runs the cloth (havok-cloth steps it); nothing lowers it.

## Convex cores

`ConvexCore` retains stored vertices, faces, planes and convex radius. Its
`expanded_triangles` policy builds the intersection of offset half-spaces in
f64, then a closed hull with adaptive exact orientation predicates. It retains
new corners rather than moving each original corner by a least-squares fit.
Output vertices are inside every plane and incident to at least three planes
within 0.01 mm after f32 rounding; sub-nanometre duplicates are welded and
zero-area triangles caused by f32 rounding are omitted. This is a sharp
polytope approximation, not the rounded Havok surface. Capsule cores yield
box approximations; a sphere's core is its centre (one vertex, no planes), tessellated as
the cube of half-edge radius around it, the same sharp approximation. A core of zero thickness (two
opposite planes whose offsets agree within the plane bound, every vertex on them)
is a polygon: its two faces offset by the radius. `rounded_triangles(max_error)` is
the rounded surface Havok gives the shape (the core swept by a sphere of the convex
radius) within a stated error: the hull of every core vertex plus every vertex of an
icosphere inscribed in the radius, subdivided until each of its faces lies within the
error of the sphere; every output vertex is on the true surface, so the result lies
inside the shape and within the error of every point of it. An `hknpConvexShape` stores
vertices only: its core is their hull (one triangle a face), or, for vertices within
the plane bound of one plane, their convex polygon as two faces.

`oracles.corpus.sekiro/object-hkx` (the real-data configuration,
CONTRIBUTING.md) holds loose self-contained object `.hkx` files for an
independent half-space residual gate. Non-convex classes are counted
separately, not silently claimed as decoded.
