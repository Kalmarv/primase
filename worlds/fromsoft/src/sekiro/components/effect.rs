//! What the effect read keeps for `lower-fromsoft-effects`: the effect files as stored (`fromsoft.effect_file`), the places
//! effects stand (`fromsoft.effect`) and the files effects name by number (`fromsoft.effect_resource`). The read selects and
//! places; the pass interprets.
use super::character::ClipEvents as SourceEvents;
use super::source::EffectPlace;
use fromsoft_formats::Game;
use fromsoft_formats::fxr::Fxr;
use fromsoft_formats::fxr::resources::TextureKind;
use primase_core::json::{JsonError, to_value};
use primase_core::{Component, EventValue, EventValueData, Geometry};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// An effect file of the install, as stored, on an entity of its own (not placed): the lowering of an effect and of the
/// effects it spawns finds its files here by their numbers. `game` is the layout the file's actions are read in.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectFile {
    pub effect: i32,
    pub game: Game,
    pub file: Arc<Fxr>,
}
impl Component for EffectFile {
    const NAME: &'static str = "fromsoft.effect_file";
    const GEOMETRY: Geometry = Geometry::None;
    /// The effect's number and the sizes of its tree, not the tree.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "effect": self.effect,
            "game": self.game,
            "actions": self.file.actions().len(),
            "states": self.file.states.len(),
        })))
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}

/// An effect standing where an entity's transform places it (`fromsoft.effect`): which effect, and the route that placed it.
/// `starts_disabled` is an SFX region's flag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Effect {
    pub effect: i32,
    pub place: EffectPlace,
    pub starts_disabled: bool,
}
impl Component for Effect {
    const NAME: &'static str = "fromsoft.effect";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// What an effect names by number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ResourceKind {
    /// A colour texture.
    Albedo,
    /// A normal map.
    Normal,
    /// A specular parameter map.
    Specular,
    /// A detail map.
    Detail,
    /// A mesh-particle model.
    Model,
    /// A model animation.
    Animation,
}

impl ResourceKind {
    /// The texture kind of a texture resource.
    pub fn texture(self) -> Option<TextureKind> {
        match self {
            ResourceKind::Albedo => Some(TextureKind::Albedo),
            ResourceKind::Normal => Some(TextureKind::Normal),
            ResourceKind::Specular => Some(TextureKind::Specular),
            ResourceKind::Detail => Some(TextureKind::Detail),
            ResourceKind::Model | ResourceKind::Animation => None,
        }
    }

    /// The resource kind of a texture kind.
    pub fn of_texture(kind: TextureKind) -> ResourceKind {
        match kind {
            TextureKind::Albedo => ResourceKind::Albedo,
            TextureKind::Normal => ResourceKind::Normal,
            TextureKind::Specular => ResourceKind::Specular,
            TextureKind::Detail => ResourceKind::Detail,
        }
    }
}

/// A file effects name by number (`fromsoft.effect_resource`), on the entity that holds it (a `core.texture` for a texture):
/// its kind and number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct EffectResource {
    pub kind: ResourceKind,
    pub id: i32,
}
impl Component for EffectResource {
    const NAME: &'static str = "fromsoft.effect_resource";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// An effect an animation event starts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectUse {
    /// The event's index in the clip's events.
    pub event: u32,
    /// The event's type number.
    pub kind: i64,
    pub effect: i32,
    /// The reference id of the dummy polygon the effect stands at, where the event names one.
    pub dummy: Option<i32>,
    /// The event's `Dummy Poly Source`, where it has one: 0 the character's own dummy polygons (a weapon's are among them
    /// as `10000 + n` for the right hand and `20000 + n` for the left), 1 the left hand's weapon, 2 the right hand's.
    pub source: Option<i32>,
    /// Whether the effect follows the dummy polygon as the animation moves it.
    pub follow: bool,
    /// Whether the event keeps the effect going for its length (else the effect plays out).
    pub repeating: bool,
    /// Seconds into the animation.
    pub start: f32,
    /// The event's end, seconds into the animation, where it has one of its own.
    pub end: Option<f32>,
}

