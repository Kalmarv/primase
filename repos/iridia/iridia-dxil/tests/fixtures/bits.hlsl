// Fixture for the DXIL lifter: bit operations, divides, conversions. Compile:
// dxc -T ps_6_0 -E main -Fo bits.dxil bits.hlsl
cbuffer Values : register(b0)
{
    float4 f;
    uint4 u;
    int4 i;
};
struct Output
{
    uint4 a : SV_Target0;
    uint4 b : SV_Target1;
    float4 c : SV_Target2;
    uint4 d : SV_Target3;
};
Output main(float4 position : SV_Position)
{
    Output o;
    o.a = uint4(countbits(u.x), firstbitlow(u.y), firstbithigh(u.z), reversebits(u.w));
    o.b = uint4(u.x / u.y, u.x % u.y, (uint)(i.x / i.y), (uint)(i.x % i.y));
    o.c = float4(f.x / f.y, min(f.x, f.y), max(f.x, f.y), saturate(f.z));
    o.d = uint4(u.x * u.y, u.z * u.w, (uint)((int)f.x), (uint)f.y);
    o.d.z += (uint)(i.z >> (i.w & 31)) + (u.z << (u.w & 31)) + mad(u.x, u.y, u.z);
    return o;
}
