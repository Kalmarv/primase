//! Anvil's content notes (pass `note-anvil-content`): what a read of Anvil content states about an entity that a
//! viewer should show, as `core.content_note` on that entity, and a `core.marker` per GI probe position.
//!
//! One note per source fact, keyed by what the fact is; the reasons are the program's own words for it, moved here with
//! the fact so every consumer shows the same text:
//! - `anvil.material_recipe`, from an [`AnvilRenderStatus`]: its reason and whether it is unsupported. This is the
//!   key and the text the sentinel of a material with no derived PBR material is drawn under, so the counts and the
//!   wording of a refusal do not change with this pass;
//! - `anvil.source_components`, from an [`AnvilSourceComponents`]: the retained source components, each type named
//!   through the format's own type table ([`crate::components::component_types::COMPONENT_TYPES`]) with its stored
//!   status;
//! - `anvil.grid_metadata`, `anvil.floor_height`, `anvil.look_preset`: one note each, the resource's role and what
//!   Properties > Data shows of it;
//! - `anvil.gi_dyn`, from an [`AnvilGiDyn`]: its probe count and that the irradiance is unresolved, positioned at the
//!   GI frame (an entity with no transform); and, per probe position, an entity of its own carrying
//!   `core.marker {kind: "gi_probe"}`, `core.name` where the frame has a name, a `core.transform` at the position and
//!   a note keyed `anvil.gi_probe` saying the irradiance is unresolved. A marker is a position, not a light colour.
//!
//! Nothing is invented: a note repeats what the component states, and a refused interpretation (the GI irradiance, the
//! look preset's lane roles) stays named as such (`unsupported` where something is not drawn). An entity that already
//! holds another note of this pass (no Anvil component does today) keeps the later one and is counted.

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{ContentNote, Entity, Marker, Name, World};

use crate::components::component_types::component_name;
use crate::components::{
    AnvilFloorHeight, AnvilGiDyn, AnvilGridMetadata, AnvilLookPreset, AnvilRenderStatus,
    AnvilSourceComponents,
};

/// Put `note` on `e`, counted when it replaces a different note this pass left there before.
fn note(w: &mut World, e: Entity, note: ContentNote, log: &mut PassLog) {
    let replaced = w.get::<ContentNote>(e).is_some_and(|previous| *previous != note);
    if replaced {
        log.omit("entities whose second Anvil content note replaces the first", 1);
    }
    w.insert(e, note);
}

/// The reading of a GI frame, kept apart from the loop below so the frame's own note and its markers are made from the
/// same values.
fn gi_notes(w: &mut World, entity: Entity, gi: &AnvilGiDyn, log: &mut PassLog) {
    note(
        w,
        entity,
        ContentNote {
            key: "anvil.gi_dyn".to_owned(),
            reason: format!(
                "{} GI probe positions; irradiance unresolved. Markers are positions, not light colours. Stream sizes in Properties > Data",
                gi.stored.probes
            ),
            unsupported: true,
            at: Some(gi.frame.point(gi.stored.position)),
        },
        log,
    );
    let name = w.get::<Name>(entity).map(|name| name.0.clone());
    for (index, source) in gi.stored.probe_positions().enumerate() {
        let at = gi.frame.point(source);
        let probe = w.spawn();
        w.insert(probe, super::placed_at(at));
        w.insert(
            probe,
            Marker {
                kind: "gi_probe".to_owned(),
                ground: None,
                yaw: None,
            },
        );
        if let Some(name) = &name {
            w.insert(
                probe,
                Name(format!("{name} probe {index} (irradiance unresolved)")),
            );
        }
        w.insert(
            probe,
            ContentNote {
                key: "anvil.gi_probe".to_owned(),
                reason: "irradiance unresolved".to_owned(),
                unsupported: true,
                at: None,
            },
        );
        log.place(probe);
    }
}

