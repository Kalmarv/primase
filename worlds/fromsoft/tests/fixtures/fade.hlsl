// Fixture for resolve-fromsoft-shaders' dither and camera-fade constants (worlds/fromsoft/tests/resolve.rs): the five fields an
// alpha-tested G-buffer program reads from the per-draw buffers, at the offsets the real programs give them (cbMatDynParam's
// FC_isDitherAlpha at 0 and FC_isEnableCameraDitherFade at 4, cbModelParam's FC_DitherEnable at 32, FC_CameraFadeEnd at 36
// and FC_CameraFadeInvRange at 40), read as the real programs read them: the camera fade discards where the fragment's
// distance times the inverse range minus the end is below the dither noise, the dither alpha where the instance's amount is.
//   fxc /T ps_5_0 /E main /Fo fade.dxbc fade.hlsl
cbuffer cbMatDynParam : register(b2)
{
    float FC_isDitherAlpha;
    float FC_isEnableCameraDitherFade;
};
cbuffer cbModelParam : register(b7)
{
    float4 FC_DifColMul;
    float4 FC_EnvMapRatios;
    float FC_DitherEnable;
    float FC_CameraFadeEnd;
    float FC_CameraFadeInvRange;
};

struct Input
{
    float4 position : SV_Position;
    float4 world : TEXCOORD0;
};

float4 main(Input i) : SV_Target0
{
    float noise = frac(i.position.x * 0.125) * 0.99 + 0.01;
    if (FC_isEnableCameraDitherFade != 0 && i.world.w * FC_CameraFadeInvRange - FC_CameraFadeEnd < noise)
        discard;
    if (FC_DitherEnable > 0 && FC_isDitherAlpha != 0 && i.world.x < noise)
        discard;
    return float4(1, 1, 1, 1);
}
