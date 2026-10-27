// Fixture for the DXIL lifter: texture operations. Compile: dxc -T ps_6_0 -E main -Fo textures.dxil textures.hlsl
Texture2D<float4> t2 : register(t0);
Texture2DArray<float4> ta : register(t1);
Texture3D<float4> t3 : register(t2);
TextureCube<float4> tc : register(t3);
Texture2D<float> depth : register(t4);
Texture2D<uint4> ti : register(t5);
SamplerState lin : register(s0);
SamplerComparisonState cmp : register(s1);
cbuffer Values : register(b0)
{
    float4 a;
    int4 n;
};
struct Output
{
    float4 x : SV_Target0;
    float4 y : SV_Target1;
    float4 z : SV_Target2;
    uint4 k : SV_Target3;
};
Output main(float4 position : SV_Position, float3 uvw : TEXCOORD0)
{
    Output o;
    o.x = t2.Sample(lin, uvw.xy) + t2.SampleBias(lin, uvw.xy, a.x) + t2.SampleLevel(lin, uvw.xy, a.y);
    o.y = ta.Sample(lin, float3(uvw.xy, 2.0)) + t3.SampleLevel(lin, uvw, 1.0) + tc.SampleLevel(lin, uvw, 0.0);
    o.z = t2.SampleGrad(lin, uvw.xy, ddx(uvw.xy), ddy(uvw.xy)) + t2.Gather(lin, uvw.xy);
    o.z.x += depth.SampleCmp(cmp, uvw.xy, a.z) + depth.SampleCmpLevelZero(cmp, uvw.xy, a.w);
    uint w, h, lv;
    t2.GetDimensions(0, w, h, lv);
    o.k = ti.Load(int3(n.xy, 0)) + uint4(w, h, lv, 0);
    o.x += t2.Load(int3(n.zw, 0)) + t2.Sample(lin, uvw.xy, int2(1, -1)) + ddx_fine(o.y) + ddy_coarse(o.y);
    return o;
}
