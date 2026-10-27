// Atomic operations on unordered-access textures from one thread group of 8 by 8 (compute). Compiled with
//   dxc -T cs_6_0 -E main (image_atomics.dxil, also -Od and -T cs_6_6), and the DXBC copy with fxc -T cs_5_0.
// Thread (x, y) of the group has at = (x % 4, y % 4) and makes three requests, in this order:
//   Add of x + 1 to counts[at]; Max of x * 10 + y to counts[at + (0, 4)]; Min of x * 10 + y to counts[at + (4, 0)]
// and a signed Max of x - 4 to signed_counts[at].
RWTexture2D<uint> counts : register(u0);
RWTexture2D<int> signed_counts : register(u1);

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint2 at = uint2(id.x % 4, id.y % 4);
    InterlockedAdd(counts[at], id.x + 1);
    InterlockedMax(counts[at + uint2(0, 4)], id.x * 10 + id.y);
    InterlockedMin(counts[at + uint2(4, 0)], id.x * 10 + id.y);
    InterlockedMax(signed_counts[at], (int)id.x - 4);
}
