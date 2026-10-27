// D3D behaviour fixture (our own HLSL): fxc /T ps_5_0 /E main. Special values come through the constant buffer.
cbuffer C : register(b0) { float4 f; uint4 u; int4 i; };
struct O {
    float4 a : SV_Target0; // min/max/saturate with NaN and signed zeros
    uint4 b : SV_Target1;  // unsigned divide and modulo (by zero too)
    uint4 c : SV_Target2;  // float -> int / uint (overflow, NaN, negative)
    uint4 d : SV_Target3;  // shift counts past 31, arithmetic shift, and
    float4 e : SV_Target4; // divide by zero, sqrt/log of negatives, a denormal product
};
O main(float4 pos : SV_Position) {
    O o;
    o.a = float4(min(f.x, f.y), max(f.x, f.y), saturate(f.x), max(f.y, f.x));
    o.b = uint4(u.x / u.y, u.x % u.y, u.z / u.w, u.z % u.w);
    o.c = uint4(asuint((int)f.x), (uint)f.x, asuint((int)f.z), (uint)f.z);
    o.d = uint4(u.x << u.y, u.x >> u.y, asuint(i.x >> i.y), (u.x & u.y));
    o.e = float4(f.x / f.w, sqrt(f.z), log2(f.w), f.y * f.y);
    return o;
}
