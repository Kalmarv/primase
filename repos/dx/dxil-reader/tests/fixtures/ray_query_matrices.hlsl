// What a ray query reports of the instance it is offering a procedural candidate in (compute, one thread). Compiled with
// dxc -T cs_6_5 -E main (ray_query_matrices.dxil). One ray, origins[0] and directions[0], is traced; the first procedural
// candidate's object-to-world and world-to-object matrices (three rows of four, row by row: twelve floats each) and the
// world ray's origin and direction (three floats each) are written as floats' bits to results[0..30]; results[30] is 1
// when there was a candidate.
RaytracingAccelerationStructure scene : register(t0);
RWStructuredBuffer<uint> results : register(u0);

cbuffer Rays : register(b0) {
    float4 origins[1];
    float4 directions[1];
};

[numthreads(1, 1, 1)]
void main() {
    RayDesc ray;
    ray.Origin = origins[0].xyz;
    ray.Direction = directions[0].xyz;
    ray.TMin = 0.0;
    ray.TMax = 1000.0;
    RayQuery<RAY_FLAG_NONE> q;
    q.TraceRayInline(scene, 0, 0xff, ray);
    if (q.Proceed()) {
        float3x4 o2w = q.CandidateObjectToWorld3x4();
        float3x4 w2o = q.CandidateWorldToObject3x4();
        for (uint r = 0; r < 3; r++) {
            for (uint c = 0; c < 4; c++) {
                results[4 * r + c] = asuint(o2w[r][c]);
                results[12 + 4 * r + c] = asuint(w2o[r][c]);
            }
        }
        float3 o = q.WorldRayOrigin();
        float3 d = q.WorldRayDirection();
        results[24] = asuint(o.x);
        results[25] = asuint(o.y);
        results[26] = asuint(o.z);
        results[27] = asuint(d.x);
        results[28] = asuint(d.y);
        results[29] = asuint(d.z);
        results[30] = 1;
    }
}
