// Original fixture: a second vertex colour whose w the pixel program reads only when a material word is set, as the
// ColorAdjust programs read COLOR1.w only under their snp_0_bool_0; x reaches an output either way.
// fxc /T vs_5_0 /E vertex /Fo vertex_colour1_vs.dxbc vertex_colour1.hlsl
// fxc /T ps_5_0 /E pixel /Fo vertex_colour1_ps.dxbc vertex_colour1.hlsl
cbuffer cbMtdParam : register(b3) {
    bool weighted;
};
struct Varyings {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float2 colour1 : TEXCOORD1;
};
Varyings vertex(float3 position : POSITION0, int4 uv : TEXCOORD0, float4 colour1 : COLOR1) {
    Varyings o;
    o.position = float4(position, 1);
    o.uv = float2(uv.xy) / 2048;
    o.colour1 = colour1.xw;
    return o;
}
float4 pixel(Varyings i) : SV_Target0 {
    float mask = i.uv.x;
    if (weighted) {
        mask *= i.colour1.y;
    }
    return float4(mask, i.uv.y, 0, i.colour1.x);
}
