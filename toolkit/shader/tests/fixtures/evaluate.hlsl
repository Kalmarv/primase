// Fixture for primase-shader's evaluate, compiled to both bytecodes:
//   fxc /T ps_5_0 /E main /Fo evaluate.dxbc evaluate.hlsl
//   dxc -T ps_6_0 -E main -Fo evaluate.dxil evaluate.hlsl
// A constant buffer whose scalar packs after a float2, a texture at t3, a float2 input the compiler packs into the
// .zw of the UV's register, and outputs at registers 0 and 2 only.
cbuffer Material : register(b1)
{
    float4 tint;
    float2 tiling;
    float strength;
};
Texture2D albedo : register(t3);
SamplerState wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float2 detail : TEXCOORD1;
    float weight : TEXCOORD2;
};
struct Output
{
    float4 colour : SV_Target0;
    float4 data : SV_Target2;
};

Output main(Input i)
{
    Output o;
    o.colour = albedo.Sample(wrap, i.uv * tiling) * tint;
    o.data = float4(i.detail * strength, i.weight, 1.0);
    return o;
}