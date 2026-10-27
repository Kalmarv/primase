// Fixture for primase-frostbite (our own HLSL), four programs of one source:
//   dxc -T vs_6_0 -E main -Od -Fo tables_vs.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D RECORD2 -Fo tables_vs.record2.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D NO_HEADER -Fo tables_vs.no_header.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D THREE_BUFFERS -Fo tables_vs.three_buffers.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D SKINNED -Fo tables_vs.skinned.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D SKINNED -D WEIGHTED_TRANSLATION -Fo tables_vs.skinned_weighted.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D SKINNED -D STRIDE_TWO -Fo tables_vs.skinned_stride2.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D SKINNED -D IGNORE_BIT18 -Fo tables_vs.skinned_bit18.dxil tables_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D SKINNED -D SECOND_BLOCK -Fo tables_vs.skinned_second_block.dxil tables_vs.hlsl
// A vertex program that finds its world position the way the census's instanced vertex programs do: the record of the
// instance (the buffer at t0, one word, or two with the matrix in the last) names the element of a header in the matrix
// table (t67), whose first word's bits move the three rows after it, and the clip position is the camera-relative form of
// camera_relative_vs.hlsl. The variants break one thing each: records of two words, rows read where the header is, a
// third buffer read, and a program that skins: four joints (TEXCOORD0, u32 x4) at four weights (TEXCOORD1, float x4) blend
// the joints' matrices (three rows each, after an element of translation that follows the header).
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
VO main(float3 position : POSITION0,
#ifdef SKINNED
        uint4 joints : TEXCOORD0, float4 weights : TEXCOORD1,
#endif
        uint id : SV_InstanceID)
{
    VO o;
#ifdef RECORD2
    uint rec = instances[base + id].y;
#else
    uint rec = instances[base + id];
#endif
    uint header = asuint(matrices[rec].x);
#ifdef SKINNED
    // the shape of the skinned vertex programs of the census (the hat's): the header's offset flag puts a float4 (the
    // instance's translation, added to the position after the weighted sum) later, the joints' three rows each follow it by
    // three elements, and the header's bit 18 puts them one element later still
    uint at_x = rec + 1 + ((header & 1) != 0 ? ((header >> 1) & 0xFFFF) : 0);
    float4 translation = matrices[at_x];
#ifdef IGNORE_BIT18
    uint first = at_x + 3;
#else
    uint first = at_x + 3 + ((header >> 18) & 1);
#endif
#ifdef STRIDE_TWO
#define JOINT_STRIDE 2
#else
#define JOINT_STRIDE 3
#endif
    float4 p = float4(position, 1);
    float3 w = float3(0, 0, 0);
#ifdef SECOND_BLOCK
    // the 6848-byte hat program also reads the elements the offset flag would have put the block at, the translation and
    // the first joint's rows, and no output moved when they were filled: read here, and used only by a select no fill takes
    float4 second_translation = matrices[rec + 1];
    float4 second_row = matrices[rec + 4 + ((header >> 18) & 1)];
    w.x += (second_translation.x > 1e30 || second_row.x > 1e30) ? 1.0 : 0.0;
#endif
    [unroll] for (uint k = 0; k < 4; k++) {
        uint at = first + JOINT_STRIDE * joints[k];
        w += weights[k] * float3(dot(matrices[at], p), dot(matrices[at + 1], p), dot(matrices[at + 2], p));
#ifdef WEIGHTED_TRANSLATION
        w += weights[k] * translation.xyz;
#endif
    }
#ifndef WEIGHTED_TRANSLATION
    w += translation.xyz;
#endif
#else
    uint later = ((header & 1) != 0 ? ((header >> 1) & 0xFFFF) : 0) + ((header >> 18) & 1);
#ifdef NO_HEADER
    uint first = rec;
#else
    uint first = rec + 1 + later;
#endif
    float4 p = float4(position, 1);
    float3 w = float3(dot(matrices[first], p), dot(matrices[first + 1], p), dot(matrices[first + 2], p));
#endif
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
