// A geometry program (Shader Model 6): a triangle in, two triangle strips out, two instances. Compiled with
// dxc -T gs_6_0 -E main (geometry.dxil, also -Od and -T gs_6_6).
// Per invocation: the triangle's corners with x scaled by `scale` (the first strip), a restart, then the corners moved by
// the primitive number in x and the instance in y, uv.x increased by the instance (the second strip).
cbuffer Scale : register(b0) {
    float scale;
};

struct Vertex {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

[instance(2)]
[maxvertexcount(6)]
void main(triangle Vertex input[3], uint primitive : SV_PrimitiveID, uint instance : SV_GSInstanceID,
          inout TriangleStream<Vertex> stream) {
    for (uint i = 0; i < 3; i++) {
        Vertex v = input[i];
        v.position.x *= scale;
        stream.Append(v);
    }
    stream.RestartStrip();
    for (uint j = 0; j < 3; j++) {
        Vertex v = input[j];
        v.position.x += primitive;
        v.position.y += instance;
        v.uv.x += instance;
        stream.Append(v);
    }
}
