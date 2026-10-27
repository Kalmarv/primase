// Arrays of resources read by a computed index: four textures (t0 to t3, space 1), an unbounded sampler array
// (s0, space 2) and a single texture (t8). Compiled with dxc -T ps_6_0 (also -Od, and -T ps_6_6).
Texture2D<float4> tex[4] : register(t0, space1);
SamplerState smp[] : register(s0, space2);
Texture2D<float4> single : register(t8);

cbuffer Select : register(b0) {
    uint2 which; // x: the texture, y: the sampler
    float2 uv;
};

struct Out {
    float4 sampled : SV_Target0; // tex[which.x] with smp[which.y]
    float4 fixed : SV_Target1;   // tex[2] with smp[1], and a texel of tex[which.x + 1]
    float4 sizes : SV_Target2;   // tex[which.x]'s width and height, and single's
};

Out main(float4 pos : SV_Position) {
    Out o;
    o.sampled = tex[which.x].SampleLevel(smp[which.y], uv, 0);
    o.fixed = tex[2].SampleLevel(smp[1], uv, 0) + tex[which.x + 1].Load(int3(1, 2, 0));
    uint w, h, sw, sh;
    tex[which.x].GetDimensions(w, h);
    single.GetDimensions(sw, sh);
    o.sizes = float4(w, h, sw, sh);
    return o;
}
