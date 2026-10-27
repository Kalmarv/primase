// Fixture for primase-dxil: a sample, a constant buffer, a front-face branch (phi) and the intrinsics a G-buffer
// shader uses. Compile: dxc -T ps_6_0 -E main -Fo blend.dxil blend.hlsl
cbuffer Constants : register(b3)
{
    float4 scale;
    float4 bias;
    uint4 flags;
};
Texture2D layer : register(t2);
SamplerState linear_wrap : register(s1);

struct Input
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float weight : TEXCOORD1;
    nointerpolation uint id : TEXCOORD2;
};
struct Output
{
    float4 colour : SV_Target0;
    float4 extra : SV_Target1;
};

Output main(Input i, bool front : SV_IsFrontFace)
{
    Output o;
    float4 t = layer.Sample(linear_wrap, i.uv * scale.xy + bias.xy);
    float w = saturate(i.weight * scale.z - bias.z);
    o.colour = lerp(t, float4(1.0, 0.5, 0.25, 1.0), w);
    float d = dot(t.xyz, float3(1.0, 2.0, 3.0));
    if (front)
    {
        d = -d;
    }
    o.extra = float4(d, sqrt(abs(d)), max(i.uv.x, i.uv.y), (float)(i.id * 3u + flags.x));
    return o;
}
