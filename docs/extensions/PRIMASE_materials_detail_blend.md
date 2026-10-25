# PRIMASE_materials_detail_blend

A vendor extension on a glTF material: the material's surface as its base and tiled detail textures combine at draw
time, the equation a family of source programs computes (`pbr.detail` in primase, `primase_core::detail`). It is listed
in `extensionsUsed` and never in `extensionsRequired`. The material's own `pbrMetallicRoughness` (and
`KHR_materials_specular`) is the source's bake of the same surface at a capped size: a viewer without the extension
draws that, and loses only the detail's resolution.

```json
"extensions": {"PRIMASE_materials_detail_blend": {
  "equation": "detail_lerp",
  "lowering": "gloss_reflectance",
  "baseColorTexture":  {"index": 0, "colorSpace": "srgb"},
  "baseNormalTexture": {"index": 1, "colorSpace": "linear"},
  "baseMaskTexture":   {"index": 2, "colorSpace": "linear"},
  "details": [
    {"colorTexture": {"index": 3, "colorSpace": "srgb",
                      "extensions": {"KHR_texture_transform": {"scale": [16, 16]}}},
     "normalTexture": {"index": 4, "colorSpace": "linear",
                       "extensions": {"KHR_texture_transform": {"scale": [16, 16]}}},
     "tint": [3.64, 3.185, 2.73], "glossBias": 0.25,
     "glossScale": {"baseNormalZ": {}}, "reflectance": {"maskPlusBias": 0.375}},
    {"colorTexture": {"index": 5, "colorSpace": "srgb"},
     "normalTexture": {"index": 6, "colorSpace": "linear"},
     "tint": [5.46, 5.46, 5.46], "glossBias": -0.1,
     "glossScale": {"constant": 0.5}, "reflectance": {"bias": 0.6}}],
  "weights": [
    {"source": {"mask": {"texture": {"index": 1, "colorSpace": "linear"},
                         "channels": [0, 0, 22, 0], "bias": 0}},
     "steps": [{"power": 260}, {"min": 1}]}],
  "gain": 1, "normalStrength": 0.5, "dielectricF0": 0.03, "color": [0.9, 0.85, 0.8],
  "glossEncoding": {"power": 1, "root": 2, "scale": 0.95},
  "alphaDetail": 1,
  "frozen": [{"frontFacing": {}}, {"engineConstant": {"name": "SomeEngineConstant", "value": [0]}}]
}}
```

## Textures

Each texture field is a glTF texture info (`index`, `texCoord`, and `KHR_texture_transform` for the tiling the source
samples it at, as on any glTF slot), plus `colorSpace`: `srgb` where the source reads the texel decoded from sRGB, `linear`
where it reads it as stored. Wrap and filters are the texture's sampler, as for a core slot. The sample of a texture is its
four lanes as the source reads them; a field reads the lanes its role says.

## The equation

`equation` is a closed set; `detail_lerp` is the only value. With `A`, `N`, `M` the base colour, normal and mask
samples, `D_i`, `K_i` detail `i`'s colour and normal samples, `lerp(a, b, t) = a + (b - a) * t` lane by lane, and
`chain(x)` the details' values `x_0, x_1, ...` lerped in turn by the weights (`x_0`, then `lerp(.., x_1, w_1)`, then
`lerp(.., x_2, w_2)`), a conforming viewer computes, in this order and in 32-bit floats:

1. The weights, one fewer than the details: `w_i` = the source's value, then each step in turn. A `mask` source is
   `dot(sample.rgba, channels) + bias` of its texture; a `vertex` source is channel `channel` of the attribute. `power`
   `n` (1 to 512) is taken by squaring: `x`, `x^2`, `x^4`, ... each the square of the one before, the powers of `n`'s
   set bits multiplied in from the lowest (`x^260 = x^4 * x^256`). `min` `b` is `min(x, b)`, `b` where `x` is NaN.
2. `C = saturate(gain * A.rgb * chain(tint_i * D_i.rgb))`.
3. `f = saturate(chain(r_i))`, where `r_i` is detail `i`'s reflectance term: `maskPlusBias` `b` is `M.x + b`, `bias`
   `b` is `b`.
4. Albedo `(1 - f) * C * color`; reflectance (F0) `lerp(dielectricF0, C, f)` per lane.
5. Normal, in tangent space and unnormalised: `xy = 2 * N.xy - 1`, `z = sqrt(1 - min(dot(xy, xy), 1))`, then
   `xy += normalStrength * (2 * chain(K_i.xy) - 1)`.
6. Gloss as stored: `saturate(2 * chain(s_i) * chain(K_i.z + glossBias_i))`, where `s_i` is detail `i`'s gloss scale:
   `baseNormalZ` is `N.z`, `constant` `c` is `c`. The gloss is that decoded by `glossEncoding`:
   `scale * stored^(power / root)`.
7. Alpha: `saturate(D_alphaDetail.a * A.a)` where `alphaDetail` is present, else 1 (the surface is opaque).

One detail is no lerp at all: its values are the chain's.

`lowering` says how the result becomes the PBR model: `gloss_reflectance` is the base colour the albedo (and the alpha),
roughness `1 - gloss`, metallic 0, and the reflectance the F0 as `KHR_materials_specular`'s `specularColorFactor` 25
times the specular colour (glTF's F0 is `0.04 * factor * colour`).

## What it holds constant

`frozen` lists what the equation holds constant that the source varies per draw, each with its value: a viewer that
draws the equation draws the surface under these. Its entries are a closed set:

- `frontFacing`: the surface drawn front-facing.
- `vertexInput`: `{"semantic", "index", "value"}`, a vertex input held at `value`.
- `engineConstant`: `{"name", "value"}`, a constant the source's engine sets per draw, held at `value` (its words).
- `engineTexture`: `{"name", "value"}`, a texture the source's engine binds per draw, read as the colour `value`.

## Reading it strictly

primase's reader (`formats/gltf/src/detail.rs`, run by `lower-gltf-materials`) takes the extension as this document
states it and refuses, with an error naming the material, anything else: a key not listed above (`extras` apart), a
value outside a closed set (`equation`, `lowering`, a `glossScale`, `reflectance`, `source` or `frozen` form, a step), one
to three `details` and exactly one fewer `weights`, a `power` outside 1 to 512, a `glossEncoding` `power` or `root` outside
1 to 255, a `vertex` attribute that is not `COLOR_n` or `TEXCOORD_n` (channel 0 to 3), an `alphaDetail` that is not a
detail, a texture info without `colorSpace` or with one other than `srgb` or `linear`, and a number that is not a finite
32-bit float. A material whose extension is sound but one of whose images the reader could not decode is read without
its detail (its `pbr.material` alone), counted; nothing is guessed. A conforming writer lists the extension in
`extensionsUsed` only.

## Numbers

Every number is written as the 32-bit float the source states, so a reader gets the same bits back.

## Evidence

primase writes the extension only for a material whose program's equation it has proven (`derive-fromsoft-detail`): solved from runs of the
program with designed samples and held to it on random samples and on samples where the weights mix, every output
within 64 units in the last place. The comparison of each constant with the material word it reads, and the cost of
folding the gain into the tints, are the census's audit of the derivation (`detail_derive`'s example), not part of
what the extension states.
