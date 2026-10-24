// Fixture for resolve-fromsoft-shaders' palette rule (worlds/fromsoft/tests/resolve.rs): the shape of Elden Ring's LOD
// billboard vertex programs (GXFlver_Slod_Billboard_Gbuf), which read only the joint's translation of VC_aObjMatrix
// (the last word of each of its three rows) and add it to a camera-facing offset: the position is never multiplied by
// the joint's matrix, so the program does not show how the palette's lines hold it. The registers are the fixture's,
// not claimed to be the game's. It computes nothing meaningful beyond the position.
//   fxc /T vs_5_0 /E main /Fo layered_billboard_vs.dxbc layered_billboard_vs.hlsl   (fxc 10.0.26100.0)
cbuffer cbObjMatrix : register(b5)
{
    float4 VC_aObjMatrix[12];
};
cbuffer cbSceneParam : register(b8)
{
    float4 SC_CameraPos;
    float4 SC_CameraRight;
    float4 SC_CameraUp;
    float4x4 VC_MatrixViewProj;
};

struct Input
{
    float3 position : POSITION0;
    uint4 normal : NORMAL0;
};
struct Output
{
    float4 clip : SV_Position;
    float4 world : POSITION1;
};

Output main(Input i)
{
    Output o;
    uint b = i.normal.w * 3;
    float3 centre = float3(VC_aObjMatrix[b].w, VC_aObjMatrix[b + 1].w, VC_aObjMatrix[b + 2].w);
    float3 world = centre + i.position.x * SC_CameraRight.xyz + i.position.y * SC_CameraUp.xyz;
    o.clip = mul(VC_MatrixViewProj, float4(world - SC_CameraPos.xyz, 1));
    o.world = float4(world, 1);
    return o;
}
