// Fixture for resolve-fromsoft-shaders' vertex stage (worlds/fromsoft/tests/resolve.rs): the shape of a layered
// shader's G-buffer vertex program, with its inputs (a packed normal and tangent, the packed first UV sets, the Byte4C
// members 1 to 6 as TEXCOORD1 to TEXCOORD6, a second colour, the instance index), its outputs (the pixel program's
// inputs by label) and the engine's constants by their own names (the texture scroll read as the real program does:
// each set times `xy` over 2048, plus `zw`; the third and fourth unused). It computes nothing meaningful.
//   fxc /T vs_5_0 /E main /Fo layered_vs.dxbc layered_vs.hlsl
cbuffer cbInstanceData : register(b4)
{
    float4 VC_InstanceData[4];
};
cbuffer cbObjMatrix : register(b5)
{
    float4 VC_aObjMatrix[12];
};
cbuffer cbTexScroll : register(b11)
{
    float4 VC_TexScroll_0;
    float4 VC_TexScroll_1;
    float4 VC_TexScroll_2;
    float4 VC_TexScroll_3;
};
cbuffer cbSceneParam : register(b8)
{
    float4 SC_CameraPos;
    float4 FC_ScreenSize;
    float4x4 FC_MatrixView;
    float4x4 VC_MatrixViewProj;
    float4 FC_ShadowColor;
};

struct Input
{
    float3 position : POSITION0;
    uint4 normal : NORMAL0;
    uint4 tangent : TANGENT0;
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
    uint k = i.instance * 3;
    float4 p = float4(i.position, 1);
    float4 w = float4(dot(VC_aObjMatrix[k], p), dot(VC_aObjMatrix[k + 1], p), dot(VC_aObjMatrix[k + 2], p), 1);
    o.clip = mul(VC_MatrixViewProj, w);
    o.world = w + SC_CameraPos * FC_ScreenSize + mul(FC_MatrixView, w) * FC_ShadowColor + VC_InstanceData[i.instance & 3];
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
