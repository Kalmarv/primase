// Fixture for resolve-fromsoft-shaders' forward-pass engine textures (worlds/fromsoft/tests/resolve.rs): the scene depth the fog,
// foam and water programs read at t56 (hardware depth in x, linearised with the scene buffer's FC_DepthComputeParam: view depth =
// -z / (x_param - depth)) and the SSAO texture at t33, which no material binds.
//   fxc /T ps_5_0 /E main /Fo forward.dxbc forward.hlsl
Texture2D g_DepthTexture : register(t56);
Texture2D g_SSAOTexture : register(t33);
SamplerState SS_ClampLinear : register(s9);
cbuffer cbSceneParam : register(b8)
{
    float4 FC_DepthComputeParam;
};

float4 main(float4 position : SV_Position) : SV_Target0
{
    float depth = g_DepthTexture.Sample(SS_ClampLinear, position.xy).x;
    float view = -FC_DepthComputeParam.z / (FC_DepthComputeParam.x - depth);
    float ao = g_SSAOTexture.Sample(SS_ClampLinear, position.xy).x;
    return float4(view, ao, 0, 1);
}
