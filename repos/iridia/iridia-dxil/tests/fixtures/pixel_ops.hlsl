// Pixel-stage operations the signature does not carry or the loads add to: the sample index (SV_SampleIndex), the
// standard sample positions of a four-sample texture, a texel load at a constant offset, and the half-float
// conversions. Compiled with dxc -T ps_6_0 (also -Od, and -T ps_6_6).
Texture2D<float4> img : register(t0);
Texture2DMS<float4, 4> ms : register(t1);

cbuffer Values : register(b0) {
    float h;     // converted to a half
    uint packed; // a half in its low 16 bits
};

struct Out {
    float4 loaded : SV_Target0;  // img.Load(int3(5, 6, 0), int2(-2, 3)): the texel (3, 9) of level 0
    float4 numbers : SV_Target1; // the sample's position, f16tof32(packed), f32tof16(h)
};

Out main(float4 pos : SV_Position, uint sample : SV_SampleIndex) {
    Out o;
    o.loaded = img.Load(int3(5, 6, 0), int2(-2, 3));
    float2 where = ms.GetSamplePosition(sample);
    o.numbers = float4(where, f16tof32(packed), f32tof16(h));
    return o;
}
