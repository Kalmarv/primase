// Inline ray tracing against an acceleration structure (compute, one group of 5). Compiled with dxc -T cs_6_5 -E main
// (ray_query_hits.dxil, also -Od). Thread i traces the ray origins[i], directions[i] (t from 0 to 100) with the flags
// and instance mask of params[i].xy, commits every non-opaque triangle it is offered, and writes ten words to
// results[10 * i]: the committed status (0 none, 1 triangle), the distance (a float's bits), the instance index, the
// instance id, the instance's contribution to the hit group index, the geometry index, the primitive index, the two
// barycentric weights (floats' bits) and whether the triangle faces the ray (0 or 1).
RaytracingAccelerationStructure scene : register(t0);
RWStructuredBuffer<uint> results : register(u0);

cbuffer Rays : register(b0) {
    float4 origins[5];
    float4 directions[5];
    uint4 params[5];
};

[numthreads(5, 1, 1)]
void main(uint i : SV_GroupIndex) {
    RayDesc ray;
    ray.Origin = origins[i].xyz;
    ray.Direction = directions[i].xyz;
    ray.TMin = 0.0;
    ray.TMax = 100.0;
    RayQuery<RAY_FLAG_NONE> q;
    q.TraceRayInline(scene, params[i].x, params[i].y, ray);
    while (q.Proceed()) {
        if (q.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            q.CommitNonOpaqueTriangleHit();
        }
    }
    uint at = 10 * i;
    uint status = q.CommittedStatus();
    results[at + 0] = status;
    if (status == COMMITTED_TRIANGLE_HIT) {
        results[at + 1] = asuint(q.CommittedRayT());
        results[at + 2] = q.CommittedInstanceIndex();
        results[at + 3] = q.CommittedInstanceID();
        results[at + 4] = q.CommittedInstanceContributionToHitGroupIndex();
        results[at + 5] = q.CommittedGeometryIndex();
        results[at + 6] = q.CommittedPrimitiveIndex();
        float2 b = q.CommittedTriangleBarycentrics();
        results[at + 7] = asuint(b.x);
        results[at + 8] = asuint(b.y);
        results[at + 9] = q.CommittedTriangleFrontFace() ? 1 : 0;
    }
}
