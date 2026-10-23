// Unordered-access counters (compute): an append buffer, a consume buffer, a structured buffer whose counter is
// incremented and another whose counter is decremented (dxc refuses one buffer that does both). Compiled with
// dxc -T cs_6_0 -E main (uav_counter.dxil, also -Od and -T cs_6_6).
// Thread i of 8 appends i * 10 to `appended`, consumes one value from `consumed` into results[i], takes the old count of
// `counted` into results[8 + i] and decrements the counter of `drained`.
AppendStructuredBuffer<uint> appended : register(u0);
ConsumeStructuredBuffer<uint> consumed : register(u1);
RWStructuredBuffer<uint> counted : register(u2);
RWStructuredBuffer<uint> drained : register(u3);
RWStructuredBuffer<uint> results : register(u4);

[numthreads(8, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    appended.Append(id.x * 10);
    results[id.x] = consumed.Consume();
    results[8 + id.x] = counted.IncrementCounter();
    results[16 + id.x] = drained.DecrementCounter();
}
