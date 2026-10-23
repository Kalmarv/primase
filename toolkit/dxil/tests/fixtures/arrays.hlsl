// Fixture for the DXIL lifter: a local array indexed by a value from a constant buffer, a constant table, and a
// dynamically indexed constant buffer array. Compile: dxc -T ps_6_0 -E main -Fo arrays.dxil arrays.hlsl
cbuffer Values : register(b0)
{
    float4 a;
    int4 n;
    float4 table[6];
};
struct Output
{
    float4 x : SV_Target0;
    uint4 k : SV_Target1;
};
static const float weights[5] = { 0.1, 0.2, 0.4, 0.2, 0.1 };
Output main(float4 position : SV_Position, float2 t : TEXCOORD0)
{
    Output o;
    float local[4] = { a.x, a.y, a.z, a.w };
    float s = 0.0;
    [loop]
    for (int i = 0; i < 4; i++)
        s += local[(i + n.x) & 3] * weights[i];
    o.x = float4(s, weights[n.y & 3], table[n.z].x, table[(n.w + 1) % 6].y);
    o.k = uint4(0, 0, 0, 0);
    return o;
}
