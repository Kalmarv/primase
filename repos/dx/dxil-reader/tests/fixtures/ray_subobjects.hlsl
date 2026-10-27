// A ray tracing library with its state object subobjects (Shader Model 6.6): every kind the runtime data stores, and the
// programs they name. Compile it whole as
//   dxc -T lib_6_6 (ray_subobjects.dxil)
// and, to lift one program of it (the lifter takes a library of one), once for each of
//   -exports closest_hit, -exports any_hit, -exports intersection, -exports miss
// (ray_subobjects.<name>.dxil), checking with the whole library's runtime data that the subobjects are still in the
// container (if dxc drops them from an export-limited build, list the subobjects in -exports too).
// The subobjects, in the runtime data's order of declaration:
//   state_config: allow local dependencies on existing collections; global_signature: SRV(t0) as a global root signature;
//   local_signature: two root constants in b0; association: local_signature on closest_hit and miss; shader_config: payload
//   32 bytes, attributes 8; pipeline_config: recursion depth 2; triangle_group: closest_hit (no any-hit);
//   procedural_group: any_hit, closest_hit, intersection.
RaytracingAccelerationStructure scene : register(t0);

struct Payload {
    float distance;
    uint count;
};

StateObjectConfig state_config = { STATE_OBJECT_FLAGS_ALLOW_LOCAL_DEPENDENCIES_ON_EXTERNAL_DEFINITONS };
GlobalRootSignature global_signature = { "SRV(t0)" };
LocalRootSignature local_signature = { "RootConstants(num32BitConstants = 2, b0)" };
SubobjectToExportsAssociation association = { "local_signature", "closest_hit;miss" };
RaytracingShaderConfig shader_config = { 32, 8 };
RaytracingPipelineConfig pipeline_config = { 2 };
TriangleHitGroup triangle_group = { "", "closest_hit" };
ProceduralPrimitiveHitGroup procedural_group = { "any_hit", "closest_hit", "intersection" };

[shader("closesthit")]
void closest_hit(inout Payload payload, in BuiltInTriangleIntersectionAttributes attributes) {
    payload.distance = RayTCurrent();
    payload.count += 1;
}

[shader("anyhit")]
void any_hit(inout Payload payload, in BuiltInTriangleIntersectionAttributes attributes) {
    payload.count += 1;
}

struct Sphere {
    float2 uv;
};

[shader("intersection")]
void intersection() {
    Sphere sphere;
    sphere.uv = float2(0.5, 0.5);
    ReportHit(RayTCurrent(), 1, sphere);
}

[shader("miss")]
void miss(inout Payload payload) {
    payload.distance = -1;
}
