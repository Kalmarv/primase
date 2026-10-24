// Fixture for primase-frostbite (our own HLSL), four programs of one source:
//   dxc -T vs_6_0 -E main -Od -Fo camera_relative_vs.dxil camera_relative_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D NO_CAMERA -Fo camera_relative_vs.no_camera.dxil camera_relative_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D WORLD_USES_CAMERA -Fo camera_relative_vs.world_uses_camera.dxil camera_relative_vs.hlsl
//   dxc -T vs_6_0 -E main -Od -D OTHER_LAST_ROW -Fo camera_relative_vs.other_last_row.dxil camera_relative_vs.hlsl
// A vertex program that places its position as the census's camera-relative vertex programs do: each clip lane is four
// terms, the three coordinates of (world - camera) times the lane's row of a matrix and the row's constant, over the words
// 980 to 995 (the rows) and 1060 to 1062 (the camera) of the constant buffer at register 4. The variants break one thing
// each: no camera subtracted, the world position made of the camera, the last lane from another matrix.
cbuffer Pass : register(b4)
{
    float4 before[245];
    float4 clip_rows[4];
    float4 between[16];
    float3 camera;
    float4 after[3];
    float4 other_row;
};
struct VO { float4 clip : SV_Position; };
VO main(float3 position : POSITION0)
{
    VO o;
#ifdef NO_CAMERA
    float3 c = float3(0, 0, 0);
#else
    float3 c = camera;
#endif
#ifdef WORLD_USES_CAMERA
    float3 w = position + camera * 0.5;
#else
    float3 w = position;
#endif
    float x = w.x - c.x;
    float y = w.y - c.y;
    float z = w.z - c.z;
    o.clip.x = clip_rows[0].w + (clip_rows[0].z * z + (clip_rows[0].y * y + clip_rows[0].x * x));
    o.clip.y = clip_rows[1].w + (clip_rows[1].z * z + (clip_rows[1].y * y + clip_rows[1].x * x));
    o.clip.z = clip_rows[2].w + (clip_rows[2].z * z + (clip_rows[2].y * y + clip_rows[2].x * x));
#ifdef OTHER_LAST_ROW
    o.clip.w = other_row.w + (other_row.z * z + (other_row.y * y + other_row.x * x));
#else
    o.clip.w = clip_rows[3].w + (clip_rows[3].z * z + (clip_rows[3].y * y + clip_rows[3].x * x));
#endif
    return o;
}
