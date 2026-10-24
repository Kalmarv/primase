// Fixture for resolve-fromsoft-shaders' matrix lines (worlds/fromsoft/tests/resolve.rs): the shape of Sekiro's `_cloth_Gbuf`
// vertex programs, whose palette rows are chosen by a flag, row by row before the position is dotted with them (where
// layered_vs.hlsl -D CLOTH chooses between two positions made whole).
// With VC_isEnableCloth off the position goes through the joint's
// matrix of VC_aObjMatrix (three rows of four per joint, the joint index in NORMAL0.w plus the instance's matrix base); with
// it on, through the model's VC_World. The branch makes each row a merge of the palette's word and VC_World's, which the
// matrix lines are read through. The registers of cbMatDynParam and cbvModelParam are the fixture's, not claimed to be the
// game's. It computes nothing meaningful beyond the position.
//   fxc /T vs_5_0 /E main /Fo layered_cloth_rows_vs.dxbc layered_cloth_rows_vs.hlsl
struct Instance
{
    float4 mWorld[3];
    float4 userClipPlane;
    uint4 matricesData;
};
cbuffer cbInstanceData : register(b4)
{
    Instance VC_InstanceData[2];
};
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
    uint4 normal : NORMAL0;
    uint instance : SV_InstanceID;
};
struct Output
{
    float4 clip : SV_Position;
    float4 world : POSITION1;
};

Output main(Input i)
{
    Output o;
    uint b = (i.normal.w + VC_InstanceData[i.instance & 1].matricesData.x) * 3;
    float4 row0, row1, row2;
    [branch] if (VC_isEnableCloth)
    {
        row0 = VC_World[0];
        row1 = VC_World[1];
        row2 = VC_World[2];
    }
    else
    {
        row0 = VC_aObjMatrix[b];
        row1 = VC_aObjMatrix[b + 1];
        row2 = VC_aObjMatrix[b + 2];
    }
    float4 p = float4(i.position, 1);
    float4 world = float4(dot(p, row0), dot(p, row1), dot(p, row2), 1);
    o.clip = mul(VC_MatrixViewProj, float4(world.xyz - SC_CameraPos.xyz, 1));
    o.world = world;
    return o;
}
