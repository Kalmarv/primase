// A mesh program (Shader Model 6.5): one meshlet of up to 4 vertices and 2 triangles per thread group of 32. Compiled with
//   dxc -T ms_6_5 -E main (mesh.dxil, also -Od and -T ms_6_6).
// The group writes a quad as two triangles: vertex i (thread i < 4) at the corner (i & 1, i >> 1) scaled by `scale`, with
// the colour (i & 1, i >> 1, 0); thread 0 emits triangle 0 as (0, 1, 2) and thread 1 triangle 1 as (2, 1, 3).
cbuffer Scale : register(b0) {
    float scale;
};

struct Vertex {
    float4 position : SV_Position;
    float3 color : COLOR0;
};

[numthreads(32, 1, 1)]
[outputtopology("triangle")]
void main(uint index : SV_GroupIndex, out vertices Vertex vertices[4], out indices uint3 triangles[2]) {
    SetMeshOutputCounts(4, 2);
    if (index < 4) {
        float2 corner = float2(index & 1, index >> 1);
        vertices[index].position = float4(corner * scale, 0, 1);
        vertices[index].color = float3(corner, 0);
    }
    if (index == 0) {
        triangles[0] = uint3(0, 1, 2);
    }
    if (index == 1) {
        triangles[1] = uint3(2, 1, 3);
    }
}
