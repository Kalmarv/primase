//! Where an animation came from.
use primase_core::Component;

/// The binder and the member a clip was read from (`fromsoft.clip_origin`, on the clip's `havok.animation` entity): the
/// binder's file name (`c3000.anibnd`, or one of the divisions an Elden Ring character keeps its animations in,
/// `c3000_div01.anibnd`; a part form's `wp_a_0761_1.anibnd`) and the member's name as the binder stores it (the path of its
/// `.hkx`). The clip's own name is the member's file stem, which several divisions never share (a name read twice is refused).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ClipOrigin {
    pub binder: String,
    pub member: String,
}

impl Component for ClipOrigin {
    const NAME: &'static str = "fromsoft.clip_origin";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::World;

    /// The origin is a component of the clip's entity and reads back as JSON the way the reader states it.
    #[test]
    fn a_clip_origin_is_a_component_that_states_its_binder_and_member() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(
            e,
            ClipOrigin {
                binder: "c3000_div01.anibnd".into(),
                member: r"N:\NTC\data\Target\INTERROOT_win64\chr\c3000\hkx\a000_003000.hkx".into(),
            },
        );
        let origin = w.get::<ClipOrigin>(e).expect("the component");
        assert_eq!(origin.binder, "c3000_div01.anibnd");
        assert_eq!(
            ClipOrigin::NAME,
            "fromsoft.clip_origin",
            "the name the world's JSON and the lab read it by"
        );
        let json = origin.to_json().unwrap().expect("states itself");
        assert_eq!(json["binder"], "c3000_div01.anibnd");
        assert!(json["member"].as_str().unwrap().ends_with("a000_003000.hkx"));
    }
}
