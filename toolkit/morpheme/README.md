# Morpheme reference rigs

`bundle_be32` reads the 32-bit big-endian simple-bundle layout: a 32-byte header
(`u32` type, `u32` ID, 16-byte GUID, `u32` payload length, one retained word), then
the payload padded to 16 bytes. Every record is bounded, including unsupported
asset types, and traversal must end exactly at EOF. Padding is retained without
requiring zeroes. There is no claim of support for other Morpheme layouts.

`rig_be32` reads type 1 payloads. Hierarchy and reference joint indices retain
source order and scoped identities. Name IDs address a byte string table; names
may repeat and need not be UTF-8. Relative-pointer bases are the rig, the string
table, the reference-attribute header and its data buffer, respectively. Runtime
pointers in the hierarchy are ignored as the native relocator replaces them.

Reference descriptors 2 and 6 hold 16-byte position and XYZW quaternion entries.
Only XYZ has a position meaning; the fourth lane is retained as raw bits. The
reader rejects missing/duplicate attributes, unsupported descriptor layouts,
invalid spans, nonfinite supported values, nonunit quaternions (length tolerance
0.001), missing name IDs, invalid parents and cycles. It never normalizes values.
The separate header orientation and raw header vector remain separate from joint locals;
the vector's words are retained without assigning a reference/bind-space meaning.

Auxiliary rig fields and network/motion interpretation are outside this codec.
Callers retain/report them; callers also own frame conversion, variant selection
and any connection to render skeletons. The crate performs no file I/O.

The literal two-joint fixture uses four distinct pointer bases, reordered name IDs,
noncommuting rotations, nonzero translations, junk runtime pointers/padding and a
bounded unknown sibling record. It supplies the decoding oracle. Native consumer
evidence is documented by the game reader, separately from corpus structure checks.
