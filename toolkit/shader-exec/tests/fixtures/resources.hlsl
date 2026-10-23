// Fixture for the interface vocabulary's readers (primase-dxbc and primase-dxil), compiled to both bytecodes:
//   fxc /T ps_5_0 /E main /Fo resources.dxbc resources.hlsl
//   dxc -T ps_6_0 -E main -Fo resources.dxil resources.hlsl
// One of every resource kind the vocabulary has, a constant buffer of every layout rule (matrices of both majorness,
// arrays, structures, integers and booleans), and a varying of every interpolation. Every resource is used, so the
// compilers keep it. The program computes nothing meaningful.
struct Light
{
    float3 position;
    float range;
    float4 colour[2];
};
struct Element
{
    float4 a;
    float4 b;
};

cbuffer Frame : register(b2)
{
    float4x4 world;
    row_major float3x3 rotation;
    float2 offsets[3];
    Light lights[2];
    int count;
    uint4 flags;
    bool enabled;
};
cbuffer Extra : register(b5)
{
    float scale;
    double wide;
    float3x4 skew;
    row_major float2x4 pose;
};

Texture1D<float4> tex1d : register(t0);
Texture2D<float4> tex2d : register(t1);
Texture2D<uint> texuint : register(t2);
Texture2D<int2> texsint : register(t3);
Texture2DArray<float> texarray : register(t4);
Texture2DMS<float4> texms : register(t5);
Texture3D<float4> tex3d : register(t6);
TextureCube<float4> texcube : register(t7);
TextureCubeArray<float4> texcubearray : register(t8);
Buffer<float2> typed : register(t9);
StructuredBuffer<Element> structured : register(t10);
ByteAddressBuffer raw : register(t11);
RWTexture2D<float4> image : register(u2);
RWBuffer<uint> ubuffer : register(u3);
RWStructuredBuffer<Element> ustructured : register(u4);
RWByteAddressBuffer uraw : register(u5);
SamplerState linear_wrap : register(s0);
SamplerComparisonState shadow : register(s3);

struct Input
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    nointerpolation uint id : TEXCOORD1;
    centroid float3 normal : TEXCOORD2;
    noperspective float weight : TEXCOORD3;
    sample float4 tint : TEXCOORD4;
};
struct Output
{
    float4 colour : SV_Target0;
    float4 data : SV_Target1;
    float depth : SV_Depth;
};

Output main(Input i, bool front : SV_IsFrontFace)
{
    Output o;
    float4 c = tex1d.Sample(linear_wrap, i.uv.x) + tex2d.Sample(linear_wrap, i.uv);
    c += texarray.Sample(linear_wrap, float3(i.uv, 1.0)) + tex3d.Sample(linear_wrap, float3(i.uv, 0.5));
    c += texcube.Sample(linear_wrap, i.normal) + texcubearray.Sample(linear_wrap, float4(i.normal, 1.0));
    c += texms.Load(int2(i.position.xy), 0);
    c.x += texuint.Load(int3(0, 0, 0)) + texsint.Load(int3(1, 1, 0)).y;
    c.xy += typed.Load(i.id);
    c += structured[i.id].a + structured[i.id].b + asfloat(raw.Load(i.id * 4));
    c.x += texarray.SampleCmpLevelZero(shadow, float3(i.uv, 0.0), 0.5);
    image[int2(i.position.xy)] = c;
    ubuffer[i.id] = flags.x;
    ustructured[i.id].a = c;
    uraw.Store(i.id * 4, count);
    c = mul(world, c) + float4(mul(rotation, i.normal), 0) + float4(offsets[count], 0, 0) + lights[count].colour[1]
        + float4(lights[1].position, lights[0].range) * scale + (float)wide + float4(mul(skew, c), 0) + mul(pose, c).xyxy;
    o.colour = enabled ? c * i.weight + i.tint : float4(front, 0, 0, 0);
    o.data = float4(i.normal, i.id);
    o.depth = i.position.z;
    return o;
}
