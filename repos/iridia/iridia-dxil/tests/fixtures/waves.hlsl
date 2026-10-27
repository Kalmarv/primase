// Wave operations in a one-dimensional thread group of 64 invocations, each writing 13 words of results. The values
// below are for a wave of 32 lanes (invocation i is lane i % 32 of wave i / 32). Compiled with dxc -T cs_6_0 (also
// -Od, and -T cs_6_6).
RWStructuredBuffer<uint> results : register(u0);

[numthreads(64, 1, 1)]
void main(uint index : SV_GroupIndex) {
    uint lane = WaveGetLaneIndex();
    uint wave = index / 32;
    uint at = index * 13;
    results[at + 0] = lane;                                   // index % 32
    results[at + 1] = WaveActiveSum(index);                   // 496 in the first wave, 1520 in the second
    results[at + 2] = WaveActiveMin(index);                   // 0, 32
    results[at + 3] = WaveActiveMax(index);                   // 31, 63
    results[at + 4] = WaveActiveBitOr(1u << (index % 32));    // 0xffffffff
    results[at + 5] = WaveActiveAllTrue(index < 48);          // 1, 0
    results[at + 6] = WaveActiveAnyTrue(index == 40);         // 0, 1
    results[at + 7] = WaveActiveBallot(index % 2 == 0).x;     // 0x55555555
    results[at + 8] = WavePrefixSum(index);                   // lane * (lane - 1) / 2 + 32 * wave * lane
    results[at + 9] = WaveReadLaneFirst(index);               // 0, 32
    results[at + 10] = WaveReadLaneAt(index, 5);              // 5, 37
    results[at + 11] = QuadReadAcrossX(index);                // index ^ 1
    results[at + 12] = WaveIsFirstLane();                     // lane == 0
}
