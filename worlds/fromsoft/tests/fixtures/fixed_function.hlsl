// Fixture for resolve-fromsoft-shaders' fixed-function (GXFlver_*) materials (worlds/fromsoft/tests/resolve.rs): textures
// named as the material's own kinds (g_DiffuseTexture, g_BumpmapTexture) at the registers the real programs give them, the
// engine's dither noise, the tiling-scale and material-parameter buffers, and the G-buffer's five targets.
//   fxc /T ps_5_0 /E main /Fo fixed_function.dxbc fixed_function.hlsl
Texture2D g_DiffuseTexture : register(t0);
Texture2D g_BumpmapTexture : register(t3);
Texture2D g_DitherTexture : register(t38);
SamplerState SS_DiffuseMap : register(s0);
cbuffer cbTextureTilingScale : register(b1)
{
    float4 FC_TextureTilingScale[25];
};
cbuffer cbMtdParam : register(b3)
{
    float4 FC_DifMapMulCol;
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
    float4 a = g_DiffuseTexture.Sample(SS_DiffuseMap, i.uv.xy * FC_TextureTilingScale[0].xy);
    float4 n = g_BumpmapTexture.Sample(SS_DiffuseMap, i.uv.xy * FC_TextureTilingScale[3].xy);
    float4 d = g_DitherTexture.Sample(SS_DiffuseMap, i.position.xy);
    o.normal = float4(n.xyz, 1);
    o.gloss = d;
    o.albedo = a * FC_DifMapMulCol;
    o.reflectance = float4(0.04, 0.04, 0.04, 1);
    o.emission = float4(0, 0, 0, 1);
    return o;
}
