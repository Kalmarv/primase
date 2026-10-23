//! What a compute program asks of a device beyond WebGPU's baseline. A program is validated against D3D's limits (a
//! thread group of at most 1024 invocations, 1024 along x and y, 64 along z); WebGPU's guaranteed ones are smaller, so a
//! consumer checks the notes against its device's limits.

/// WebGPU's default limits: invocations per workgroup, size along x, y and z, and bytes of workgroup memory.
pub const WEBGPU_BASELINE: (u32, [u32; 3], u32) = (256, [256, 256, 64], 16384);

/// Notes for a module whose compute entry point exceeds [`WEBGPU_BASELINE`]: empty when it fits.
pub fn compute_limit_notes(module: &naga::Module) -> Vec<String> {
    let mut notes = Vec::new();
    let (invocations, axes, memory) = WEBGPU_BASELINE;
    for entry in &module.entry_points {
        if entry.stage != naga::ShaderStage::Compute {
            continue;
        }
        let size = entry.workgroup_size;
        let total = u64::from(size[0]) * u64::from(size[1]) * u64::from(size[2]);
        if total > u64::from(invocations) || size.iter().zip(axes).any(|(s, a)| *s > a) {
            notes.push(format!(
                "the thread group is {} by {} by {} ({total} invocations): WebGPU's default limits are \
                 {invocations} invocations and {} by {} by {}; the device's must be raised",
                size[0], size[1], size[2], axes[0], axes[1], axes[2]
            ));
        }
    }
    let shared: u64 = module
        .global_variables
        .iter()
        .filter(|(_, g)| g.space == naga::AddressSpace::WorkGroup)
        .map(|(_, g)| u64::from(module.types[g.ty].inner.size(module.to_ctx())))
        .sum();
    if shared > u64::from(memory) {
        notes.push(format!(
            "the thread group's shared memory is {shared} bytes: WebGPU's default limit is {memory}; the device's must \
             be raised"
        ));
    }
    notes
}

/// Notes for an interface that carries something WebGPU has no built-in for.
pub fn interface_notes(interface: &crate::Interface) -> Vec<String> {
    use crate::SystemValue::{RenderTargetArrayIndex, ViewportArrayIndex};
    let mut notes = Vec::new();
    let layered = |v: &crate::Varying| matches!(v.system, Some(RenderTargetArrayIndex | ViewportArrayIndex));
    if interface.inputs.iter().chain(&interface.outputs).any(layered) {
        notes.push(
            "a render target array index or viewport index is carried as a flat varying: WebGPU has no built-in for \
             it, and routing a primitive to the layer or viewport it names is the pipeline's"
                .into(),
        );
    }
    if interface
        .outputs
        .iter()
        .any(|v| v.system == Some(crate::SystemValue::CullDistance))
    {
        notes.push(
            "a cull distance is carried as a varying: WebGPU has no built-in for it, and culling a primitive whose \
             vertices' distances are all negative is the pipeline's"
                .into(),
        );
    }
    if interface.stage == crate::Stage::Mesh {
        notes.push(
            "a mesh program needs the device's mesh shader support (WebGPU's baseline has none): the vertices and \
             primitives it writes are the module's output variable, and it reads no payload of a task program"
                .into(),
        );
    }
    notes
}
