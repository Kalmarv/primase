// Descriptors from the heaps (compute, Shader Model 6.6): a texture sampled, a structured buffer read and a structured
// buffer written, all by indices from a constant buffer. Compiled with dxc -T cs_6_6 -E main (heap.dxil, also -Od).
cbuffer Indices : register(b0) {
    uint texture_index;
    uint sampler_index;
    uint source_index;
    uint target_index;
};

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    Texture2D<float4> image = ResourceDescriptorHeap[texture_index];
    SamplerState smp = SamplerDescriptorHeap[sampler_index];
    StructuredBuffer<float> source = ResourceDescriptorHeap[source_index];
    RWStructuredBuffer<float4> target = ResourceDescriptorHeap[target_index];
    float2 uv = (id.xy + 0.5) / 8.0;
    target[id.x + id.y * 8] = image.SampleLevel(smp, uv, 0) * source[id.x];
}
