// Wave operations in a thread group that is not along x alone (compute), whose lane number WebGPU has no built-in for.
// Compiled with dxc -T cs_6_0 -E main (lane_2d.dxil, also -Od and -T cs_6_6). Each thread writes its lane in its wave,
// the number of lanes that are active with it, and the lowest lane's value of its flattened index.
RWStructuredBuffer<uint> results : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID, uint index : SV_GroupIndex) {
    uint lane = WaveGetLaneIndex();
    results[index * 3] = lane;
    results[index * 3 + 1] = WaveActiveCountBits(true);
    results[index * 3 + 2] = WaveReadLaneFirst(index);
}
