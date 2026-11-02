// Fixture for core's reading of a program's albedo as layers (tests/blend.rs), compiled with
//   fxc /T ps_5_0 /E main /Fo blend.dxbc blend.hlsl
// The shape of a layered G-buffer program: a bottom layer moved part of the way to a constant colour, a second layer
// scaled by a colour past one and sampled at a scaled and offset UV, laid over it by a weight made of a vertex lane and
// a mask sampled on the second UV set (its coordinate scaled in two steps, then offset); the blend saturated and
// multiplied by a palette row picked by a record number the vertex program hands on and a slot the material states.
cbuffer Material : register(b1)
{
    float4 tile;
    float4 adjust;
    float4 tint;
    float4 gain;
    float4 slot;
};
cbuffer Instances : register(b4)
{
    float4 palette[26];
};
Texture2D base_map : register(t0);
Texture2D layer_map : register(t1);
Texture2D mask_map : register(t2);
SamplerState wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float4 normal : NORMAL0;
    float4 uv : TEXCOORD0;
    float4 weights : TEXCOORD1;
};

struct Output
{
    float4 normal : SV_Target0;
    float4 albedo : SV_Target1;
};

Output main(Input i)
{
    float3 bottom = lerp(base_map.Sample(wrap, i.uv.xy).rgb, adjust.rgb, adjust.a);
    float3 layer = layer_map.Sample(wrap, i.uv.xy * tile.xy + tile.zw).rgb * tint.rgb;
    float mask = mask_map.Sample(wrap, i.uv.zw * gain.yz * 2 - gain.w).r;
    float weight = saturate(1 - 1.4 * gain.x * (mask + 1 - 2 * i.weights.x));
    float3 colour = lerp(bottom, layer, weight);
    uint record = (uint)round(i.normal.w);
    uint index = record * 13 + 9 + (uint)slot.w;
    float4 factor = index < 26 ? palette[index] : float4(0, 0, 0, 0);
    Output o;
    o.normal = float4(i.normal.xyz, 0);
    o.albedo = float4(saturate(colour) * factor.rgb, 1);
    return o;
}
