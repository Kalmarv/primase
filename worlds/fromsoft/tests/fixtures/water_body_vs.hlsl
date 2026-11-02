// Fixture for derive-fromsoft-water's measurement (worlds/fromsoft/tests/water_bodies.rs): the vertex program of the
// fixture water (water_body.hlsl), shaped as Elden Ring's forward vertex programs hand a surface on: the clip position
// from the world position minus the camera's through the camera-relative view-projection, the world position with the
// depth along the view axis (clip w) as POSITION1, and the texture coordinate stored as shorts (uv * 2048).
//   fxc /T vs_5_0 /E main /Fo water_body_vs.dxbc water_body_vs.hlsl   (fxc 10.0.19041)
cbuffer cbSceneParam : register(b8)
{
    float4 SC_CameraPos;
    float4x4 VC_MatrixViewProj;
};

struct Input
{
    float3 position : POSITION0;
    int2 uv : TEXCOORD0;
};
struct Output
{
    float4 clip : SV_Position;
    float4 world : POSITION1;
    float4 uv : TEXCOORD0;
};

Output main(Input i)
{
    Output o;
    o.clip = mul(VC_MatrixViewProj, float4(i.position - SC_CameraPos.xyz, 1));
    o.world = float4(i.position, o.clip.w);
    o.uv = float4(float2(i.uv) / 2048.0, 0, 0);
    return o;
}
