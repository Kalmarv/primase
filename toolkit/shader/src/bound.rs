//! The size a constant buffer is bound with.
//!
//! A program declares its constant buffers at the size it indexes them (a skinning palette of two matrices where the
//! source's own code indexes any joint), and a source can bind more: D3D12 bounds a constant buffer read by the view
//! the draw binds, not by the program's declaration. [`BoundSizes`] states those sizes by buffer for a lifter, which
//! then gives the buffer's uniform array the bound size's rows (a read past the declaration but inside the bound is
//! the buffer's, not zero) and the interface's layout the bound size.
use std::collections::BTreeMap;

use primase_core::shader::StageBinding;

use crate::{Interface, ResourceId, ResourceKind};

/// The bound size, in bytes, of the constant buffers whose binding states one; a buffer not named is bound at its
/// declared size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundSizes(BTreeMap<ResourceId, u32>);

impl BoundSizes {
    /// No buffer is bound beyond its declaration.
    pub fn new() -> BoundSizes {
        BoundSizes::default()
    }

    /// The sizes a stage's binding states ([`StageBinding::constant_buffers`]).
    pub fn of(stage: &StageBinding) -> BoundSizes {
        BoundSizes(
            stage
                .constant_buffers
                .iter()
                .filter_map(|c| c.bound_size.map(|bytes| (c.resource, bytes)))
                .collect(),
        )
    }

    /// Bind `resource` at `bytes`.
    pub fn set(&mut self, resource: ResourceId, bytes: u32) {
        self.0.insert(resource, bytes);
    }

    /// The size `resource` is bound at, where one is stated.
    pub fn get(&self, resource: ResourceId) -> Option<u32> {
        self.0.get(&resource).copied()
    }

    /// Whether no buffer states one.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Raise the layout of each constant buffer of `interface` this names to its bound size. A bound smaller than the
    /// declaration is refused (the program indexes what it declares), as is a resource that is not a constant buffer
    /// of the program.
    pub fn apply(&self, interface: &mut Interface) -> Result<(), String> {
        for (&id, &bytes) in &self.0 {
            let Some(resource) = interface.resources.iter_mut().find(|r| r.id == id) else {
                return Err(format!(
                    "a bound size for resource {}, which the program does not have",
                    id.0
                ));
            };
            let ResourceKind::ConstantBuffer { layout } = &mut resource.kind else {
                return Err(format!(
                    "a bound size for resource {}, which is not a constant buffer",
                    id.0
                ));
            };
            if bytes < layout.size {
                return Err(format!(
                    "resource {} is bound at {bytes} bytes, below the {} its program declares",
                    id.0, layout.size
                ));
            }
            layout.size = bytes;
        }
        Ok(())
    }
}
