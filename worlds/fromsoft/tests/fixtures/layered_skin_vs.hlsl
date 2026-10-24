// Fixture for resolve-fromsoft-shaders' skinned vertex permutation (worlds/fromsoft/tests/resolve.rs): the shape of a layered
// shader's `_Gbuf_Skin` vertex program, as `layered_vs.hlsl` is of the unskinned one, with the joint indices and weights as
// inputs and VC_aObjMatrix as the joints' palette (three rows of four per joint) the program blends, after the position
// is taken through one of VC_aClothCancelObjMatrix's matrices (a second, short palette the real programs apply first). It
// computes nothing meaningful beyond the blend.
//   fxc /T vs_5_0 /E main /Fo layered_skin_vs.dxbc layered_skin_vs.hlsl
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
cbuffer cbClothCancelObjMatrix : register(b12)
{
    column_major float4x3 VC_aClothCancelObjMatrix[2];
};
cbuffer cbSceneParam : register(b8)
{
    float4 SC_CameraPos;
    float4 FC_ScreenSize;
    float4x4 FC_MatrixView;
    float4x4 VC_MatrixViewProj;
    float4 FC_ShadowColor;
};
cbuffer cbTexScroll : register(b11)
{
    float4 VC_TexScroll_0;
    float4 VC_TexScroll_1;
    float4 VC_TexScroll_2;
    float4 VC_TexScroll_3;
};

struct Input
{
    float3 position : POSITION0;
    uint4 normal : NORMAL0;
    uint4 tangent : TANGENT0;
    uint4 joints : BLENDINDICES0;
    float4 weights : BLENDWEIGHT0;
    float4 colour : COLOR1;
    int4 uv : TEXCOORD0;
    float4 m1 : TEXCOORD1;
    float4 m2 : TEXCOORD2;
    float4 m3 : TEXCOORD3;
    float4 m4 : TEXCOORD4;
    float4 m5 : TEXCOORD5;
    float4 m6 : TEXCOORD6;
    uint instance : SV_InstanceID;
};
struct Output
{
    float4 clip : SV_Position;
    float4 world : POSITION1;
    float4 normal : NORMAL0;
    float4 tangent : TANGENT0;
    float4 uv : TEXCOORD0;
    float4 weights : TEXCOORD5;
    float4 tint : TEXCOORD7;
    float4 detail : TEXCOORD12;
};

Output main(Input i)
{
    Output o;
    float4 w = i.weights / max(dot(float4(1, 1, 1, 1), i.weights), 0.001);
    uint4 b = (i.joints + VC_InstanceData[0].matricesData.x) * 3;
    float4 p = float4(mul(float4(i.position, 1), VC_aClothCancelObjMatrix[(i.joints.x + VC_InstanceData[0].matricesData.x) & 1]), 1);
    float4 row0 = VC_aObjMatrix[b.x] * w.x + VC_aObjMatrix[b.y] * w.y + VC_aObjMatrix[b.z] * w.z + VC_aObjMatrix[b.w] * w.w;
    float4 row1 = VC_aObjMatrix[b.x + 1] * w.x + VC_aObjMatrix[b.y + 1] * w.y + VC_aObjMatrix[b.z + 1] * w.z + VC_aObjMatrix[b.w + 1] * w.w;
    float4 row2 = VC_aObjMatrix[b.x + 2] * w.x + VC_aObjMatrix[b.y + 2] * w.y + VC_aObjMatrix[b.z + 2] * w.z + VC_aObjMatrix[b.w + 2] * w.w;
    float4 world = float4(dot(p, row0), dot(p, row1), dot(p, row2), 1);
    o.clip = mul(VC_MatrixViewProj, world);
    o.world = world + SC_CameraPos * FC_ScreenSize + mul(FC_MatrixView, world) * FC_ShadowColor + VC_InstanceData[i.instance & 1].userClipPlane;
    // the stored bytes as the real program decodes them: (byte - 127) / 127
    o.normal = float4((float3(i.normal.xyz) - 127.0) * 0.007874016, 0);
    o.tangent = float4((float3(i.tangent.xyz) - 127.0) * 0.007874016, (float(i.tangent.w) - 127.0) * 0.007874016);
    o.uv = float4(float2(i.uv.xy) * VC_TexScroll_0.xy / 2048.0 + VC_TexScroll_0.zw,
                  float2(i.uv.zw) * VC_TexScroll_1.xy / 2048.0 + VC_TexScroll_1.zw);
    o.weights = float4(i.m1.x, i.m2.z, i.m4.x, i.m5.y);
    o.tint = float4(i.m3.xyz, i.colour.x);
    o.detail = float4(i.m6.z, 0, 0, 0);
    return o;
}
