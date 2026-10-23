// Inline ray tracing with procedural primitives (compute, one group of 64). Compiled with dxc -T cs_6_5 -E main
// (ray_query_procedural.dxil). Thread i traces the ray origins[i], directions[i] (t from 0 to 1000) with the flags and
// instance mask of params[i].xy. A triangle is committed as in ray_query_grid.hlsl; a procedural primitive is a box that
// bounds the sphere spheres[PrimitiveIndex] (centre in xyz, radius in w, in the object space of its instance): the
// program intersects the ray with the sphere there and commits the nearer hit at or after the ray's t min. Ten words
// are written to results[10 * i]: the committed status (0 none, 1 triangle, 2 procedural), the distance (a float's
// bits), the instance index, the instance id, the instance's contribution to the hit group index, the geometry index,
// the primitive index, then for a triangle its two barycentric weights (floats' bits) and its facing, 0 for the rest.
RaytracingAccelerationStructure scene : register(t0);
RWStructuredBuffer<uint> results : register(u0);

cbuffer Rays : register(b0) {
    float4 origins[64];
    float4 directions[64];
    uint4 params[64];
    float4 spheres[8];
};

[numthreads(64, 1, 1)]
void main(uint i : SV_GroupIndex) {
    RayDesc ray;
    ray.Origin = origins[i].xyz;
    ray.Direction = directions[i].xyz;
    ray.TMin = 0.0;
    ray.TMax = 1000.0;
    RayQuery<RAY_FLAG_NONE> q;
    q.TraceRayInline(scene, params[i].x, params[i].y, ray);
    while (q.Proceed()) {
        if (q.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            q.CommitNonOpaqueTriangleHit();
        } else {
            float3x4 world_to_object = q.CandidateWorldToObject3x4();
            float3 o = mul(world_to_object, float4(q.WorldRayOrigin(), 1.0));
            float3 d = mul(world_to_object, float4(q.WorldRayDirection(), 0.0));
            float4 sphere = spheres[q.CandidatePrimitiveIndex() & 7];
            float3 m = o - sphere.xyz;
            float a = dot(d, d);
            float b = dot(m, d);
            float c = dot(m, m) - sphere.w * sphere.w;
            float disc = b * b - a * c;
            if (disc >= 0.0) {
                float t = (-b - sqrt(disc)) / a;
                // only a hit nearer than the one committed already: whether a farther one is ignored (as DXR says) or
                // taken is not the same on every implementation (WARP takes it), so the program does not ask
                if (t >= q.RayTMin() && (q.CommittedStatus() == COMMITTED_NOTHING || t < q.CommittedRayT())) {
                    q.CommitProceduralPrimitiveHit(t);
                }
            }
        }
    }
    uint at = 10 * i;
    uint status = q.CommittedStatus();
    results[at + 0] = status;
    if (status != COMMITTED_NOTHING) {
        results[at + 1] = asuint(q.CommittedRayT());
        results[at + 2] = q.CommittedInstanceIndex();
        results[at + 3] = q.CommittedInstanceID();
        results[at + 4] = q.CommittedInstanceContributionToHitGroupIndex();
        results[at + 5] = q.CommittedGeometryIndex();
        results[at + 6] = q.CommittedPrimitiveIndex();
        if (status == COMMITTED_TRIANGLE_HIT) {
            float2 bary = q.CommittedTriangleBarycentrics();
            results[at + 7] = asuint(bary.x);
            results[at + 8] = asuint(bary.y);
            results[at + 9] = q.CommittedTriangleFrontFace() ? 1 : 0;
        }
    }
}
