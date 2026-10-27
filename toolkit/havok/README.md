# primase-havok

Havok data made primase's. The files are read by `havok-formats` (`repos/havok-formats`); this crate holds what
turns what it reads into geometry and world data:

- `ConvexCore`: a convex shape's stored core as triangles.
- `world`: a Havok skeleton as `core.skeleton`, a spline compressed animation as it is stored (`havok.animation`),
  and the pass `lower-havok-clips` that samples the animations a program asks for into `core.clip`s.

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
