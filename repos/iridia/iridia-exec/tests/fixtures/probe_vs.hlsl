// Fixture for the vertex probe (tests/probe.rs), compiled to both bytecodes:
//   fxc /T vs_5_0 /E main /Fo probe_vs.dxbc probe_vs.hlsl   (fxc 10.0.26100.0)
//   dxc -T vs_6_0 -E main -Fo probe_vs.dxil probe_vs.hlsl   (dxc 1.7.2308.16)
// A vertex program that reads a texture of its own: it samples it, loads a texel and asks its size, and moves the
// vertex by each, scaled by `lift`. With `lift` zero the clip position is the constants' alone; otherwise the texture's
// answers show in it. The program computes nothing meaningful beyond that.
Texture2D<float4> height : register(t0);
SamplerState point_clamp : register(s0);

cbuffer Probe : register(b0)
{
    float4 scale;
    float4 offset;
    float lift;
};

struct Input
{
    float4 position : POSITION0;
    float2 uv : TEXCOORD0;
};

float4 main(Input i) : SV_Position
{
    float4 p = i.position * scale + offset;
    uint width, rows, levels;
    height.GetDimensions(0, width, rows, levels);
    p.x += float(width + rows + levels) * lift;
    p.y += height.SampleLevel(point_clamp, i.uv, 0).x * lift;
    p.z += height.Load(int3(0, 0, 0)).y * lift;
    return p;
}
