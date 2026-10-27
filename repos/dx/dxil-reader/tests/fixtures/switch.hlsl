// Fixture for the DXIL lifter: a switch (with a default and a case shared by two labels) and a switch in a loop.
// Compile: dxc -T ps_6_0 -E main -Fo switch.dxil switch.hlsl
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
    o.x = float4(0, 0, 0, 1);
    o.k = uint4(0, 0, 0, 0);
    switch (n.x)
    {
    case 0:
        o.x.x = a.x;
        break;
    case 1:
    case 2:
        o.x.x = a.y + t.x;
        break;
    case 5:
        o.x.x = a.z * t.y;
        break;
    default:
        o.x.x = -1.0;
        break;
    }
    uint acc = 0;
    [loop]
    for (int i = 0; i < n.y; i++)
    {
        switch (i & 3)
        {
        case 0: acc += 1; break;
        case 1: acc += 10; continue;
        case 2: acc += 100; break;
        default: acc += 1000; break;
        }
        acc += 5;
    }
    o.k.x = acc;
    return o;
}
