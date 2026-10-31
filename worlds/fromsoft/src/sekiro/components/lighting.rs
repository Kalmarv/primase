//! Prepared source lighting state, used to order registered graph and environment lowerings.
use primase_core::render_graph::RenderGraph;
use primase_core::{JsonError, Resource, World};

/// The registered graph preparation has supplied a core lighting pipeline. Environment graph augmentation requires
/// this resource as well as the graph, so raw reader output cannot be mistaken for prepared execution data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedLighting;
impl Resource for PreparedLighting {
    const NAME: &'static str = "fromsoft.prepared_lighting";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::json!({"prepared": true})))
    }
    fn verify(&self, world: &World) -> Result<(), String> {
        if world
            .resource::<RenderGraph>()
            .is_none_or(|g| g.lighting.is_none())
        {
            return Err("prepared lighting has no core render graph lighting pipeline".into());
        }
        Ok(())
    }
}
