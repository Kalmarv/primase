// Stores into an array of images at an element the program computes (compute, one group of 8 by 8). Compiled with dxc
// -T cs_6_6 -E main (image_arrays.dxil; the skate programs use shader model 6.6 and a computed element index), also
// -T cs_6_0 and -Od.
// Thread (x, y) with group index g = x + 8 y writes float4(x, y, g, 1) to texel (x, y) of imgs[g & 3].
RWTexture2D<float4> imgs[4] : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID, uint g : SV_GroupIndex) {
    imgs[g & 3][id.xy] = float4(id.x, id.y, g, 1);
}
