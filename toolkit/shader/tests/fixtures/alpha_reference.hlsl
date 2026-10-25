// Original structural alpha-reference fixtures. SDK 10.0.26100.0 fxc /T ps_5_0 and dxc -T ps_6_0,
// /E (or -E) each entry below; outputs alpha_reference_<entry>.dxbc/.dxil.
// The test supplies t2 as the dither role, except same_source which deliberately supplies t0.
cbuffer Material : register(b0) { float reference; float scale; float gain; float branch; };
Texture2D base_map : register(t0);
Texture2D detail_map : register(t1);
Texture2D dither_map : register(t2);
SamplerState sampler0 : register(s0);
struct Input { float4 position : SV_Position; float2 uv : TEXCOORD0; float4 colour : COLOR0; };

float4 direct(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    if (b.a < reference) discard;
    return b;
}
float4 transformed(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    clip(b.a - reference);
    if (branch > 0) return float4(b.rgb, saturate(b.a) * gain);
    return float4(b.rgb, saturate(b.a));
}
float4 dithered(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    precise float p = (dither_map.Sample(sampler0, i.uv).a - 0.5) * scale;
    if (b.a + p < reference) discard;
    return float4(b.rgb, saturate(b.a) * gain);
}
float4 product(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    float s = b.a * detail_map.Sample(sampler0, i.uv).a;
    precise float p = (dither_map.Sample(sampler0, i.uv).a - 0.5) * scale;
    if (reference > s + p) discard;
    return float4(b.rgb, saturate(s) * gain);
}
float4 same_source(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    precise float p = (base_map.Sample(sampler0, i.uv + 0.125).a - 0.5) * scale;
    if (b.a + p < reference) discard;
    return float4(b.rgb, saturate(b.a) * gain);
}
float4 shared_scale(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    precise float s = b.a * scale;
    precise float p = (dither_map.Sample(sampler0, i.uv).a - 0.5) * scale;
    if (s + p < reference) discard;
    return float4(b.rgb, saturate(s) * gain);
}
float4 control_only(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    if (b.a < reference) discard;
    return float4(b.rgb, branch > b.a ? 0.25 : 0.75);
}
float4 coordinate_only(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    if (b.a < reference) discard;
    return float4(b.rgb, detail_map.Sample(sampler0, float2(b.a, i.uv.y)).a);
}
// The tested value reaches no target: the alpha written is a constant (an Elden Ring G-buffer program writes none).
// discard where base.a * detail.a < reference.
float4 unwritten(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    if (b.a * detail_map.Sample(sampler0, i.uv).a < reference) discard;
    return float4(b.rgb, 1.0);
}
// discard where base.a + (dither.a - 0.5) * scale < reference, the alpha written a constant.
float4 unwritten_dither(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    precise float p = (dither_map.Sample(sampler0, i.uv).a - 0.5) * scale;
    if (b.a + p < reference) discard;
    return float4(b.rgb, 1.0);
}
float4 unrelated(Input i) : SV_Target0 {
    float4 b = base_map.Sample(sampler0, i.uv);
    if (i.colour.a < reference) discard;
    return b;
}
