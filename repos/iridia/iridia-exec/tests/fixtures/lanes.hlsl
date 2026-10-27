// Fixture for the executor's invocations in lockstep (tests/lanes.rs), compiled to both bytecodes:
//   fxc /T ps_5_0 /E main /Fo lanes.dxbc lanes.hlsl
//   dxc -T ps_6_0 -E main -Fo lanes.dxil lanes.hlsl
// (fxc 10.0.26100.4654 and dxc 1.7.2308.16, of the Windows 10 SDK 10.0.26100.0)
// The pixels of a row take different branches: a sample on one side of `threshold` only, a loop each pixel turns its
// own number of times (and leaves early where its own test says), a discard some take. Past `budget` a pixel's loop
// does not end before the executor's step limit.
cbuffer Material : register(b0)
{
    float threshold;
    float budget;
};
Texture2D albedo : register(t0);
SamplerState wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

float4 main(Input i) : SV_Target0
{
    float4 c = float4(i.uv, ddx(i.uv.x), 1.0);
    [branch] if (i.uv.x > threshold)
        c *= albedo.SampleLevel(wrap, i.uv, 0);
    else
        c.xy = c.yx * 0.5;
    uint turns = i.uv.x > budget ? 100000000u : (uint)(i.uv.x * 8.0);
    float z = 1.0;
    [loop] for (uint k = 0; k < turns; k++)
    {
        z = z * 0.75 + 0.125;
        [branch] if (z < i.uv.y)
            break;
    }
    c.z += z;
    if (frac(i.uv.x * 3.0) > 0.8)
        discard;
    return c;
}
