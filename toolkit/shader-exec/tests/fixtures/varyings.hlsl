// Fixture for the interface vocabulary's varyings (primase-shader's tests/interface.rs), compiled to both bytecodes:
//   fxc /T ps_5_0 /E main /Fo varyings.dxbc varyings.hlsl
//   dxc -T ps_6_0 -E main -Fo varyings.dxil varyings.hlsl
// A varying of every interpolation, the position and front-face system values, two colour targets. Every input is
// used: a program declares (and so interpolates) only the inputs it reads.
Texture2D albedo : register(t0);
SamplerState wrap : register(s0);

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
};

Output main(Input i, bool front : SV_IsFrontFace)
{
    Output o;
    float4 c = albedo.Sample(wrap, i.uv);
    o.colour = front ? c * i.weight + i.tint : float4(0, 0, 0, 0);
    o.data = float4(i.normal, i.id) + i.position.x;
    return o;
}
