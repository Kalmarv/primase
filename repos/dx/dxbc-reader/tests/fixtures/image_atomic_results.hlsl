// Atomic operations on an unordered-access texture whose old value the program reads (one thread group of 4 threads,
// compute). Compiled with dxc -T cs_6_0 -E main (image_atomic_results.dxil, also -Od), and the DXBC copy with fxc -T cs_5_0.
// Thread x makes three requests, in this order, on texels of counts, and stores each old value to olds[(x, request)]:
//   Add of x + 1 to counts[(0, 0)]; Exchange with x + 10 of counts[(1, 0)]; Max of x * 3 into counts[(2, 0)].
RWTexture2D<uint> counts : register(u0);
RWTexture2D<uint> olds : register(u1);

[numthreads(4, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint added;
    InterlockedAdd(counts[uint2(0, 0)], id.x + 1, added);
    olds[uint2(id.x, 0)] = added;
    uint exchanged;
    InterlockedExchange(counts[uint2(1, 0)], id.x + 10, exchanged);
    olds[uint2(id.x, 1)] = exchanged;
    uint maxed;
    InterlockedMax(counts[uint2(2, 0)], id.x * 3, maxed);
    olds[uint2(id.x, 2)] = maxed;
}
