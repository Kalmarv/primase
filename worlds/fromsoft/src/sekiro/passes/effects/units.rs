//! The effect files' enumerations and conventions as core's: blend modes, orientations, attachments, initial directions,
//! distance limits and lighting. The numbers are the fxr project's (EvenTorset/fxr, Unlicense: sourced); a value the project
//! does not name is counted in [`Notes`] and given the nearest core meaning.
//!
//! **The frame.** A file's local axes are converted to core's by negating z ([`crate::pose::placed`] does it for matrices): a
//! direction the project states as "north (+Z)" is core's `-Z`, "south" `+Z`. The vectors here are core's.

use super::value::Notes;
use primase_core::particles::{
    Anchor, Attachment, Blend, DistanceFade, Follow, Heading, LayerOperation, LayerTarget, MeshOrientation,
    Orientation, TrailOrientation,
};

/// A direction the file states, in core's axes.
pub fn dir(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[1], -v[2]]
}

/// The file's local +Z ("north", the frame's forward), in core's axes.
pub const FORWARD: [f32; 3] = [0.0, 0.0, -1.0];

/// A blend mode (`BlendMode`): 0 and 7 are add and 6 normal in the project's tests ("seemingly identical"), 3 multiply.
pub fn blend(mode: i32, notes: &mut Notes) -> Blend {
    match mode {
        1 => Blend::Replace,
        2 => Blend::Alpha,
        3 => Blend::Multiply,
        4 => Blend::Additive,
        5 => Blend::Subtractive,
        0 | 7 => {
            notes.approximate(
                "blend modes 0 and 7 as additive (the project's reading: seemingly identical to add)",
            );
            Blend::Additive
        }
        6 => {
            notes
                .approximate("blend mode 6 as normal (the project's reading: seemingly identical to normal)");
            Blend::Alpha
        }
        _ => {
            notes.lose("a blend mode the project does not name, taken as normal");
            Blend::Alpha
        }
    }
}

/// What a multi-texture billboard's second and third layers change, by its layer word (`unk_ds3_f2_10`), the project's
/// documented table: 0, both the colour (the opacity is the first layer's); 1, both the opacity by their brightness;
/// 2, the second the opacity by its brightness and the third the colour. The effect pixel program reads the same three
/// values (`g_ps_TexBlendType`) the same way. A value the table does not name is counted and taken as 0.
pub fn layer_targets(word: i32, notes: &mut Notes) -> [LayerTarget; 2] {
    match word {
        0 => [LayerTarget::Colour, LayerTarget::Colour],
        1 => [LayerTarget::Opacity, LayerTarget::Opacity],
        2 => [LayerTarget::Opacity, LayerTarget::Colour],
        _ => {
            notes.lose(
                "a multi-texture layer word the project does not name, taken as 0 (both layers the colour)",
            );
            [LayerTarget::Colour, LayerTarget::Colour]
        }
    }
}

/// How a multi-texture billboard's second or third layer combines, by its operation word (`unk_ds3_f2_11` for the second,
/// `unk_ds3_f2_12` for the third): 0 multiply, 1 add, 2 overlay, as the effect pixel program's `g_ps_TexBlendType2` and
/// `g_ps_TexBlendType3` choose them. The words are read as those two from the values they hold (0 to 2 in every Elden Ring
/// effect file) and their place after the layer word: an inference, counted where a word is not 0. `None` for a value the
/// program combines in no way: the layer then changes nothing.
pub fn layer_operation(word: i32, notes: &mut Notes) -> Option<LayerOperation> {
    if word != 0 {
        notes.approximate(
            "a multi-texture layer's operation read from its word (unk_ds3_f2_11, unk_ds3_f2_12) as the effect pixel \
             program's TexBlendType2 and TexBlendType3 (an inference from their values and place)",
        );
    }
    match word {
        0 => Some(LayerOperation::Multiply),
        1 => Some(LayerOperation::Add),
        2 => Some(LayerOperation::Overlay),
        _ => {
            notes.lose("a multi-texture layer of an operation word the effect pixel program does not combine (left out)");
            None
        }
    }
}

/// What a flat particle faces (`OrientationMode`).
pub fn orientation(mode: i32, notes: &mut Notes) -> Orientation {
    match mode {
        // global south (-Z of the file) is core's +Z; the particle's normal points that way
        0 => Orientation::World([0.0, 0.0, 1.0]),
        1 => Orientation::CameraPlane,
        // the -Z of the parent node
        2 => Orientation::Frame([0.0, 0.0, 1.0]),
        3 => {
            notes.approximate("orientation 3 (south, a different projection) as south");
            Orientation::World([0.0, 0.0, 1.0])
        }
        4 => Orientation::CameraYaw,
        5 => Orientation::World([1.0, 0.0, 0.0]),
        6 => Orientation::Camera,
        7 => Orientation::FrameYaw,
        _ => {
            notes.lose("an orientation mode the project does not name, taken as the camera plane");
            Orientation::CameraPlane
        }
    }
}

