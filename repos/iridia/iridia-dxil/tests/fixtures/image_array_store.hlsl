// A store into an element of an array of storage images (compute), the element picked by a computed index. Compiled
// with dxc -T cs_6_6 -E main (image_array_store.dxil, also -Od). Thread (x, y) of 8 by 8 writes (x, y, g, 1) to image
// g & 3, g being its flattened index.
RWTexture2D<float4> imgs[4] : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID, uint g : SV_GroupIndex) {
    imgs[g & 3][id.xy] = float4(id.x, id.y, g, 1);
}
