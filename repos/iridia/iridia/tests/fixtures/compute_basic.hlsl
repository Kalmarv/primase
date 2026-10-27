// A compute program: a thread group of 8 by 4 by 2, the invocation ids, thread-group memory, barriers, atomics on
// thread-group memory and on a buffer, and read-write buffers of each kind. Compiled with dxc -T cs_6_0 (also -Od,
// and -T cs_6_6).
RWByteAddressBuffer raw : register(u0);
RWStructuredBuffer<uint> words : register(u1);
RWBuffer<float4> typed : register(u2);

cbuffer Values : register(b0) {
    uint scale;
};

groupshared uint tile[64];
groupshared uint counter;

[numthreads(8, 4, 2)]
void main(uint3 group : SV_GroupID, uint3 local : SV_GroupThreadID, uint3 id : SV_DispatchThreadID,
          uint index : SV_GroupIndex) {
    if (index == 0) {
        counter = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    tile[index] = id.x * 100 + id.y * 10 + id.z;
    InterlockedAdd(counter, index);
    GroupMemoryBarrierWithGroupSync();
    // words[0] counts the invocations; each group writes the mirror of its tile plus the sum of the indices
    uint before;
    InterlockedAdd(words[0], 1, before);
    uint g = group.x + 2 * group.y;
    words[1 + index + 64 * g] = tile[63 - index] + counter;
    raw.Store(4 * (index + 64 * g), tile[index] ^ scale);
    typed[index + 64 * g] = float4(local, index);
}
