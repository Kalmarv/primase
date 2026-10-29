// Fixture for bake-fromsoft-materials' carried alpha test (src/sekiro/passes/bake_materials/cut.rs): Sekiro's
// cbMtdParam (b3) with FC_AlphaRef at 80, and a discard of the product of two textures' alphas against its x, as
// Elden Ring's C[DetailBlend]_0_Gbuf_[A] programs test their two albedo maps. The value tested is written to no
// target (the albedo's alpha is 1), so no output alpha is the cut.
//   fxc /T ps_5_0 /E main /Fo alpha_product.dxbc alpha_product.hlsl
// (fxc 10.1, of the Windows 10 SDK 10.0.26100.0)
cbuffer cbMtdParam : register(b3)
{
    float4 FC_DifMapMulCol;
    float4 unused_a[4];
    float4 FC_AlphaRef;
};

Texture2D first : register(t0);
Texture2D second : register(t1);
SamplerState linear_wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};
struct Output
{
    float4 normal : SV_Target0;
    float4 gloss : SV_Target1;
    float4 albedo : SV_Target2;
};

Output main(Input i)
{
    float4 a = first.Sample(linear_wrap, i.uv);
    float4 b = second.Sample(linear_wrap, i.uv);
    if (a.w * b.w < FC_AlphaRef.x)
        discard;
    Output o;
    o.normal = float4(0.5, 0.5, 1, 1);
    o.gloss = float4(0.5, 0, 0, 1);
    o.albedo = float4(FC_DifMapMulCol.rgb * a.rgb * b.rgb, 1);
    return o;
}
