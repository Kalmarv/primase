# Optional Basis Universal encoder

`basisu_c_sys` is pinned to 0.9.1, wrapper revision
`38f0c7e7e8b4d44ec9154d95dee14847711b6b98`. Its packaged native sources
match Basis Universal revision `9bebe16726b3a61c8c213eeee3b7cffb462ef34e`:
109 of 109 files match the upstream Git blobs, with no modifications.
The adjacent SHA256 manifest records those packaged files. The original
manifest's SHA256 is
`e1ce11f7640783628b4cbf982c73ab20fa6b903ad8f7c9c094e3e2f06607f03e`.

Source: [wrapper](https://github.com/beicause/basisu_c_sys/tree/38f0c7e7e8b4d44ec9154d95dee14847711b6b98),
[native encoder](https://github.com/BinomialLLC/basis_universal/tree/9bebe16726b3a61c8c213eeee3b7cffb462ef34e).
The wrapper is MIT OR Apache-2.0; Basis Universal is Apache-2.0; the bundled
Zstandard implementation is BSD-licensed. Native licence texts are retained here;
copyright and third-party notices remain in the unmodified packaged sources.

The optional `ktx2` feature compiles C++ and uses bindgen/libclang. Default
builds do not compile this dependency. The wrapper's `transcode_default` feature
set is explicit: disabling XUASTC in this release removes `bc7u` definitions
while its unconditional XBC7 decoder still references them, breaking compilation.
Enabling these decoder capabilities does not change the selected output codec:
the encoder explicitly selects UASTC LDR 4x4 plus Zstandard.

The independent test-only `basisu` 0.1.0 decoder is a young pure-Rust port,
not a production dependency. It uses the stable `Transcoder` API and its
pure-Rust Zstandard feature. Independent decode checks and separate-process
SHA256 repeatability gates are required before accepting encoder output.
