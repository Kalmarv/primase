// What a ray query remembers of the ray it was traced with (compute): its flags, origin, direction and t min.
// Compiled with dxc -T cs_6_5 -E main (ray_query_state.dxil, also -Od). Written to results as flags, origin, direction,
// t min; naga's WGSL writer does not write ray queries, and no executor runs it.
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
    query.Proceed();
    results[0] = query.RayFlags();
    results[1] = query.WorldRayOrigin().x;
    results[2] = query.WorldRayOrigin().y;
    results[3] = query.WorldRayOrigin().z;
    results[4] = query.WorldRayDirection().x;
    results[5] = query.WorldRayDirection().y;
    results[6] = query.WorldRayDirection().z;
    results[7] = query.RayTMin();
}
