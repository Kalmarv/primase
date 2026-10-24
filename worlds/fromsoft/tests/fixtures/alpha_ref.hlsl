// Fixture for resolve-fromsoft-shaders' alpha-test reference (worlds/fromsoft/tests/resolve.rs): Sekiro's cbMtdParam (b3)
// with FC_AlphaRef at 80 where the real programs have it, and a discard against its x. `tested` is an alpha-tested
// G-buffer program as the `_Gbuf_[A]` programs are: it discards where the alpha it writes to target 2's w is below
// FC_AlphaRef.x. `unrelated` discards by FC_AlphaRef.x against a value it writes to no target.
//   fxc /T ps_5_0 /E <entry> /Fo alpha_ref_<entry>.dxbc alpha_ref.hlsl
//   dxc -T ps_6_0 -E <entry> -Fo alpha_ref_<entry>.dxil alpha_ref.hlsl
// (fxc 10.1 and dxc 1.7.2308.16, of the Windows 10 SDK 10.0.26100.0)
cbuffer cbMtdParam : register(b3)
{
    float4 FC_DifMapMulCol;
    float4 unused_a[4];
    float4 FC_AlphaRef;
    float4 unused_b[3];
    float4 FC_GlowScale;
};

struct Input
{
    float4 position : SV_Position;
    float4 uv : TEXCOORD0;
};
struct Output
{
    float4 normal : SV_Target0;
    float4 gloss : SV_Target1;
    float4 albedo : SV_Target2;
    float4 reflectance : SV_Target3;
    float4 emission : SV_Target4;
};

Output written(Input i, float alpha)
{
    Output o;
    o.normal = float4(0.5, 0.5, 1, 1);
    o.gloss = FC_GlowScale;
    o.albedo = float4(FC_DifMapMulCol.rgb * i.uv.xyz + unused_a[0].rgb + unused_b[0].rgb, alpha);
    o.reflectance = float4(0.04, 0.04, 0.04, 1);
    o.emission = float4(0, 0, 0, 1);
    return o;
}

// the alpha it tests against the reference is the alpha it writes to target 2's w
Output tested(Input i)
{
    float alpha = saturate(FC_DifMapMulCol.w * i.uv.z);
    if (alpha < FC_AlphaRef.x)
        discard;
    return written(i, alpha);
}

// the value it tests against the reference is written to no target
Output unrelated(Input i)
{
    float alpha = saturate(FC_DifMapMulCol.w * i.uv.z);
    if (i.uv.w < FC_AlphaRef.x)
        discard;
    return written(i, alpha);
}
