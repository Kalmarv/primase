//! What the stages that run a function at a time (geometry, hull and domain programs) share: how a program's resources
//! are found in the caller's bindings.

use std::collections::BTreeMap;

use super::pixel::SlotBackend;
use super::{Bound, Executable};
use crate::{BufferKind, D3dClass, Program, ResourceId, ResourceKind, Sampler, StorageTarget};

/// The rows of constant buffers, by (register, space).
pub type Constants = BTreeMap<(u32, u32), Vec<[u32; 4]>>;

/// Where the resources of a program are bound.
#[derive(Debug)]
pub(super) struct Bindings {
    /// The constant buffers by resource, as (register, space).
    constants: BTreeMap<ResourceId, (u32, u32)>,
    slots: BTreeMap<ResourceId, u32>,
    buffers: BTreeMap<ResourceId, BufferKind>,
}

impl Bindings {
    pub(super) fn new(program: &Program) -> Result<Self, String> {
        let mut constants = BTreeMap::new();
        let mut slots = BTreeMap::new();
        let mut buffers = BTreeMap::new();
        for r in &program.interface.resources {
            let Some(binding) = r.provenance.d3d else {
                return Err(format!("resource {} has no D3D bind slot", r.provenance.name));
            };
            slots.insert(r.id, binding.register);
            if matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                && binding.class == D3dClass::ConstantBuffer
            {
                constants.insert(r.id, (binding.register, binding.space));
            }
            if let ResourceKind::Buffer(kind)
            | ResourceKind::Storage {
                target: StorageTarget::Buffer(kind),
                ..
            } = &r.kind
            {
                buffers.insert(r.id, *kind);
            }
        }
        Ok(Self {
            constants,
            slots,
            buffers,
        })
    }

    /// The buffers `executable` reads, from the caller's `constants`.
    pub(super) fn bind<'a>(
        &self,
        executable: &Executable,
        constants: &'a Constants,
    ) -> Result<Bound<'a>, String> {
        executable.bind(
            |id| {
                let key = self.constants.get(&id)?;
                constants.get(key).map(Vec::as_slice)
            },
            |_| None,
        )
    }

    /// The program's requests in the sampler's vocabulary: bind slots.
    pub(super) fn backend<'a>(&'a self, sampler: &'a mut dyn Sampler) -> SlotBackend<'a> {
        SlotBackend {
            sampler,
            slots: &self.slots,
            buffers: &self.buffers,
        }
    }
}
