//! Stored impostor geometry and the render explanations that survive lowering.
use primase_core::{Component, Entity};

#[derive(Clone,Debug,PartialEq)]
pub struct AnvilSurfaceBake {pub material:Entity,pub atlas_size:u32,pub texels_per_metre:f32}
impl Component for AnvilSurfaceBake{
    const NAME:&'static str="anvil.surface_bake";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::None;
    fn references(&self,out:&mut Vec<Entity>){out.push(self.material);}
}

#[derive(Clone,Debug,PartialEq)]
pub struct AnvilSkyControllers {
    pub look:String,
    pub time_of_day:crate::read::controller::Controller,
    pub weather:Option<crate::read::controller::Controller>,
}
impl Component for AnvilSkyControllers {
    const NAME:&'static str="anvil.sky_controllers";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::None;
    fn payload_bytes(&self)->u64{
        let bytes=|c:&crate::read::controller::Controller|c.tracks.iter().map(|t|t.path.len()*4+t.times.len()*2+t.values.len()*16).sum::<usize>();
        (bytes(&self.time_of_day)+self.weather.as_ref().map_or(0,bytes)) as u64
    }
}
/// Source-derived static lighting. The scene uses the same radiance panorama for background, diffuse SH and
/// roughness-prefiltered reflections; no LDR sky-colour approximation or GIDyn-as-baked-light interpretation.
#[derive(Clone,Debug,PartialEq)]
pub struct AnvilEnvironment {
    pub description:String,
    pub width:u32,
    pub height:u32,
    pub radiance:std::sync::Arc<Vec<[f32;4]>>,
    pub towards_sun:[f32;3],
    pub sunlight:[f32;3],
    pub ambient:[f32;3],
    pub fog:Option<AnvilFog>,
    pub notices:Vec<String>,
}
#[derive(Clone,Debug,PartialEq)]
pub struct AnvilFog {
    pub albedo:[f32;3],pub density:f32,pub bottom:f32,pub top:f32,pub phase:f32,
}
impl Component for AnvilEnvironment {
    const NAME:&'static str="anvil.environment";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::None;
    fn payload_bytes(&self)->u64{(self.radiance.len()*16) as u64}
    fn to_json(&self)->Result<Option<serde_json::Value>,primase_core::JsonError>{
        Ok(Some(serde_json::json!({"source":self.description,"width":self.width,"height":self.height,
            "towards_sun":self.towards_sun,"sunlight":self.sunlight,"ambient":self.ambient,
            "fog":self.fog.as_ref().map(|f|serde_json::json!({"albedo":f.albedo,"density":f.density,
                "bottom":f.bottom,"top":f.top,"phase":f.phase})),"notices":self.notices})))
    }
}

/// Coarse terrain held separately so the near-terrain lowering cannot accidentally lower full coarse rectangles.
/// The near source is referenced only until the ring has sampled its exact boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilFarTerrain {
    pub near: Entity,
    pub rect: [f64; 4],
    pub radius: f64,
    pub levels: Vec<super::AnvilTerrain>,
}
impl Component for AnvilFarTerrain {
    const NAME: &'static str = "anvil.far_terrain";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) { out.push(self.near); }
    fn payload_bytes(&self) -> u64 { self.levels.iter().map(Component::payload_bytes).sum() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnvilFarClass { Real, MergedProxy, Canopy, Tree }
impl Component for AnvilFarClass {
    const NAME: &'static str = "anvil.far_class";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(serde_json::json!({"class": format!("{self:?}")})))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilFarSelected;
impl Component for AnvilFarSelected {
    const NAME: &'static str = "anvil.far_selected";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnvilSky {
    pub index_id: u64,
    pub stored: crate::read::sky::StoredSky,
}
impl Component for AnvilSky {
    const NAME: &'static str = "anvil.sky";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        self.stored.keyframes.iter().map(|k| k.inscatter.len() + k.optical_depth.len()
            + k.irradiance.len() + k.sun_depth.len() + k.sun_irradiance.len()).sum::<usize>() as u64
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(serde_json::json!({"source_id": self.index_id, "sun_elevation": self.stored.sun_elevation,
            "sun_color": self.stored.sun_color, "keyframes": self.stored.keyframes.iter()
                .map(|key| serde_json::json!({"hour": key.hour, "crc": key.crc, "inscatter_bytes": key.inscatter.len()}))
                .collect::<Vec<_>>(), "runtime_weather_evaluated": false})))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnvilSpotLight {
    pub index_id: u64,
    pub stored: crate::read::light::StoredSpotLight,
}
impl Component for AnvilSpotLight {
    const NAME: &'static str = "anvil.spot_light";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        let light = &self.stored;
        Ok(Some(serde_json::json!({"source_id": self.index_id, "color": light.color,
            "stored_intensity": light.intensity, "stored_luminance": light.luminance,
            "inner": light.inner, "outer": light.outer, "range": light.range,
            "intensity_unit": "not established; lowering requires caller calibration"})))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnvilImpostor {
    pub index_id: u64,
    pub frame: primase_core::frame::SourceFrame,
    pub stored: crate::read::impostor::StoredImpostor,
    pub textures: [Option<Entity>; 4],
}

impl Component for AnvilImpostor {
    const NAME: &'static str = "anvil.impostor";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures.iter().flatten());
    }
    fn payload_bytes(&self) -> u64 {
        (self.stored.cards.len() * 108 + 199) as u64
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(serde_json::json!({"source_id": self.index_id, "top_cards": self.stored.top,
            "side_cards": self.stored.side, "texture_ids": self.stored.maps.map(|map| map.map(|r| r.id)),
            "textures": self.textures.map(|texture| texture.map(|e| e.index())),
            "cutoff": f32::from_bits(self.stored.table[14])})))
    }
}

/// Source-authored render semantics, retained beside PBR so a material with no texture is distinguishable
/// from a lost binding or an unsupported recipe. A diagnostic is never inferred merely from a grey pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilRenderStatus {
    pub source: String,
    pub template: Option<String>,
    pub reason: String,
    pub unsupported: bool,
}

impl Component for AnvilRenderStatus {
    const NAME: &'static str = "anvil.render_status";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        (self.source.len() + self.reason.len() + self.template.as_ref().map_or(0, String::len)) as u64
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(serde_json::json!({"source": self.source, "template": self.template,
            "reason": self.reason, "unsupported": self.unsupported})))
    }
}
