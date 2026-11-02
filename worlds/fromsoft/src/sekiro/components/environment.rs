//! Authored environment metadata, explicit pass selections and requested/loaded payload identities.
use fromsoft_formats::{
    envmap::{BoxFields, Package, PackageInventory, Tier},
    msb::Region,
};
use primase_core::{Component, Entity, Geometry, JsonError, Resource, World};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Every supported package present in the archive, without choosing a tier or time state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnvironmentCatalog {
    pub area: String,
    pub packages: Vec<PackageInventory>,
}
impl Resource for EnvironmentCatalog {
    const NAME: &'static str = "fromsoft.environment_catalog";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// One environment region: where its box comes from, its fields, its probe group and its reflection origin.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnvironmentRegion {
    pub area: String,
    pub placement: Placement,
    /// The authored box's fields; a far tile's are the stand-ins its placement pass states.
    pub fields: BoxFields,
    pub group: u32,
    pub available_packages: Vec<String>,
    /// Core coordinate frame; the reader's frame conversion does not select any state.
    pub reflection_origin: [f32; 3],
}
impl EnvironmentRegion {
    /// The region's name: the authored box's, or the far tile's group.
    pub fn name(&self) -> String {
        match &self.placement {
            Placement::Authored(region) => region.name.clone(),
            Placement::FarTile { layer, .. } => format!("far tile group {:04} (layer {layer})", self.group),
        }
    }
}

/// Where an environment region's box comes from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Placement {
    /// An `EnvironmentMapEffectBox` of the layout, its shape and pose as stored.
    Authored(Region),
    /// A far tile's own probe group ([`FarTileEnvironmentGroup`]), which no box of the layout names, placed by a pass:
    /// one of the tile's layers stacked from the ground up (`layer` 0 the lowest), a box over the tile in its stored
    /// frame (`centre` and `half_extents` in metres), placed as `how` says.
    FarTile {
        layer: u8,
        centre: [f32; 3],
        half_extents: [f32; 3],
        how: FarTilePlacement,
    },
}

/// How a far tile's box was placed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum FarTilePlacement {
    /// Fitted to the groups' own volumes by `place-fromsoft-far-tile-environment`; the game's placement is not traced.
    Fitted,
}

/// A far tile's own probe group as the source holds it: the group of the tile's environment packages that no box of
/// its layout names, and the packages that hold it. The source places it nowhere; a placement pass gives it an
/// [`EnvironmentRegion`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FarTileEnvironmentGroup {
    pub area: String,
    pub group: u32,
    pub available_packages: Vec<String>,
}
impl Component for FarTileEnvironmentGroup {
    const NAME: &'static str = "fromsoft.far_tile_environment_group";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl Component for EnvironmentRegion {
    const NAME: &'static str = "fromsoft.environment_region";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A program's explicit selection, attached only by the registered selection pass.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct EnvironmentSelection {
    pub state_a: u8,
    pub state_b: u8,
    pub factor: f32,
    pub tier: Tier,
}
impl EnvironmentSelection {
    pub fn verify_values(self) -> Result<(), String> {
        if self.state_a > 99
            || self.state_b > 99
            || !self.factor.is_finite()
            || !(0. ..=1.).contains(&self.factor)
        {
            return Err(
                "environment states must fit two decimal digits and factor must be finite in [0,1]".into(),
            );
        }
        Ok(())
    }
    pub fn notice(self) -> String {
        format!(
            "environment states {:02}->{:02} at {} and tier {} stated by the caller; the game's clock selector is not traced",
            self.state_a,
            self.state_b,
            self.factor,
            self.tier.word()
        )
    }
}
impl Resource for EnvironmentSelection {
    const NAME: &'static str = "fromsoft.environment_selection";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn verify(&self, _: &World) -> Result<(), String> {
        self.verify_values()
    }
}

/// A selection pass's load request. The reader loads these packages and makes no quality/state choice.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnvironmentLoadRequest(pub Vec<Package>);
impl Resource for EnvironmentLoadRequest {
    const NAME: &'static str = "fromsoft.environment_load_request";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn verify(&self, world: &World) -> Result<(), String> {
        let catalog = world
            .resource::<EnvironmentCatalog>()
            .ok_or("environment request has no authored catalog")?;
        if self
            .0
            .iter()
            .any(|p| !catalog.packages.iter().any(|entry| &entry.package == p))
        {
            return Err("environment request names a package absent from the authored catalog".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct EnvironmentGroup {
    pub coefficients: [Option<Entity>; 4],
    pub cube: Option<Entity>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnvironmentPayload {
    pub package: Package,
    pub groups: BTreeMap<u32, EnvironmentGroup>,
}
/// Loaded package payloads; texture entities are retained through these declared references.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EnvironmentPayloads(pub Vec<EnvironmentPayload>);
impl Resource for EnvironmentPayloads {
    const NAME: &'static str = "fromsoft.environment_payloads";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for p in &self.0 {
            for g in p.groups.values() {
                out.extend(g.coefficients.iter().flatten());
                out.extend(g.cube);
            }
        }
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        for p in &mut self.0 {
            for g in p.groups.values_mut() {
                for e in g.coefficients.iter_mut().chain(std::iter::once(&mut g.cube)) {
                    if e.is_some_and(|e| !keep(e)) {
                        *e = None;
                    }
                }
            }
        }
    }
}
