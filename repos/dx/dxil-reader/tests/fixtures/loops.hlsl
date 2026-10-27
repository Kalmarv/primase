// Fixture for the DXIL lifter: loops with a bound from a constant buffer, break, continue, a nested loop and a
// loop with two exits. Compile: dxc -T ps_6_0 -E main -Fo loops.dxil loops.hlsl
cbuffer Values : register(b0)
{
    float4 a;
    int4 n;
};
struct Output
{
    float4 x : SV_Target0;
    uint4 k : SV_Target1;
};
Output main(float4 position : SV_Position, float2 t : TEXCOORD0)
{
    Output o;
    float sum = 0.0;
    uint count = 0;
    [loop]
    for (int i = 0; i < n.x; i++)
    {
        if (i == n.y)
            continue;
        if (sum > a.w)
            break;
        sum += a.x * (float)i + t.x;
        count++;
    }
    o.x = float4(sum, 0, 0, 1);
    o.k = uint4(count, 0, 0, 0);
    // a nested loop with a second way out
    uint inner = 0;
    [loop]
    for (int j = 0; j < n.z; j++)
    {
        [loop]
        for (int m = 0; m < 4; m++)
        {
            inner += (uint)(j * 4 + m);
            if (inner > (uint)n.w)
                break;
        }
        if (inner > 1000u)
            break;
    }
    o.k.y = inner;
    // a while loop with the test at the top
    float v = t.y;
    int guard = 0;
    [loop]
    while (v < a.y && guard < 16)
    {
        v = v * 1.5 + 0.25;
        guard++;
    }
    o.x.y = v;
    o.k.z = (uint)guard;
    return o;
}
