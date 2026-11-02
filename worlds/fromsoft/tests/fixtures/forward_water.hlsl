// Fixture for resolve-fromsoft-shaders' engine-texture table (worlds/fromsoft/tests/resolve.rs): the frame inputs a forward
// water program reads besides the scene depth: the colour of the scene behind the surface under both its names
// (g_RefractionTexture at t55, SceneColor_0_Screen_0 at t45), the screen-space reflection at t44 (mixed over the probes'
// light by its alpha) and the water interaction normals at t57 (blended into the wave normal by their alpha).
//   fxc /T ps_5_0 /E main /Fo forward_water.dxbc forward_water.hlsl
Texture2D g_RLRTexture : register(t44);
Texture2D SceneColor_0_Screen_0 : register(t45);
Texture2D g_RefractionTexture : register(t55);
Texture2D g_DepthTexture : register(t56);
Texture2D g_WaterSimulationNormalTexture : register(t57);
SamplerState SS_ClampLinear : register(s9);

float4 main(float4 position : SV_Position) : SV_Target0
{
    float2 uv = position.xy;
    float4 ripple = g_WaterSimulationNormalTexture.Sample(SS_ClampLinear, uv);
    float2 bent = uv + (ripple.xy * 2 - 1) * ripple.w * 0.01;
    float3 behind = g_RefractionTexture.Sample(SS_ClampLinear, bent).rgb + SceneColor_0_Screen_0.Sample(SS_ClampLinear, uv).rgb;
    float depth = g_DepthTexture.Sample(SS_ClampLinear, bent).x;
    float4 ssr = g_RLRTexture.Sample(SS_ClampLinear, uv);
    return float4(behind * exp(-depth) + ssr.rgb + (1 - ssr.a), 1);
}
