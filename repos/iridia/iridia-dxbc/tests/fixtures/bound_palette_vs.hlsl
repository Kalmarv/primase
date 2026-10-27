// Fixture for the bound size of a constant buffer (toolkit/dxil, toolkit/dxbc tests/bound_size.rs): a skinning palette the
// program declares as two matrices (six rows of four) and indexes by a joint, as the engine binds a larger buffer than the
// shader declares (D3D12 bounds a constant buffer read by the view bound, not by the declaration). It computes nothing
// meaningful beyond one row of the joint's matrix.
//   dxc -T vs_6_0 -E main -Fo bound_palette_vs.dxil bound_palette_vs.hlsl
//   fxc /T vs_5_0 /E main /Fo bound_palette_vs.dxbc bound_palette_vs.hlsl
cbuffer cbPalette : register(b0)
{
    float4 Palette[6];
};

struct Input
{
    float3 position : POSITION0;
    uint joint : BLENDINDICES0;
};

float4 main(Input i) : SV_Position
{
    uint b = i.joint * 3;
    float4 p = float4(i.position, 1);
    return float4(dot(Palette[b], p), dot(Palette[b + 1], p), dot(Palette[b + 2], p), 1);
}
