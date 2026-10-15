# Mesh operations

`Mesh` stores positions, optional normals/UVs, and triangle-list indices.
Operations validate indices, attribute lengths and finite values before use.
Outputs preserve deterministic traversal order; they do not
implicitly change coordinate frames. Each function documents its exact rule.

- `compact`: first-use vertex remapping, all attributes retained.
- `weld_millimetres`: position-only grid weld, first representative retained,
  f32 millimetre keys rounded to even, collapsed faces omitted.
- `flat_normals`: duplicate face vertices and replace normals; degenerate faces
  use +Z. UVs are retained.
- `transform`: inverse-transpose normals and mirrored winding, with the
  reference orthogonal/sheared classification and normalization thresholds.
- `split_long`: shared midpoint subdivision in reference child order, normal
  and UV interpolation, with an explicit split limit bounding growth.
- `uv_tiles`: reference epsilon-adjusted UV bounding-box repeat count;
  `MAX_UV_TILES` is 256.
- `box_mesh`, `sphere`, `cylinder`, `capsule`, `disc`: reference primitive
  tessellation and radius thresholds. Discs use cylinder endpoint offsets.
  Sphere/cylinder/capsule axes and fallback normals use the caller's coordinates;
  poles and degenerate-axis fallback are +Z. No implicit Y-up conversion.

`vector::Vec3` is the shared vector type, with array conversions and ordinary
arithmetic operators. Add/subtract/multiply/divide, dot and cross use f32.
Length and normalization use a f64 norm; normalized components round once to
f32. Every finite nonzero vector normalizes, including tiny values; zero returns
zero. `normalize_or` selects the caller's fallback for zero/nonfinite input.
There are no runtime-specific arithmetic modes or small-vector passthroughs.
Geometric degeneracy thresholds remain explicit in each mesh operation.

Independent C# fixtures are value oracles. Stored attributes and topology remain
exact. Derived coordinates use a bound of four f32 epsilon units at the working
scale: max(1, abs(reference)) normally, or centre/endpoint magnitude plus radius
for tessellation (where cancellation can leave a tiny result). Gates count exact
and within-bound values and report maximum absolute differences. Tiny vectors
that the old oracle left unchanged are named deviations, checked instead against
our unit-length contract. `cargo test -p primase-meshops -- --nocapture` reports
these gates and invalid-input checks.

## Optional real-data cases

Set `PRIMASE_MESH_CASES` to a generated binary oracle bundle. The test skips when
unset. A bundle is supplied by an independent reference implementation; it must
not be checked into the repository when it contains external meshes.

All fields are little-endian u32 or f32. A mesh consists of four arrays, each
preceded by its scalar element count: positions (f32 triples), normals (f32
triples), UVs (f32 pairs), indices (u32). Empty normal/UV arrays are allowed.
The bundle starts with a mesh-case count. Each case stores its input mesh,
operation count (five), and records of operation ID plus expected mesh:

| ID | Operation |
|---|---|
| 0 | Flat normals |
| 1 | Long-edge split at 16 units |
| 2 | Transform with columns (-2,0,0), (.3,3,0), (.1,0,4), translation (1,2,3) |
| 3 | Millimetre weld with XY origin (0,0) |
| 4 | First-use vertex compaction |

Stored channels and indices compare by bits; derived channels use the stated bound. The test reports source
meshes, vertices, triangles and operation comparisons. Corpus selection and
coverage belong in the external run report; this test makes no full-census claim.

## Synthetic fixture encoding

`primitives.bin` starts with a case count, then records of kind (u32), radius
or box handedness (f32), vertex and index counts (u32), position triples (f32),
and indices (u32). Kinds 0/1/2/3 are sphere/cylinder/capsule/box. Inputs appear
explicitly in the gate test, including the fixed centres and endpoints.
`operations.bin` contains an operation count, one input mesh, then operation-ID
and output-mesh records (the synthetic test supplies the operation parameters).
`weld-uv.bin` stores a UV case count, each triangle's six f32 UV coordinates and
its f32 tile count, then input and expected welded position/index arrays with
scalar counts. No private source code or external assets are included.

`weld_quantized` accepts loose triangle corners and a positive grid scale;
`weld_millimetres` is the indexed, XY-origin variant with scale 1000.

`vectors.bin` is an independent SharpDX vector-arithmetic fixture: u32 count,
then 19 u32 float bit patterns per record (a[3], b[3], t, followed by length,
length squared, normalized a[3], a/t[3] or a when t=0, dot(a,b), cross(a,b)[3]).
The gate checks all 400 records against the shared vector operations with the stated bound.
