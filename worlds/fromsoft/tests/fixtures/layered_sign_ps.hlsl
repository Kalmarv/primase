// Live consumers for the layered VS's tangent sign and second colour. The original layered.dxbc pixel fixture reads
// only SV_Position, TEXCOORD0 and TEXCOORD5, so neither of these values is required when that program is bound.
// fxc /T ps_5_0 /E main /Fo layered_sign_ps.dxbc layered_sign_ps.hlsl
float4 main(float4 tangent : TANGENT0, float4 tint : TEXCOORD7) : SV_Target0
{
    return float4(tangent.w, tint.w, 0, 1);
}
