# The DXBC container

`chunks(bytes)` reads the container Direct3D shader programs are stored in, Shader Model 4 to 6: a header (`DXBC`, a
16-byte digest, version 1, the container's size, the number of parts), the directory of the parts' offsets, then the parts
one after another in the directory's order (each its four-character kind, its length and its bytes), the last ending where
the container does. That is the layout of every one of the 77,956 containers of the oracle shader corpora and the
committed fixtures; Shader Model 6 parts are not padded, so an offset need not be a multiple of 4. The reader reads to
the exact end and refuses, naming the byte, a part anywhere else (overlapping or after a gap), a byte after the last part,
a kind twice, a size other than the container's and a version other than 1. The digest is not verified: it is not an
authenticity check. `checksum(bytes)` computes it (MD5's block function over the bytes from 20 on, padded as the compiler
pads), for a caller that wants to know it.

`primase-dxbc` (programs of Shader Model 4 and 5) and `primase-dxil` (Shader Model 6) read their containers with it, and
map its error into their own refusal at the container's byte; callers that read a container's parts themselves use it
directly. `tests/container.rs` builds containers by hand for each refusal and reads the lifters' committed fixtures.
