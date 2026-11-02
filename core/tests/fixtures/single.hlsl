// Fixture for core's reading of a program's albedo as layers (tests/blend.rs), compiled with
//   fxc /T ps_5_0 /E main /Fo single.dxbc single.hlsl
// The degenerate layered G-buffer program: one texture, sampled at a scaled and offset UV and scaled by a colour that
// may pass one, laid over nothing; the colour saturated and multiplied by a palette row picked as blend.hlsl picks it.
// The constant buffers and inputs are blend.hlsl's.
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
    float3 colour = base_map.Sample(wrap, i.uv.xy * tile.xy + tile.zw).rgb * tint.rgb;
    uint record = (uint)round(i.normal.w);
    uint index = record * 13 + 9 + (uint)slot.w;
    float4 factor = index < 26 ? palette[index] : float4(0, 0, 0, 0);
    Output o;
    o.normal = float4(i.normal.xyz, 0);
    o.albedo = float4(saturate(colour) * factor.rgb, 1);
    return o;
}