/// The effects the events of a clip start (`fromsoft.clip_effect_uses`, on the clip): the read keeps the effects' files, this
/// says which events start them and how, and `lower-fromsoft-effects` makes of it what a player can follow
/// (`core.clip_effects`). `lost` counts, by reason, the events that name an effect this could not say.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipEffectUses {
    pub uses: Vec<EffectUse>,
    pub lost: Vec<(String, u32)>,
}
impl Component for ClipEffectUses {
    const NAME: &'static str = "fromsoft.clip_effect_uses";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// The event types whose first field is an effect number in the stock files: of the events that name a positive number, type
/// 96's name an effect that exists in all but 3 of 7,586 in Sekiro and 16,885 of 20,078 in Elden Ring (the rest are the DLC's),
/// type 118's 182 of 182 and 2,692 of 2,723, type 120's 1,472 of 1,473 in Elden Ring.
const EFFECT_NUMBERS: &[i32] = &[96, 118, 120];

/// The event types the community template gives an `FFX ID` field that the stock files do not show to hold effect numbers: type
/// 112's field holds one of 6 (Sekiro) or 15 (Elden Ring) small numbers, 110's the numbers 1 to 10, 119's (Elden Ring) is an
/// effect that no binder holds in 247 of 249, 116's (Elden Ring, 6 events) names 3 that exist; the others (95, 99, 100, 101, 104,
/// 108, 109, 121, 122, 961) hold no number above 0 in either game. They are counted, not used.
const UNPROVEN: &[i32] = &[95, 99, 100, 101, 104, 108, 109, 110, 112, 116, 119, 121, 122, 961];

/// The event types that name an effect some way, for an event whose fields were not read.
const EFFECT_EVENTS: &[i32] = &[96, 118, 120, 114, 115, 117, 792];

fn field<'a>(values: &'a [EventValue], name: &str) -> Option<&'a EventValueData> {
    values.iter().find(|v| v.name == name).map(|v| &v.value)
}

fn whole(values: &[EventValue], name: &str) -> Option<i64> {
    match field(values, name)? {
        EventValueData::I64(v) => Some(*v),
        _ => None,
    }
}

fn flag(values: &[EventValue], name: &str) -> bool {
    matches!(field(values, name), Some(EventValueData::Bool(true)))
}

/// The effects the events of `source` start, by the community template's field names: an event of a type whose first field is
/// an effect number ([`EFFECT_NUMBERS`]) names the effect directly, at the dummy polygon of its `Dummy Poly ID` (a blade's
/// `Dummy Poly - Blade Base ID`), following it when `Follow Dummy Poly` is set. What names an effect another way, or does not
/// show it does, is counted, not guessed: the goods and magic events (the effect is a row of a param), the throw and foot
/// events (an index into one), the types the template names an effect in that the stock files do not bear out ([`UNPROVEN`]),
/// and an effect on a weapon's dummy polygon (the weapon's model is not in the read).
pub fn clip_effect_uses(source: &SourceEvents) -> ClipEffectUses {
    let mut uses = Vec::new();
    let mut lost: BTreeMap<&'static str, u32> = BTreeMap::new();
    for (index, event) in source.events.iter().enumerate() {
        let Some(typed) = source.typed.get(index) else {
            if EFFECT_EVENTS.contains(&event.kind) {
                *lost
                    .entry("events of an effect type whose fields were not read")
                    .or_default() += 1;
            }
            continue;
        };
        let values = typed.values.as_slice();
        if matches!(event.kind, 114 | 115) {
            *lost
                .entry("an effect a goods or magic param row names (events 114 and 115)")
                .or_default() += 1;
            continue;
        }
        if matches!(event.kind, 117 | 792) {
            *lost
                .entry("an effect a throw direction or foot index names (events 117 and 792)")
                .or_default() += 1;
            continue;
        }
        let Some(named) = whole(values, "FFX ID").or_else(|| whole(values, "FFX ID: For All")) else {
            continue;
        };
        if named <= 0 {
            continue;
        }
        if UNPROVEN.contains(&event.kind) {
            *lost
                .entry("events of types the stock files do not show to hold effect numbers (95, 99 to 101, 104, 108 to 110, 112, 116, 119, 121, 122, 961)")
                .or_default() += 1;
            continue;
        }
        if !EFFECT_NUMBERS.contains(&event.kind) {
            continue;
        }
        let Ok(effect) = i32::try_from(named) else {
            *lost.entry("an effect number past 32 bits").or_default() += 1;
            continue;
        };
        if whole(values, "Dummy Poly Source").is_some_and(|from| from != 0) {
            *lost
                .entry("an effect on a weapon's dummy polygon (the weapon's model is not read)")
                .or_default() += 1;
            continue;
        }
        let dummy = whole(values, "Dummy Poly ID")
            .or_else(|| whole(values, "Dummy Poly - Blade Base ID"))
            .and_then(|id| i32::try_from(id).ok());
        let kind = i64::from(event.kind);
        uses.push(EffectUse {
            event: u32::try_from(index).unwrap_or(u32::MAX),
            kind,
            effect,
            dummy,
            source: whole(values, "Dummy Poly Source").and_then(|from| i32::try_from(from).ok()),
            follow: flag(values, "Follow Dummy Poly"),
            repeating: flag(values, "Is Repeat"),
            start: event.start,
            end: (!event.open_ended()).then_some(event.end),
        });
    }
    ClipEffectUses {
        uses,
        lost: lost
            .into_iter()
            .map(|(reason, n)| (reason.to_owned(), n))
            .collect(),
    }
}

