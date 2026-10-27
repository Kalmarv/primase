// A tessellated triangle patch: a vertex program that writes no position (its outputs feed the hull program), the hull
// program with its patch-constant function, and the domain program that places each vertex of the patch. The same
// source as the DXIL fixture; compiled with
//   fxc /T vs_5_0 /E vs_main (tessellation.vs.dxbc),  /T hs_5_0 /E hs_main (tessellation.hs.dxbc),
//   fxc /T ds_5_0 /E ds_main (tessellation.ds.dxbc)  and the same with /Od (tessellation.vs.od.dxbc, ...).
cbuffer View : register(b0) {
    float4x4 viewProjection;
    float scale;
};

struct VsIn {
    float3 position : POSITION;
    float4 color : COLOR;
};

struct CtrlPoint {
    float3 position : CTRL_POSITION;
    float4 color : CTRL_COLOR;
};

struct PatchInfo {
    float edges[3] : SV_TessFactor;
    float inside : SV_InsideTessFactor;
    float3 centre : CENTRE;
};

CtrlPoint vs_main(VsIn i) {
    CtrlPoint o;
    o.position = i.position * scale;
    o.color = i.color;
    return o;
}

PatchInfo patch_main(InputPatch<CtrlPoint, 3> p, uint id : SV_PrimitiveID) {
    PatchInfo info;
    info.edges[0] = 2.0;
    info.edges[1] = 3.0;
    info.edges[2] = 4.0 + id;
    info.inside = 5.0;
    info.centre = (p[0].position + p[1].position + p[2].position) / 3.0;
    return info;
}

[domain("tri")]
[partitioning("integer")]
[outputtopology("triangle_cw")]
[outputcontrolpoints(3)]
[patchconstantfunc("patch_main")]
[maxtessfactor(16.0)]
CtrlPoint hs_main(InputPatch<CtrlPoint, 3> p, uint i : SV_OutputControlPointID) {
    CtrlPoint o = p[i];
    o.position += float3(0.0, 0.0, i);
    return o;
}

struct DsOut {
    float4 position : SV_Position;
    float4 color : COLOR;
};

[domain("tri")]
DsOut ds_main(PatchInfo info, float3 location : SV_DomainLocation, const OutputPatch<CtrlPoint, 3> p) {
    DsOut o;
    float3 position = p[0].position * location.x + p[1].position * location.y + p[2].position * location.z;
    position += info.centre * 0.5 + info.edges[0] * 0.25 + info.inside * 0.125;
    o.position = mul(viewProjection, float4(position, 1.0));
    o.color = p[0].color * location.x + p[1].color * location.y + p[2].color * location.z;
    return o;
}
