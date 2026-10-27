// Fixtures for the uses a program's discard shows for its output lanes (tests/output_uses.rs), one pixel program per
// entry point, each compiled to both bytecodes:
//   fxc /T ps_5_0 /E <entry> /Fo output_uses_<entry>.dxbc output_uses.hlsl
//   dxc -T ps_6_0 -E <entry> -Fo output_uses_<entry>.dxil output_uses.hlsl
// (fxc 10.1 and dxc 1.7.2308.16, of the Windows 10 SDK 10.0.26100.0)
// Each writes three G-buffer targets, the alpha of two texels in target 2's w: `tested` discards where that alpha is
// below a reference (an alpha-tested program), `unrelated` where a value it writes nowhere is (the vertex colour's
// alpha against a fade), `opaque` never.
cbuffer Material : register(b0)
{
    float alpha_ref;
    float fade;
};
Texture2D base_map : register(t0);
Texture2D detail_map : register(t1);
SamplerState linear_wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 colour : COLOR0;
};

struct GBuffer
{
    float4 normal : SV_Target0;
    float4 gloss : SV_Target1;
    float4 albedo : SV_Target2;
};

GBuffer written(Input i, out float alpha)
{
    float4 base = base_map.Sample(linear_wrap, i.uv);
    alpha = saturate(base.a * detail_map.Sample(linear_wrap, i.uv).a);
    GBuffer o;
    o.normal = float4(i.colour.xyz * 0.5 + 0.5, 0);
    o.gloss = float4(base.r, 0, 0, 0);
    o.albedo = float4(base.rgb, alpha);
    return o;
}

// the alpha it tests is the alpha it writes to target 2's w
GBuffer tested(Input i)
{
    float alpha;
    GBuffer o = written(i, alpha);
    if (alpha < alpha_ref)
        discard;
    return o;
}

// the value it tests is written to no target
GBuffer unrelated(Input i)
{
    float alpha;
    GBuffer o = written(i, alpha);
    if (i.colour.a < fade)
        discard;
    return o;
}

// no discard
GBuffer opaque(Input i)
{
    float alpha;
    return written(i, alpha);
}