/// Note every Anvil content fact in `w` (see the module documentation).
pub fn note_anvil_content(w: &mut World) -> PassLog {
    let mut log = PassLog::default();
    let statuses: Vec<(Entity, AnvilRenderStatus)> = w
        .iter::<AnvilRenderStatus>()
        .map(|(e, status)| (e, status.clone()))
        .collect();
    for (e, status) in statuses {
        note(
            w,
            e,
            ContentNote {
                key: "anvil.material_recipe".to_owned(),
                reason: status.reason,
                unsupported: status.unsupported,
                at: None,
            },
            &mut log,
        );
    }
    let sources: Vec<(Entity, AnvilSourceComponents)> = w
        .iter::<AnvilSourceComponents>()
        .map(|(e, source)| (e, source.clone()))
        .collect();
    for (e, source) in sources {
        let types = source
            .components
            .iter()
            .map(|c| {
                let name = component_name(c.type_id).unwrap_or("unknown");
                format!("{name} ({:#010x}): {}", c.type_id, c.status)
            })
            .collect::<Vec<_>>()
            .join("; ");
        note(
            w,
            e,
            ContentNote {
                key: "anvil.source_components".to_owned(),
                reason: format!("retained source components, inspect Properties > Data: {types}"),
                unsupported: false,
                at: None,
            },
            &mut log,
        );
    }
    let grids: Vec<Entity> = w.iter::<AnvilGridMetadata>().map(|(e, _)| e).collect();
    for e in grids {
        note(
            w,
            e,
            ContentNote {
                key: "anvil.grid_metadata".to_owned(),
                reason: "stored grid metadata in Properties > Data; structural reference lists are not placed scene geometry; additional World fields remain unread".to_owned(),
                unsupported: false,
                at: None,
            },
            &mut log,
        );
    }
    let floors: Vec<Entity> = w.iter::<AnvilFloorHeight>().map(|(e, _)| e).collect();
    for e in floors {
        note(
            w,
            e,
            ContentNote {
                key: "anvil.floor_height".to_owned(),
                reason: "coarse FloorHeightData retained; Properties > Data gives grid dimensions and height-word scale; vertical sampling/lowering must be stated by the caller".to_owned(),
                unsupported: false,
                at: None,
            },
            &mut log,
        );
    }
    let presets: Vec<Entity> = w.iter::<AnvilLookPreset>().map(|(e, _)| e).collect();
    for e in presets {
        note(
            w,
            e,
            ContentNote {
                key: "anvil.look_preset".to_owned(),
                reason: "look preset metadata in Properties > Data; forecast lane roles/event activation unresolved; this resource has no authored position".to_owned(),
                unsupported: false,
                at: None,
            },
            &mut log,
        );
    }
    let frames: Vec<(Entity, AnvilGiDyn)> = w.iter::<AnvilGiDyn>().map(|(e, gi)| (e, gi.clone())).collect();
    for (e, gi) in frames {
        gi_notes(w, e, &gi, &mut log);
    }
    log
}

/// [`note_anvil_content`] as a pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteAnvilContent;

/// `note-anvil-content`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, it notes what the world
/// states.
pub const NOTE_ANVIL_CONTENT: PassSpec = PassSpec {
    name: "note-anvil-content",
    options: &[],
    make: |_| Ok(Box::new(NoteAnvilContent)),
};

