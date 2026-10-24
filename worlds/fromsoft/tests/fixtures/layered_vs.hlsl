// Fixture for resolve-fromsoft-shaders' vertex stage (worlds/fromsoft/tests/resolve.rs): the shape of a layered
// shader's G-buffer vertex program, with its inputs (a packed normal and tangent, the packed first UV sets, the Byte4C
// members 1 to 6 as TEXCOORD1 to TEXCOORD6, a second colour, the instance index), its outputs (the pixel program's
// inputs by label) and the engine's constants by their own names (the texture scroll read as the real program does:
// each set times `xy` over 2048, plus `zw`; the third and fourth unused). It computes nothing meaningful.
//   fxc /T vs_5_0 /E main /Fo layered_vs.dxbc layered_vs.hlsl
//   fxc /T vs_5_0 /E main /D RELATIVE /Fo layered_rel_vs.dxbc layered_vs.hlsl
//   fxc /T vs_5_0 /E main /D OFFSET /Fo layered_off_vs.dxbc layered_vs.hlsl
//   fxc /T vs_5_0 /E main /D CLOTH /Fo layered_cloth_vs.dxbc layered_vs.hlsl
//   fxc /T vs_5_0 /E main /D LINING /Fo layered_lining_vs.dxbc layered_vs.hlsl
// With LINING the program has the shape of the Lining_S2 family: TEXCOORD1 is a pair of integers (a third coordinate set)
// where the layered programs take four floats (a Byte4C member), and a second tangent, TANGENT2, is read.
// With CLOTH the program has the cloth permutation's shape: the switch VC_isEnableCloth (cbMatDynParam) chooses between the
// palette and the instance's world rows, the tangent's w comes from a one-byte BINORMAL0 input, and POSITION1 is declared
// and never read.
// With OFFSET a constant is added to the clip position, so the identity matrix does not give the position back (a program
// the camera probe cannot decide).
// With RELATIVE the clip position takes the camera away from the world position first, as most programs of Elden Ring do.
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
#ifdef CLOTH
cbuffer cbMatDynParam : register(b2)
{
    float4 VC_pad;
    bool VC_isEnableCloth;
};
#endif
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
#ifdef LINING
    int2 uv1 : TEXCOORD1;
    uint4 tangent2 : TANGENT2;
#else
    float4 m1 : TEXCOORD1;
#endif
    float4 m2 : TEXCOORD2;
    float4 m3 : TEXCOORD3;
    float4 m4 : TEXCOORD4;
    float4 m5 : TEXCOORD5;
    float4 m6 : TEXCOORD6;
#ifdef CLOTH
    uint binormal : BINORMAL0;
    float3 position1 : POSITION1;
#endif
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
    uint k = (i.instance + VC_InstanceData[0].matricesData.x) * 3;
    float4 p = float4(i.position, 1);
#ifdef CLOTH
    float4 w = VC_isEnableCloth
        ? float4(dot(VC_InstanceData[0].mWorld[0], p), dot(VC_InstanceData[0].mWorld[1], p), dot(VC_InstanceData[0].mWorld[2], p), 1)
        : float4(dot(VC_aObjMatrix[k], p), dot(VC_aObjMatrix[k + 1], p), dot(VC_aObjMatrix[k + 2], p), 1);
#else
    float4 w = float4(dot(VC_aObjMatrix[k], p), dot(VC_aObjMatrix[k + 1], p), dot(VC_aObjMatrix[k + 2], p), 1);
#endif
#ifdef RELATIVE
    o.clip = mul(VC_MatrixViewProj, float4(w.xyz - SC_CameraPos.xyz, 1));
#elif defined(OFFSET)
    o.clip = mul(VC_MatrixViewProj, w) + float4(0.5, 0, 0, 0);
#else
    o.clip = mul(VC_MatrixViewProj, w);
#endif
    o.world = w + SC_CameraPos * FC_ScreenSize + mul(FC_MatrixView, w) * FC_ShadowColor + VC_InstanceData[i.instance & 1].userClipPlane;
    // the stored bytes as the real program decodes them: (byte - 127) / 127
    o.normal = float4((float3(i.normal.xyz) - 127.0) * 0.007874016, 0);
#ifdef CLOTH
    o.tangent = float4((float3(i.tangent.xyz) - 127.0) * 0.007874016, (float(i.binormal) - 127.0) * 0.007874016);
#else
    o.tangent = float4((float3(i.tangent.xyz) - 127.0) * 0.007874016, (float(i.tangent.w) - 127.0) * 0.007874016);
#endif
    o.uv = float4(float2(i.uv.xy) * VC_TexScroll_0.xy / 2048.0 + VC_TexScroll_0.zw,
                  float2(i.uv.zw) * VC_TexScroll_1.xy / 2048.0 + VC_TexScroll_1.zw);
#ifdef LINING
    o.weights = float4(i.uv1.x, i.tangent2.x, i.m4.x, i.m5.y);
#else
    o.weights = float4(i.m1.x, i.m2.z, i.m4.x, i.m5.y);
#endif
    o.tint = float4(i.m3.xyz, i.colour.x);
    o.detail = float4(i.m6.z, 0, 0, 0);
    return o;
}
