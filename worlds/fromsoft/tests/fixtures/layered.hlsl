// Fixture for resolve-fromsoft-shaders (worlds/fromsoft/tests/resolve.rs), built to Sekiro's naming: the engine's
// frame, scene and object constants, the material's `<shader>_snp_*` parameters and per-texture tiling, textures
// named Texture2D_<slot>, and the G-buffer's five render targets. It computes nothing meaningful; every constant is
// used so the compiler keeps it.
//   fxc /T ps_5_0 /E main /Fo layered.dxbc layered.hlsl
cbuffer Engine : register(b0)
{
    float4 FC_DifMapMulCol;
    float4 mWorld[3];
    float4 aColor;
    float FC_ColorSlotNo;
    float FC_GlobalTime;
    float FC_TimeOfDay;
    float SC_PreExposure;
    float4 SC_RenderCameraPos;
    float4x4 FC_MatrixInvViewProj;
};
cbuffer Material : register(b1)
{
    float4 FC_TextureTilingScale[10];
    float2 M_Multiple__snp_scale;
    float3 M_Multiple__snp_tint_color_0;
    float M_Multiple__snp_PADDING;
    float M_Multiple__snp_missing;
};
Texture2D Texture2D_8 : register(t8);
Texture2D Texture2D_2 : register(t2);
SamplerState linear_wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float4 uv : TEXCOORD0;
    float4 weights : TEXCOORD5;
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
    float2 uv0 = i.uv.xy * FC_TextureTilingScale[8].xy;
    float2 uv1 = i.uv.zw * FC_TextureTilingScale[2].xy;
    float4 a = Texture2D_8.Sample(linear_wrap, uv0);
    float4 n = Texture2D_2.Sample(linear_wrap, uv1);
    float4 world = mWorld[0] + mWorld[1] + mWorld[2];
    float4 engine = FC_DifMapMulCol * aColor + world + SC_RenderCameraPos + mul(FC_MatrixInvViewProj, i.position);
    float frame = FC_ColorSlotNo + FC_GlobalTime + FC_TimeOfDay + SC_PreExposure;
    float3 material = float3(M_Multiple__snp_scale, M_Multiple__snp_missing) + M_Multiple__snp_tint_color_0
        + M_Multiple__snp_PADDING;
    o.normal = float4(n.xyz * 0.5 + 0.5, 1);
    o.gloss = float4(frame, 0, 0, 0);
    o.albedo = float4(a.rgb * material, 1) + engine * i.weights;
    o.reflectance = float4(0.04, 0.04, 0.04, 1);
    o.emission = float4(0, 0, 0, 1);
    return o;
}
