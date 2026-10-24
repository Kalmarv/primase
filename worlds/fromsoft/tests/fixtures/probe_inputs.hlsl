// Original fixture. SDK 10.0.26100.0: fxc /T ps_5_0 /E main /Fo probe_inputs.dxbc probe_inputs.hlsl
cbuffer Probe : register(b0) { float4 FC_GlowScale; };
float4 main(float2 first : TEXCOORD0, float2 second : TEXCOORD1) : SV_Target0 {
    return float4(first, second) + FC_GlowScale;
}
