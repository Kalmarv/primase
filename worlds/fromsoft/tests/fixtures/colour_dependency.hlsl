// Original fixtures for finished Colour portability, compiled with Windows SDK fxc 10.0.26100.0.
// fxc /T ps_5_0 /E material /Fo colour_material.dxbc colour_dependency.hlsl
// fxc /T ps_5_0 /E scene /Fo colour_scene.dxbc colour_dependency.hlsl
cbuffer Material : register(b0) { float4 colour; };
cbuffer Frame : register(b1) { float4 light; };
float4 material() : SV_Target0 { return colour; }
float4 scene() : SV_Target0 { return float4(colour.rgb * light.rgb, colour.a); }