/// The direction of a model particle's model that points along its travel (`ModelOrientationMode` 2): the model's +Y. A
/// captured frame of Elden Ring (a spell's crystal trail, Model 83050 with rotation x 180 and a random rotation z) puts
/// every model's +Y along the way it moves, turned back by the rotation x, and turns it about that axis.
pub const MODEL_FORWARD: [f32; 3] = [0.0, 1.0, 0.0];

/// The direction of a rich model particle's model that points along its travel (`RichModelOrientationMode` 2): the
/// model's +Z as the game stores it, -Z in the world frame (which mirrors Z). The same captured frame (RichModel 84016,
/// random rotation z) puts its +Z along the way the spell flies and turns it about that axis.
pub const RICH_MODEL_FORWARD: [f32; 3] = [0.0, 0.0, -1.0];

/// What a model particle faces (`ModelOrientationMode`).
pub fn model_orientation(mode: i32, notes: &mut Notes) -> MeshOrientation {
    match mode {
        0 => MeshOrientation::World,
        1 => MeshOrientation::World,
        2 => MeshOrientation::Heading {
            forward: MODEL_FORWARD,
        },
        3..=5 => MeshOrientation::CameraYaw,
        _ => {
            notes.lose("a model orientation mode the project does not name, taken as the emitter frame");
            MeshOrientation::Frame
        }
    }
}

/// What a rich model particle faces (`RichModelOrientationMode`; its values 0 to 4 as the model's, 3 north).
pub fn rich_model_orientation(mode: i32, notes: &mut Notes) -> MeshOrientation {
    match mode {
        0 | 3 => MeshOrientation::World,
        1 => MeshOrientation::World,
        2 => MeshOrientation::Heading {
            forward: RICH_MODEL_FORWARD,
        },
        4 => MeshOrientation::CameraYaw,
        _ => {
            notes.lose("a rich model orientation mode the project does not name, taken as the emitter frame");
            MeshOrientation::Frame
        }
    }
}

/// How a tracer's strip faces (`TracerOrientationMode`): the strip is perpendicular to the direction of travel and, for
/// `Travel`, to the camera, so it faces the camera; the others are fixed axes. `Cross` (two strips) and `Diagonal` are counted.
pub fn tracer_orientation(mode: i32, notes: &mut Notes) -> TrailOrientation {
    match mode {
        0 => TrailOrientation::Camera,
        1 => TrailOrientation::Frame,
        2 | 3 => TrailOrientation::World,
        4 => {
            notes.approximate("a crossed tracer (two strips) as one world-aligned strip");
            TrailOrientation::World
        }
        _ => {
            notes.lose("a tracer orientation mode the project does not name, taken as facing the camera");
            TrailOrientation::Camera
        }
    }
}

/// The first heading of an emitted particle (`InitialDirection`), before any spread.
pub fn heading(direction: i32, notes: &mut Notes) -> Heading {
    match direction {
        0 => Heading::Shape { facing: FORWARD },
        1 => Heading::World([0.0, 1.0, 0.0]),
        2 => Heading::World([0.0, -1.0, 0.0]),
        3 => Heading::World(FORWARD),
        4 => Heading::Frame([0.0, 1.0, 0.0]),
        5 => Heading::Frame([0.0, -1.0, 0.0]),
        6 => Heading::Frame(FORWARD),
        _ => {
            notes.lose("an initial direction the project does not name, taken as out of the shape");
            Heading::Shape { facing: FORWARD }
        }
    }
}

/// How a node or its particles hang from their parent (`AttachMode`): 0 not at all, 1 with the parent node, 2 with the
/// attachment point (the dummy polygon the effect was spawned at) and parent transformations ignored, 3 and 4 the same with
/// translation only.
pub fn attachment(mode: i32, notes: &mut Notes) -> Attachment {
    let (anchor, follow) = match mode {
        0 => (Anchor::Parent, Follow::None),
        1 => (Anchor::Parent, Follow::Frame),
        2 => (Anchor::System, Follow::Frame),
        3 => (Anchor::Parent, Follow::Position),
        4 => (Anchor::System, Follow::Position),
        _ => {
            notes.lose("an attachment mode the project does not name, taken as attached to the parent");
            (Anchor::Parent, Follow::Frame)
        }
    };
    Attachment { anchor, follow }
}

