// Fixture for derive-fromsoft-water's measurement (worlds/fromsoft/tests/water_bodies.rs): a forward water program of
// known optics, named as Elden Ring's programs name the engine's values. What is behind fades as exp(-SIGMA * d) over
// the depth of water d (from the frame's depth, linearised by FC_DepthComputeParam), the body returns BODY * E / pi of
// the sun's irradiance E, both pass the surface weighted by 1 - F (Schlick, F0), the opacity rises over 1 / SHORE metres,
// and one normal map at tiling TILE scrolling SCROLL per second tilts the normal by STRENGTH.
//   fxc /T ps_5_0 /E main /Fo water_body.dxbc water_body.hlsl
static const float3 SIGMA = float3(0.3, 0.2, 0.1);
static const float F0 = 0.02;
static const float3 BODY = float3(0.05, 0.1, 0.15);
static const float SHORE = 2.0;
static const float TILE = 3.0;
static const float2 SCROLL = float2(0.01, -0.02);
static const float STRENGTH = 0.5;

cbuffer cbSceneParam : register(b8)
{
    float4 FC_DepthComputeParam;
    float4 SC_RenderCameraPos;
};
cbuffer cbPerFrame : register(b9)
{
    float FC_GlobalTime;
};
cbuffer cbLight : register(b5)
{
    float3 Vec_0;
    float pad0;
    float3 Col_0;
    float pad1;
};
Texture2D Fixture_snp_Texture2D_0_NormalMap : register(t0);
Texture2D g_RefractionTexture : register(t55);
Texture2D g_DepthTexture : register(t56);
SamplerState SS_WrapLinear : register(s8);
SamplerState SS_ClampPoint : register(s7);

float4 main(float4 position : SV_Position, float4 world : POSITION1, float4 uv : TEXCOORD0) : SV_Target0
{
    float raw = g_DepthTexture.SampleLevel(SS_ClampPoint, position.xy, 0).x;
    float behind = FC_DepthComputeParam.z / (raw - FC_DepthComputeParam.x);
    float d = max(0, behind - world.w);
    float3 t = Fixture_snp_Texture2D_0_NormalMap.Sample(SS_WrapLinear, uv.xy * TILE + SCROLL * FC_GlobalTime).xyz * 2 - 1;
    float2 tilt = t.xy * STRENGTH;
    float3 n = normalize(float3(tilt.x, sqrt(saturate(1 - dot(tilt, tilt))), tilt.y));
    float3 v = normalize(SC_RenderCameraPos.xyz - world.xyz);
    float f = F0 + (1 - F0) * pow(1 - saturate(dot(n, v)), 5);
    float3 transmitted = exp(-SIGMA * d);
    float3 refraction = g_RefractionTexture.SampleLevel(SS_ClampPoint, position.xy, 0).rgb;
    float3 body = BODY * Col_0 * saturate(dot(n, Vec_0)) / 3.14159265;
    float3 colour = (refraction * transmitted + body * (1 - transmitted)) * (1 - f);
    return float4(colour, saturate(d * SHORE));
}