/// What Elden Ring adds to the effect number of a blade event (type 118) before it starts the effect: the number the character's
/// data keeps for the hand the blade is in. The engine's handler of the event reads it at the character's data (`+0x1e8` for the
/// right hand, `+0x1f0` for the left) and adds it to the event's number; the stock events name `401001`, `401002`, `405000`,
/// `411002`..., and the effect files `401004` to `401007`, `401010` to `401012`, `401020`, `401021`, `401030`... are numbers
/// those plus an offset reach. Where the offset comes from is not decoded: the special-effect param's `traceSfxIdOffsetType` is
/// the field the community's param documentation names for the blade trail's offset (1 to 6, 9 to 12, 14 to 21 and 24 to 28 in the
/// stock params; the plain sum gives a file for `401001` plus 1, 3 to 6, 9 to 11, 19 and 20 but not plus 2), so the number is
/// stated, not derived. With no special effect on the weapon, both are 0 (the default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceOffsets {
    /// For a blade in the right hand (`Dummy Poly Source` 2, or a right-hand weapon dummy polygon `10000 + n`).
    pub right: i32,
    /// For a blade in the left hand (`Dummy Poly Source` 1, or a left-hand weapon dummy polygon `20000 + n`).
    pub left: i32,
}

/// Adds the hand's offset of `offsets` to the number of each blade event (type 118) of `uses`, as Elden Ring's handler does:
/// the right hand's for `Dummy Poly Source` 2, the left's for 1. For source 0 the handler asks the character which hand the
/// action is in (a runtime state); here the weapon dummy polygon block stands for it, `10000..20000` the right and
/// `20000..30000` the left, and a blade on any other dummy polygon gets no offset. Events of other types keep their number.
pub fn apply_trace_offsets(uses: &mut [EffectUse], offsets: TraceOffsets) {
    for u in uses.iter_mut().filter(|u| u.kind == 118) {
        let hand = match u.source {
            Some(2) => Some(offsets.right),
            Some(1) => Some(offsets.left),
            _ => match u.dummy {
                Some(10_000..20_000) => Some(offsets.right),
                Some(20_000..30_000) => Some(offsets.left),
                _ => None,
            },
        };
        if let Some(offset) = hand {
            u.effect = u.effect.saturating_add(offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::character::DecodedEvent;
    use super::*;
    use fromsoft_formats::tae::{Event, Header};

    fn value(name: &str, v: EventValueData) -> EventValue {
        EventValue {
            name: name.into(),
            value: v,
        }
    }

    /// An event of `kind` from `start` to `end` with the fields given.
    fn event(kind: i32, start: f32, end: f32, values: Vec<EventValue>) -> (Event, DecodedEvent) {
        (
            Event {
                kind,
                unk04: 0,
                start,
                end,
                parameters: Vec::new(),
            },
            DecodedEvent {
                name: None,
                naming: None,
                values,
                refusals: Vec::new(),
            },
        )
    }

    fn source(events: Vec<(Event, DecodedEvent)>) -> SourceEvents {
        let (events, typed): (Vec<_>, Vec<_>) = events.into_iter().unzip();
        SourceEvents {
            file: "c1010.tae".into(),
            animation: 3000,
            header: Header::None,
            file_name: None,
            authored_events: events.clone(),
            events,
            typed,
        }
    }

    /// The fields of the community template's one-shot effect event (type 96) and its relatives.
    fn spawn(effect: i64, dummy: i64, follow: bool) -> Vec<EventValue> {
        vec![
            value("FFX ID", EventValueData::I64(effect)),
            value("Dummy Poly ID", EventValueData::I64(dummy)),
            value("Slot ID", EventValueData::I64(0)),
            value("Ignore Dummy Poly Angle", EventValueData::Bool(false)),
            value("Follow Dummy Poly", EventValueData::Bool(follow)),
        ]
    }

    #[test]
    fn an_event_with_an_effect_field_is_a_use_at_its_dummy_and_the_rest_are_counted_or_passed() {
        let blade = vec![
            value("FFX ID", EventValueData::I64(5000)),
            value("Dummy Poly Source", EventValueData::I64(0)),
            value("Dummy Poly - Blade Base ID", EventValueData::I64(60)),
            value("Dummy Poly - Blade Tip ID", EventValueData::I64(61)),
        ];
        let on_weapon = vec![
            value("FFX ID", EventValueData::I64(5001)),
            value("Dummy Poly Source", EventValueData::I64(2)),
            value("Dummy Poly - Blade Base ID", EventValueData::I64(60)),
        ];
        let mut repeating = spawn(4001, 10, true);
        repeating.push(value("Is Repeat", EventValueData::Bool(true)));
        let floor = vec![
            value("FFX ID", EventValueData::I64(1)),
            value("Dummy Poly ID", EventValueData::I64(5)),
        ];
        let used = source(vec![
            event(96, 0.5, 0.6, spawn(4000, 220, true)),
            event(118, 0.7, 0.9, blade),
            event(118, 0.7, 0.9, on_weapon),
            event(96, 1.0, 1.1, spawn(-1, 220, false)),
            event(
                114,
                1.1,
                1.2,
                vec![value("FFX ID (Deprecated)", EventValueData::I64(0))],
            ),
            event(
                117,
                1.2,
                1.3,
                vec![value("Throw Direction SFX Index", EventValueData::I64(2))],
            ),
            event(96, 1.4, f32::MAX, repeating),
            event(112, 1.5, 1.6, floor),
            event(
                67,
                1.7,
                1.8,
                vec![value("SpEffect ID", EventValueData::I64(9000))],
            ),
        ]);
        let found = clip_effect_uses(&used);
        let seen: Vec<_> = found
            .uses
            .iter()
            .map(|u| (u.event, u.effect, u.dummy, u.follow, u.repeating))
            .collect();
        assert_eq!(
            seen,
            [
                (0, 4000, Some(220), true, false),
                (1, 5000, Some(60), false, false),
                (6, 4001, Some(10), true, true)
            ]
        );
        assert_eq!((found.uses[0].start, found.uses[0].end), (0.5, Some(0.6)));
        // an event with no end of its own has none here
        assert_eq!(found.uses[2].end, None);
        // the weapon's dummy polygon, the goods and magic row, the throw index and the type the files do not bear out
        assert_eq!(found.lost.len(), 4);
        assert!(found.lost.iter().all(|(_, n)| *n == 1));
    }

    #[test]
    fn a_blade_events_number_gets_the_offset_of_its_hand_and_other_events_keep_theirs() {
        let blade = |effect: i32, dummy: i32, source: Option<i32>| EffectUse {
            event: 0,
            kind: 118,
            effect,
            dummy: Some(dummy),
            source,
            follow: false,
            repeating: false,
            start: 0.0,
            end: None,
        };
        let mut uses = vec![
            blade(401_001, 10_300, Some(0)),
            blade(401_001, 20_300, Some(0)),
            blade(401_002, 300, Some(2)),
            blade(401_002, 300, Some(1)),
            blade(401_001, 60, Some(0)),
            EffectUse {
                kind: 96,
                ..blade(430, 10_300, Some(0))
            },
        ];
        apply_trace_offsets(&mut uses, TraceOffsets { right: 10, left: 20 });
        let numbers: Vec<i32> = uses.iter().map(|u| u.effect).collect();
        assert_eq!(numbers, [401_011, 401_021, 401_012, 401_022, 401_001, 430]);
        apply_trace_offsets(&mut uses, TraceOffsets::default());
        assert_eq!(uses.iter().map(|u| u.effect).collect::<Vec<_>>(), numbers);
    }

    #[test]
    fn events_whose_fields_were_not_read_are_counted_when_their_type_names_an_effect() {
        let mut unread = source(vec![
            event(96, 0.0, 0.1, Vec::new()),
            event(67, 0.0, 0.1, Vec::new()),
        ]);
        unread.typed.clear();
        let found = clip_effect_uses(&unread);
        assert!(found.uses.is_empty());
        assert_eq!(
            found.lost,
            [(
                "events of an effect type whose fields were not read".to_owned(),
                1
            )]
        );
    }

    #[test]
    fn resource_kinds_round_trip_through_texture_kinds() {
        for kind in [
            TextureKind::Albedo,
            TextureKind::Normal,
            TextureKind::Specular,
            TextureKind::Detail,
        ] {
            assert_eq!(ResourceKind::of_texture(kind).texture(), Some(kind));
        }
        assert_eq!(ResourceKind::Model.texture(), None);
    }
}