impl Pass for NoteAnvilContent {
    fn name(&self) -> &'static str {
        "note-anvil-content"
    }
    /// Nothing: it notes the Anvil components a world holds, any or none of them, each counted by its presence.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    /// `core.content_note` on the entities the components are on, and the GI probes' `core.marker`, `core.name` and
    /// `core.transform`.
    fn provides(&self) -> &[&'static str] {
        &["core.content_note", "core.marker", "core.name", "core.transform"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        log.absorb(note_anvil_content(w));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use primase_core::Component;

    use super::*;
    use crate::components::SourceComponent;
    use crate::components::component_types::COMPONENT_TYPES;
    use anvil_formats::gidyn::StoredGiDyn;
    use anvil_formats::grid::StoredGridPartition;
    use anvil_formats::look::StoredLook;

    /// A world with one of each Anvil content fact, and a GI frame of two probes.
    fn world() -> World {
        let mut w = World::new();
        let material = w.spawn();
        w.insert(
            material,
            AnvilRenderStatus {
                source: "lower-anvil-material".to_owned(),
                template: Some("template".to_owned()),
                reason: "no shader".to_owned(),
                unsupported: true,
            },
        );
        let source = w.spawn();
        w.insert(
            source,
            AnvilSourceComponents {
                owner: 0,
                components: vec![SourceComponent {
                    type_id: COMPONENT_TYPES[0].0,
                    id: 1,
                    status: "kept as stored".to_owned(),
                    bytes: Vec::new(),
                }],
            },
        );
        let grid = w.spawn();
        w.insert(
            grid,
            AnvilGridMetadata::Partition(StoredGridPartition {
                levels: 1,
                cells: Vec::new(),
            }),
        );
        let floor = w.spawn();
        w.insert(
            floor,
            AnvilFloorHeight {
                frame: primase_core::frame::SourceFrame::WORLD,
                cell: 1.0,
                scale: 1.0,
                origin: [0.0; 2],
                cells: [0; 2],
                blocks: Vec::new(),
            },
        );
        let preset = w.spawn();
        w.insert(
            preset,
            AnvilLookPreset {
                index_id: 1,
                stored: StoredLook::Forecast { values: [0.0; 12] },
            },
        );
        let frame = w.spawn();
        w.insert(frame, Name("gi cell".to_owned()));
        w.insert(
            frame,
            AnvilGiDyn {
                index_id: 1,
                frame: primase_core::frame::SourceFrame::WORLD,
                stored: StoredGiDyn {
                    world_id: 0,
                    cell_id: 0,
                    position: [0.0; 3],
                    samples: 0,
                    probes: 2,
                    indoor: false,
                    voxel_bounds: [[0; 3]; 2],
                    subvolume_word: 0,
                    // the fourth stream is the probe positions: 8 bytes each, two of them at the frame's position
                    streams: [Vec::new(), Vec::new(), Vec::new(), vec![0; 16]],
                    decoded_bytes: [0; 3],
                },
            },
        );
        w
    }

    fn note_of(w: &World, e: Entity) -> &ContentNote {
        let note = w.get::<ContentNote>(e).expect("a content note");
        note.verify(e, w).expect("a valid content note");
        note
    }

    #[test]
    fn every_anvil_fact_gets_its_note_under_its_own_key() {
        let mut w = world();
        let _ = note_anvil_content(&mut w);
        let statuses: Vec<Entity> = w.iter::<AnvilRenderStatus>().map(|(e, _)| e).collect();
        assert_eq!(note_of(&w, statuses[0]).key, "anvil.material_recipe");
        assert_eq!(note_of(&w, statuses[0]).reason, "no shader");
        assert!(note_of(&w, statuses[0]).unsupported);
        let sources: Vec<Entity> = w.iter::<AnvilSourceComponents>().map(|(e, _)| e).collect();
        let source = note_of(&w, sources[0]);
        assert_eq!(source.key, "anvil.source_components");
        assert_eq!(
            source.reason,
            format!(
                "retained source components, inspect Properties > Data: {} ({:#010x}): kept as stored",
                COMPONENT_TYPES[0].1, COMPONENT_TYPES[0].0
            )
        );
        assert!(!source.unsupported);
        let grids: Vec<Entity> = w.iter::<AnvilGridMetadata>().map(|(e, _)| e).collect();
        assert_eq!(
            note_of(&w, grids[0]).reason,
            "stored grid metadata in Properties > Data; structural reference lists are not placed scene geometry; additional World fields remain unread"
        );
        let floors: Vec<Entity> = w.iter::<AnvilFloorHeight>().map(|(e, _)| e).collect();
        assert_eq!(note_of(&w, floors[0]).key, "anvil.floor_height");
        assert_eq!(
            note_of(&w, floors[0]).reason,
            "coarse FloorHeightData retained; Properties > Data gives grid dimensions and height-word scale; vertical sampling/lowering must be stated by the caller"
        );
        let presets: Vec<Entity> = w.iter::<AnvilLookPreset>().map(|(e, _)| e).collect();
        assert_eq!(note_of(&w, presets[0]).key, "anvil.look_preset");
        assert_eq!(
            note_of(&w, presets[0]).reason,
            "look preset metadata in Properties > Data; forecast lane roles/event activation unresolved; this resource has no authored position"
        );
    }

    #[test]
    fn a_gi_frame_is_noted_at_its_position_and_each_probe_is_a_marker() {
        let mut w = world();
        let frame = w.iter::<AnvilGiDyn>().map(|(e, _)| e).next().expect("a GI frame");
        let mut log = PassLog::default();
        log.absorb(note_anvil_content(&mut w));
        let gi = note_of(&w, frame);
        assert_eq!(gi.key, "anvil.gi_dyn");
        assert_eq!(
            gi.reason,
            "2 GI probe positions; irradiance unresolved. Markers are positions, not light colours. Stream sizes in Properties > Data"
        );
        assert!(gi.unsupported);
        assert_eq!(gi.at, Some([0.0; 3]));
        let probes: Vec<Entity> = w.iter::<Marker>().map(|(e, _)| e).collect();
        assert_eq!(probes.len(), 2);
        assert_eq!(log.placed.len(), 2);
        for (index, probe) in probes.iter().enumerate() {
            let marker = w.get::<Marker>(*probe).expect("a marker");
            assert_eq!(marker.kind, "gi_probe");
            marker.verify(*probe, &w).expect("a valid marker");
            assert_eq!(
                w.get::<Name>(*probe).map(|name| name.0.clone()),
                Some(format!("gi cell probe {index} (irradiance unresolved)"))
            );
            let note = note_of(&w, *probe);
            assert_eq!(note.key, "anvil.gi_probe");
            assert_eq!(note.reason, "irradiance unresolved");
        }
        // a second run states the same facts again; the frame's note is unchanged
        let again = note_anvil_content(&mut w);
        assert_eq!(note_of(&w, frame).key, "anvil.gi_dyn");
        assert_eq!(again.omitted.len(), 0);
    }

    #[test]
    fn the_pass_registers_and_takes_no_options() {
        let mut registry = primase_core::pass::PassRegistry::new();
        registry.add(&NOTE_ANVIL_CONTENT);
        assert!(registry.describe("note-anvil-content").is_some());
        assert!(!registry.requires_options("note-anvil-content"));
        assert!(
            registry
                .make("note-anvil-content", &primase_core::pass::Options::new())
                .is_ok()
        );
    }
}
