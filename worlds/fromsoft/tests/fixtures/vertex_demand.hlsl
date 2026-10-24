// Original fixture: a vertex input only needed by some consumers, or by rasterization.
// fxc /T vs_5_0 /E vertex /Fo vertex_demand_vs.dxbc vertex_demand.hlsl
// fxc /T vs_5_0 /E vertex /D CLIP /Fo vertex_demand_clip_vs.dxbc vertex_demand.hlsl
// fxc /T ps_5_0 /E pixel_dead /Fo vertex_demand_dead_ps.dxbc vertex_demand.hlsl
// fxc /T ps_5_0 /E pixel_live /Fo vertex_demand_live_ps.dxbc vertex_demand.hlsl
// fxc /T ps_5_0 /E pixel_discard /Fo vertex_demand_discard_ps.dxbc vertex_demand.hlsl
struct Varyings {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float2 extra : TEXCOORD4;
#ifdef CLIP
    float clip : SV_ClipDistance0;
#endif
};
Varyings vertex(float3 position : POSITION0, int4 uv : TEXCOORD0, int2 missing : TEXCOORD1) {
    Varyings o;
    o.position = float4(position, 1);
    o.uv = float2(uv.xy) / 2048;
    o.extra = float2(missing) / 2048;
#ifdef CLIP
    o.clip = missing.x;
#endif
    return o;
}
float4 pixel_dead(Varyings i) : SV_Target0 { return float4(i.uv, 0, 1); }
float4 pixel_live(Varyings i) : SV_Target0 { return float4(i.extra, 0, 1); }
float4 pixel_discard(Varyings i) : SV_Target0 { clip(i.extra.x); return float4(i.uv, 0, 1); }
