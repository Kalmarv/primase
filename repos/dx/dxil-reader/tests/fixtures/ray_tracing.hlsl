// The ray tracing pipeline's programs, one library of them (Shader Model 6.6). Compile each program alone as
//   dxc -T lib_6_6 -exports <name> (ray_tracing.<name>.dxil, also -Od)
// for the names raygen, miss, closest_hit, any_hit, intersection and callable. The payload is a distance and a count;
// the attributes the built-in triangle's barycentrics. Every operation of the pipeline is used once: TraceRay, ReportHit,
// CallShader, IgnoreHit, AcceptHitAndEndSearch, and the ray's system values.
RaytracingAccelerationStructure scene : register(t0);
RWTexture2D<float4> output : register(u0);

struct Payload {
    float distance;
    uint count;
    float3 color;
};

struct Parameter {
    float value;
};

[shader("raygeneration")]
void raygen() {
    uint2 at = DispatchRaysIndex().xy;
    uint2 size = DispatchRaysDimensions().xy;
    RayDesc ray;
    ray.Origin = float3((float2(at) + 0.5) / float2(size), 0);
    ray.Direction = float3(0, 0, 1);
    ray.TMin = 0.001;
    ray.TMax = 1000.0;
    Payload payload;
    payload.distance = -1;
    payload.count = 0;
    payload.color = 0;
    TraceRay(scene, RAY_FLAG_NONE, 0xff, 0, 1, 0, ray, payload);
    Parameter parameter;
    parameter.value = payload.distance;
    CallShader(0, parameter);
    output[at] = float4(payload.color * parameter.value, payload.distance);
}

[shader("miss")]
void miss(inout Payload payload) {
    payload.color = float3(0.1, 0.2, 0.3) * WorldRayDirection();
    payload.count = DispatchRaysIndex().x;
}

[shader("closesthit")]
void closest_hit(inout Payload payload, in BuiltInTriangleIntersectionAttributes attributes) {
    payload.distance = RayTCurrent() + RayTMin();
    payload.count = InstanceID() + InstanceIndex() + PrimitiveIndex() + GeometryIndex() + HitKind() + RayFlags();
    payload.color = float3(attributes.barycentrics, 1.0 - attributes.barycentrics.x - attributes.barycentrics.y)
        + WorldRayOrigin() + ObjectRayOrigin() + ObjectRayDirection() + ObjectToWorld3x4()[1] + WorldToObject3x4()[2];
}

[shader("anyhit")]
void any_hit(inout Payload payload, in BuiltInTriangleIntersectionAttributes attributes) {
    if (attributes.barycentrics.x < 0.25) {
        IgnoreHit();
    }
    if (attributes.barycentrics.y > 0.75) {
        AcceptHitAndEndSearch();
    }
    payload.count += 1;
}

struct Sphere {
    float2 uv;
};

[shader("intersection")]
void intersection() {
    Sphere sphere;
    sphere.uv = float2(0.5, 0.5);
    float t = RayTCurrent() + length(ObjectRayOrigin());
    ReportHit(t, 1, sphere);
}

[shader("callable")]
void callable(inout Parameter parameter) {
    parameter.value *= 2;
}
