// Fixture for resolve-fromsoft-shaders' material-buffer rules (worlds/fromsoft/tests/resolve.rs): the engine-named fields
// of Sekiro's cbMtdParam (b3) where the real programs have them: FC_DifMapMulCol at 0, FC_AlphaRef at 80, FC_GlowScale at 144.
// It computes nothing meaningful; every field is used so the compiler keeps it.
//   fxc /T ps_5_0 /E main /Fo layered_mtd.dxbc layered_mtd.hlsl
cbuffer cbMtdParam : register(b3)
{
    float4 FC_DifMapMulCol;
    float4 unused_a[4];
    float4 FC_AlphaRef;
    float4 unused_b[3];
    float4 FC_GlowScale;
};

struct Input
{
    float4 position : SV_Position;
    float4 uv : TEXCOORD0;
};
struct Output
{
    float4 normal : SV_Target0;
    float4 gloss : SV_Target1;
    float4 albedo : SV_Target2;
    float4 reflectance : SV_Target3;
    float4 emission : SV_Target4;
};

Output main(Input i)
{
    Output o;
    o.normal = float4(0.5, 0.5, 1, 1);
    o.gloss = FC_GlowScale;
    o.albedo = FC_DifMapMulCol * i.uv + FC_AlphaRef + unused_a[0] + unused_b[0];
    o.reflectance = float4(0.04, 0.04, 0.04, 1);
    o.emission = float4(0, 0, 0, 1);
    return o;
}
