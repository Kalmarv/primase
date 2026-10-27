// Fixture for the DXIL lifter: buffer views and unordered-access textures. Compile:
// dxc -T ps_6_0 -E main -Fo resources.dxil resources.hlsl
StructuredBuffer<float4> sb : register(t0);
ByteAddressBuffer raw : register(t1);
Buffer<float4> typed : register(t2);
RWTexture2D<float4> image : register(u0);
cbuffer Values : register(b0)
{
    int4 n;
};
struct Output
{
    float4 x : SV_Target0;
    uint4 k : SV_Target1;
};
Output main(float4 position : SV_Position)
{
    Output o;
    o.x = sb[n.x] + typed[n.y] + image[uint2(n.zw)];
    o.k = raw.Load4(n.x * 16);
    image[uint2(position.xy)] = o.x;
    return o;
}
