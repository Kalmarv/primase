// The draw stage's fixtures (tests/exec_warp_draw.rs): one vertex program and pixel programs that show what the
// rasterizer hands a pixel. Compiled with dxc 1.7.2308 (Windows SDK 10.0.26100) for Shader Model 6.0:
//   dxc -T vs_6_0 -E vs_main -Fo draw_vs.dxil draw.hlsl
//   dxc -T ps_6_0 -E ps_<name> -Fo draw_<name>.dxil draw.hlsl
struct VIn {
    float4 pos : POSITION0;
    float4 col : COLOR0;
    float2 uv : TEXCOORD0;
    float4 id : TEXCOORD1;
};
struct VOut {
    float4 pos : SV_Position;
    float4 col : COLOR0;
    noperspective float2 uv : TEXCOORD0;
    nointerpolation float4 id : TEXCOORD1;
};
cbuffer V : register(b0) {
    float4 scale;
    float4 offset;
};
VOut vs_main(VIn v) {
    VOut o;
    o.pos = v.pos * scale + offset;
    o.col = v.col;
    o.uv = v.uv;
    o.id = v.id;
    return o;
}

struct Two {
    float4 a : SV_Target0;
    float4 b : SV_Target1;
};

// the three interpolation modes
Two ps_colour(VOut i) {
    Two o;
    o.a = i.col;
    o.b = float4(i.uv, i.id.xy);
    return o;
}

// the pixel's position and the triangle's facing
Two ps_position(VOut i, bool front : SV_IsFrontFace) {
    Two o;
    o.a = i.pos;
    o.b = float4(front ? 1.0 : 0.0, i.id.x, 0.0, 1.0);
    return o;
}

// a discard by a perspective-interpolated value
float4 ps_discard(VOut i) : SV_Target0 {
    if (i.col.x > 0.5) {
        discard;
    }
    return i.col;
}

// derivatives across the quad
Two ps_derivatives(VOut i) {
    Two o;
    o.a = float4(ddx_fine(i.col.x), ddy_fine(i.col.x), ddx_coarse(i.uv.x), ddy_coarse(i.uv.y));
    o.b = float4(ddx_fine(i.col.y), ddy_fine(i.col.y), ddx_fine(i.pos.z), ddy_fine(i.pos.w));
    return o;
}

// a texture sampled at the interpolated coordinate (the oracle's ramp texture, point filtering, clamp)
Texture2D<float4> ramp : register(t0);
SamplerState point_clamp : register(s0);
float4 ps_texture(VOut i) : SV_Target0 {
    return ramp.SampleLevel(point_clamp, i.uv, 0);
}
