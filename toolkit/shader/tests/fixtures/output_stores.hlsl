// Fixture for what a pixel program stores into its colour targets (tests/output_stores.rs), compiled with
//   dxc -T ps_6_0 -E main -Fo output_stores.dxil output_stores.hlsl
// (dxc 1.7.2308.16, of the Windows 10 SDK 10.0.26100.0)
// Target 0's xyz the normal biased (`n * 0.5 + 0.5`), target 1's x a square root and y the constant 0, target 2's xyz
// the normal floored to a byte (`floor(n * 128 + 128) / 256`); target 1's x is made of the material's row 1.
cbuffer Material : register(b0)
{
    float4 tint;
    float4 gloss;
};

struct Input
{
    float4 position : SV_Position;
    float3 normal : TEXCOORD0;
};

struct Output
{
    float4 t0 : SV_Target0;
    float4 t1 : SV_Target1;
    float4 t2 : SV_Target2;
};

Output main(Input i)
{
    Output o;
    float3 n = normalize(i.normal);
    o.t0 = float4(n * 0.5 + 0.5, tint.w);
    o.t1 = float4(sqrt(saturate(gloss.x)), 0.0, tint.x, 1.0);
    o.t2 = float4(floor(n * 128.0 + 128.0) * (1.0 / 256.0), 1.0);
    return o;
}
