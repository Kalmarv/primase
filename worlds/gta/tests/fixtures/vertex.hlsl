// Fixture for the DXIL lifter: a vertex shader with a position, a plain varying, an instance id and a clip distance.
// Compile: dxc -T vs_6_0 -E main -Fo vertex.dxil vertex.hlsl
cbuffer Values : register(b0)
{
    float4x4 world_view_projection;
    float4 tint;
};
struct Input
{
    float3 position : POSITION;
    float2 uv : TEXCOORD0;
    uint id : SV_VertexID;
    uint instance : SV_InstanceID;
};
struct Output
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 colour : COLOR0;
    float clip : SV_ClipDistance0;
};
Output main(Input i)
{
    Output o;
    o.position = mul(float4(i.position, 1.0), world_view_projection);
    o.uv = i.uv * (float)(i.id & 3u);
    o.colour = tint * (float)(i.instance + 1u);
    o.clip = i.position.y;
    return o;
}
