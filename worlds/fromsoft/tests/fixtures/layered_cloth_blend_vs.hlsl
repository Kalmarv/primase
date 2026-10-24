// Fixture for the matrix lines of a cloth program that blends its joints (toolkit/shader/tests/matrix_lines.rs): the shape
// of Elden Ring's `_cloth_..._Skin` vertex programs, which blend four joints' matrices of VC_aObjMatrix word by word, each
// weighted by its normalized weight, and choose each blended word by VC_isEnableCloth against VC_World's before the
// position is dotted with them (where layered_cloth_rows_vs.hlsl chooses one joint's rows). The branch makes each row a
// merge of the blend and VC_World's word, which the matrix lines are read through. The registers are the fixture's, not
// claimed to be the game's. It computes nothing meaningful beyond the position.
//   dxc -T vs_6_0 -E main -Fo layered_cloth_blend_vs.dxil layered_cloth_blend_vs.hlsl   (dxc 1.7.2308.16)
//   fxc /T vs_5_0 /E main /Fo layered_cloth_blend_vs.dxbc layered_cloth_blend_vs.hlsl   (fxc 10.0.26100.0)
cbuffer cbObjMatrix : register(b5)
{
    float4 VC_aObjMatrix[12];
};
cbuffer cbMatDynParam : register(b6)
{
    bool VC_isEnableCloth;
};
cbuffer cbvModelParam : register(b7)
{
    float4 VC_World[3];
};
cbuffer cbSceneParam : register(b8)
{
    float4 SC_CameraPos;
    float4x4 VC_MatrixViewProj;
};

struct Input
{
    float3 position : POSITION0;
    uint4 joints : BLENDINDICES0;
    float4 weights : BLENDWEIGHT0;
};
struct Output
{
    float4 clip : SV_Position;
    float4 world : POSITION1;
};

Output main(Input i)
{
    Output o;
    float4 row0, row1, row2;
    [branch] if (VC_isEnableCloth)
    {
        row0 = VC_World[0];
        row1 = VC_World[1];
        row2 = VC_World[2];
    }
    else
    {
        float4 w = i.weights / max(dot(i.weights, 1), 0.001);
        row0 = 0;
        row1 = 0;
        row2 = 0;
        [unroll] for (uint k = 0; k < 4; k++)
        {
            uint b = i.joints[k] * 3;
            row0 += VC_aObjMatrix[b] * w[k];
            row1 += VC_aObjMatrix[b + 1] * w[k];
            row2 += VC_aObjMatrix[b + 2] * w[k];
        }
    }
    float4 p = float4(i.position, 1);
    float4 world = float4(dot(p, row0), dot(p, row1), dot(p, row2), 1);
    o.clip = mul(VC_MatrixViewProj, float4(world.xyz - SC_CameraPos.xyz, 1));
    o.world = world;
    return o;
}
