# Synthetic packed-normal fixture

`flver-normal.bin` is 256 little-endian f32 values, one for each unsigned byte.
For each byte `b`, multiply the integer `b - 127` by the f32 constant with bits
`0x3c010204` using exact f64 intermediates, then round once to f32. The small
integer and 24-bit significand product fit exactly in f64. This independently
generates the shader reciprocal-multiply expectations without a native decoder
or any extracted game bytes. The test covers normal xyz and tangent xyzw.