/// The distance limits of an appearance (`minFadeDistance`, `minDistance`, `maxFadeDistance`, `maxDistance` and the two hard
/// thresholds; -1 is no limit). A particle nearer than `min_distance` is hidden and its opacity reaches 1 at
/// `min_fade_distance`; farther than `max_distance` hidden, opacity 1 up to `max_fade_distance`.
pub fn distance_fade(
    min_fade: f32,
    min: f32,
    max_fade: f32,
    max: f32,
    cut_min: f32,
    cut_max: f32,
) -> DistanceFade {
    let on = |v: f32| v >= 0.0;
    DistanceFade {
        near: (on(min) && on(min_fade)).then_some([min, min_fade]),
        far: (on(max) && on(max_fade)).then_some([max_fade, max]),
        cut_near: on(cut_min).then_some(cut_min),
        cut_far: on(cut_max).then_some(cut_max),
    }
}

/// The limits as the draws that may have none state them: `None` where nothing is limited.
pub fn limit(distance: DistanceFade) -> Option<DistanceFade> {
    (!distance.is_none()).then_some(distance)
}

/// Whether a lighting mode lights the particle (`LightingMode`: -1 unlit, 0 lit, -2 lit with an extra source; the project says
/// values it does not name occur).
pub fn lit(mode: i32, notes: &mut Notes) -> bool {
    match mode {
        -1 => false,
        0 | -2 => true,
        _ => {
            notes.approximate("a lighting mode the project does not name taken as lit");
            true
        }
    }
}

/// The depth over which a soft particle fades. The files' depth-blend switch has two unnamed distances beside it that this
/// does not read; core's default stands, counted.
pub const SOFT_DISTANCE: f32 = 0.5;

/// A turn interval the files store as a whole number of 1/50 s, in seconds.
pub fn turn_interval(stored: i32) -> f32 {
    stored.max(0) as f32 / 50.0
}

/// Frames at 30 per second, in seconds.
pub fn frames(stored: i32) -> f32 {
    stored as f32 / 30.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerations_map_and_unnamed_values_are_counted() {
        let mut notes = Notes::default();
        assert_eq!(blend(2, &mut notes), Blend::Alpha);
        assert_eq!(blend(4, &mut notes), Blend::Additive);
        assert!(notes.lost.is_empty() && notes.approximated.is_empty());
        assert_eq!(blend(7, &mut notes), Blend::Additive);
        assert_eq!(notes.approximated.values().sum::<u32>(), 1);
        assert_eq!(blend(99, &mut notes), Blend::Alpha);
        assert_eq!(notes.lost.values().sum::<u32>(), 1);
        assert_eq!(heading(3, &mut notes), Heading::World([0.0, 0.0, -1.0]));
        assert_eq!(heading(4, &mut notes), Heading::Frame([0.0, 1.0, 0.0]));
        assert_eq!(orientation(1, &mut notes), Orientation::CameraPlane);
        assert_eq!(orientation(7, &mut notes), Orientation::FrameYaw);
    }

    #[test]
    fn attachments_and_distances_follow_the_projects_reading() {
        let mut notes = Notes::default();
        let a = attachment(3, &mut notes);
        assert_eq!((a.anchor, a.follow), (Anchor::Parent, Follow::Position));
        let a = attachment(2, &mut notes);
        assert_eq!((a.anchor, a.follow), (Anchor::System, Follow::Frame));
        assert!(distance_fade(-1.0, -1.0, -1.0, -1.0, -1.0, -1.0).is_none());
        let d = distance_fade(2.0, 0.5, 30.0, 40.0, -1.0, 100.0);
        assert_eq!(d.near, Some([0.5, 2.0]));
        assert_eq!(d.far, Some([30.0, 40.0]));
        assert_eq!((d.cut_near, d.cut_far), (None, Some(100.0)));
    }

    #[test]
    fn model_particles_turned_along_their_travel_name_the_axis_the_game_turns_along_it() {
        // the captured frames: a Model's +Y and a RichModel's +Z (the world frame's -Z) lie along the flight
        let mut notes = Notes::default();
        assert_eq!(
            model_orientation(2, &mut notes),
            MeshOrientation::Heading {
                forward: [0.0, 1.0, 0.0]
            }
        );
        assert_eq!(
            rich_model_orientation(2, &mut notes),
            MeshOrientation::Heading {
                forward: [0.0, 0.0, -1.0]
            }
        );
        assert!(notes.lost.is_empty() && notes.approximated.is_empty());
    }

    #[test]
    fn intervals_and_frames_convert() {
        assert_eq!(turn_interval(25), 0.5);
        assert_eq!(turn_interval(-3), 0.0);
        assert_eq!(frames(45), 1.5);
    }
}
