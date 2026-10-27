// Arrays of buffers read and written at a computed element (compute, one group of 4). Compiled with dxc -T cs_6_0 -E
// main (buffer_arrays.dxil, also -Od and -T cs_6_6). Bind: bufs[0..3] with n0, n1, n2 words (at least 8 for bufs[2],
// at least 4 for bufs[which]), all zero; raws[0] = 10, 20, 30, 40 and raws[1] = 11, 21, 31, 41; which = 1.
// Thread i (0 to 3) then:
//   n, stride = bufs[1].GetDimensions()         -> n = n1, stride = 4
//   bufs[1][i] = n1 * 100 + i
//   InterlockedAdd(bufs[2][0], i + 1)           -> bufs[2][0] = 1 + 2 + 3 + 4 = 10 after the group, each thread's old
//                                                  value the sum of the lanes before it in the order they ran
//   bufs[2][4 + i] = raws[1].Load(4 * i)        -> bufs[2][4..8] = 11, 21, 31, 41
RWStructuredBuffer<uint> bufs[3] : register(u0, space1);
ByteAddressBuffer raws[2] : register(t0, space1);

cbuffer Select : register(b0) {
    uint which;
};

[numthreads(4, 1, 1)]
void main(uint i : SV_GroupIndex) {
    uint count, stride;
    bufs[which].GetDimensions(count, stride);
    bufs[which][i] = count * 100 + i;
    uint old;
    InterlockedAdd(bufs[(which + 1) % 3][0], i + 1, old);
    bufs[2][4 + i] = raws[which & 1].Load(4 * i);
}
