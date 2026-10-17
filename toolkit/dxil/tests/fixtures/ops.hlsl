// Fixture for primase-dxil: integer arithmetic, comparisons, rounding and the unary intrinsics.
// Compile: dxc -T ps_6_0 -E main -Fo ops.dxil ops.hlsl
cbuffer Values : register(b0)
{
    float4 a;
    int4 n;
};
struct Output
{
    float4 x : SV_Target0;
    float4 y : SV_Target1;
    uint4 z : SV_Target2;
};
Output main(float4 position : SV_Position, float2 t : TEXCOORD0)
{
    Output o;
    float u = a.x;
    o.x = float4(frac(u + 0.25), exp2(a.y), log2(a.z), rsqrt(a.w));
    o.y = float4(round(u), floor(-u), ceil(u), trunc(-u));
    int i = n.x;
    uint k = (uint)n.y;
    o.z = uint4((uint)(i / 2), (uint)(i % 3), k << 3, (uint)(i >> 1));
    if (t.x > t.y)
    {
        o.x.w = (float)max(i, (int)k) + (float)min(k, 3u);
    }
    return o;
}
