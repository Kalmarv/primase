// Original read-only probe fixture: texture answers and a non-four-lane structured element share one run.
// Windows SDK 10.0.26100.0 compilers:
// fxc /T ps_5_0 /E main /Fo answered_buffers.dxbc answered_buffers.hlsl
// dxc -T ps_6_0 -E main -Fo answered_buffers.dxil answered_buffers.hlsl
struct Entry { float4 colour; float scale; };
StructuredBuffer<Entry> entries : register(t3);
Texture2D<float4> image : register(t1);
SamplerState linear_wrap : register(s0);
cbuffer Probe : register(b0) { uint index; };
float4 main(float2 uv : TEXCOORD0) : SV_Target0 {
    return image.SampleLevel(linear_wrap, uv, 0) * entries[index].colour + entries[index].scale;
}
