// A hull program whose phases declare different numbers of temporaries (dcl_temps 2 in the control-point phase, 1 in
// the last fork phase): the lifter takes the most any phase declares. Compiled with
//   fxc /T hs_5_0 /E hs hull_phases.hlsl /Fo hull_phases.hs.dxbc
struct CP {
    float4 p : POSITION;
};
struct PC {
    float e[3] : SV_TessFactor;
    float i : SV_InsideTessFactor;
    float4 extra : EXTRA;
};

PC pc(InputPatch<CP, 3> patch, uint id : SV_PrimitiveID) {
    PC o;
    o.e[0] = 2.0;
    o.e[1] = 3.0;
    o.e[2] = 4.0;
    o.i = 5.0;
    o.extra = patch[0].p + patch[2].p + float4(id, 0, 0, 0);
    return o;
}

[domain("tri")]
[partitioning("integer")]
[outputtopology("triangle_cw")]
[outputcontrolpoints(3)]
[patchconstantfunc("pc")]
[maxtessfactor(64.0)]
CP hs(InputPatch<CP, 3> patch, uint i : SV_OutputControlPointID) {
    CP o = patch[i];
    o.p += float4(i, 0.5 * i, 0.25, 0);
    return o;
}
