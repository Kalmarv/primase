// Fixture for resolve-fromsoft-shaders' vertex binding of a program that takes ONE texture coordinate set (the non-layered
// SAT shaders': MetallicValue, MetallicTex...): TEXCOORD0 is two shorts, not the layered programs' packed pair.
//   fxc /T vs_5_0 /E main /Fo single_uv_vs.dxbc single_uv_vs.hlsl
cbuffer cbSceneParam : register(b8)
{
    float4x4 VC_MatrixViewProj;
};
struct Input
{
    float3 position : POSITION0;
    int2 uv : TEXCOORD0;
    uint instance : SV_InstanceID;
};
struct Output
{
    float4 clip : SV_Position;
    float4 uv : TEXCOORD0;
};
Output main(Input i)
{
    Output o;
    o.clip = mul(VC_MatrixViewProj, float4(i.position, 1));
    o.uv = float4(float2(i.uv) / 2048.0, 0, float(i.instance));
    return o;
}
