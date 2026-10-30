# Synthetic packed-normal fixture

`flver-normal.bin` is 256 little-endian f32 values, one for each unsigned byte.
For each byte `b`, multiply the integer `b - 127` by the f32 constant with bits
`0x3c010204` using exact f64 intermediates, then round once to f32. The small
integer and 24-bit significand product fit exactly in f64. This independently
generates the shader reciprocal-multiply expectations without a native decoder
or any extracted game bytes. The test covers normal xyz and tangent xyzw.

# Observed texture-slot forms (`coarse_slots.tsv`)

One row per pixel-shader texture slot of the read-back draws of two frame captures of Elden Ring (eldenring.exe
1.17.3, regulation 11730034, Stormveil Castle `m10_00_00_00`): the capture and draw, the placed model, the slot
register, the form the slot was bound to (`full` the texture's own, `low` its `_l` twin) and the asset textures whose
top mip, as the game uploaded it, is the slot's resource's first upload, comma-separated (several where those
uploads are equal). Slots whose candidates differ in form are left out. Names only: no bytes of the game.
`materials::drawn_coarse`'s test holds the rule to these observations and pins their counts.
