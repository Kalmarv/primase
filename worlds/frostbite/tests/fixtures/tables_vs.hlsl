// Fixture for primase-frostbite (our own HLSL), four programs of one source:
//   dxc -T vs_6_0 -E main -Od -Fo tables_vs.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D RECORD2 -Fo tables_vs.record2.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D NO_HEADER -Fo tables_vs.no_header.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D THREE_BUFFERS -Fo tables_vs.three_buffers.dxil tables_vs.hlsl
// A vertex program that finds its world position the way the census's instanced vertex programs do: the record of the
// instance (the buffer at t0, one word, or two with the matrix in the last) names the element of a header in the matrix
// table (t67), whose first word's bits move the three rows after it, and the clip position is the camera-relative form of
// camera_relative_vs.hlsl. The variants break one thing each: records of two words, rows read where the header is, a
// third buffer read.
cbuffer Pass : register(b4)
{
    float4 before[245];
    float4 clip_rows[4];
    float4 between[16];
    float3 camera;
};
cbuffer Set : register(b0)
{
    uint base;
};
#ifdef RECORD2
StructuredBuffer<uint2> instances : register(t0);
#else
StructuredBuffer<uint> instances : register(t0);
#endif
StructuredBuffer<float4> matrices : register(t67);
#ifdef THREE_BUFFERS
StructuredBuffer<float4> extra : register(t68);
#endif
struct VO { float4 clip : SV_Position; };
VO main(float3 position : POSITION0, uint id : SV_InstanceID)
{
    VO o;
#ifdef RECORD2
    uint rec = instances[base + id].y;
#else
    uint rec = instances[base + id];
#endif
    uint header = asuint(matrices[rec].x);
    uint later = ((header & 1) != 0 ? ((header >> 1) & 0xFFFF) : 0) + ((header >> 18) & 1);
#ifdef NO_HEADER
    uint first = rec;
#else
    uint first = rec + 1 + later;
#endif
    float4 p = float4(position, 1);
    float3 w = float3(dot(matrices[first], p), dot(matrices[first + 1], p), dot(matrices[first + 2], p));
#ifdef THREE_BUFFERS
    w += extra[0].xyz;
#endif
    float x = w.x - camera.x;
    float y = w.y - camera.y;
    float z = w.z - camera.z;
    o.clip.x = clip_rows[0].w + (clip_rows[0].z * z + (clip_rows[0].y * y + clip_rows[0].x * x));
    o.clip.y = clip_rows[1].w + (clip_rows[1].z * z + (clip_rows[1].y * y + clip_rows[1].x * x));
    o.clip.z = clip_rows[2].w + (clip_rows[2].z * z + (clip_rows[2].y * y + clip_rows[2].x * x));
    o.clip.w = clip_rows[3].w + (clip_rows[3].z * z + (clip_rows[3].y * y + clip_rows[3].x * x));
    return o;
}
