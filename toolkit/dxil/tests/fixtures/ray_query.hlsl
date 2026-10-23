// Inline ray tracing (compute): a query declared with flags, traced into an acceleration structure, stepped to the end
// and read for its committed hit. Compiled with dxc -T cs_6_5 -E main (ray_query.dxil, also -Od). The lifted program
// is validated and shown (naga's WGSL writer does not write ray queries); no executor runs it.
RaytracingAccelerationStructure scene : register(t0);
RWStructuredBuffer<float> results : register(u0);

cbuffer Ray : register(b0) {
    float3 origin;
    float tmin;
    float3 direction;
    float tmax;
};

[numthreads(1, 1, 1)]
void main() {
    RayQuery<RAY_FLAG_CULL_BACK_FACING_TRIANGLES> query;
    RayDesc ray;
    ray.Origin = origin;
    ray.TMin = tmin;
    ray.Direction = direction;
    ray.TMax = tmax;
    query.TraceRayInline(scene, RAY_FLAG_FORCE_OPAQUE, 0xff, ray);
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            query.CommitNonOpaqueTriangleHit();
        }
    }
    results[0] = query.CommittedStatus();
    results[1] = query.CommittedRayT();
    results[2] = query.CommittedInstanceID();
    results[3] = query.CommittedPrimitiveIndex();
    results[4] = query.CommittedTriangleBarycentrics().x;
    results[5] = query.CommittedGeometryIndex();
}
