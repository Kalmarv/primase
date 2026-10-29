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
use std::collections::{BTreeMap, BTreeSet};
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

/// What starts an effect a clip plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum UseRoute {
    /// The event names the effect (types 96, 118 and 120: its first field is the effect's number).
    Event,
    /// The trail of a held weapon: its `EquipParamWeapon` row's trace slot `slot` (`traceSfxId<n>` between the weapon's
    /// dummy polygons `traceDmyIdHead<n>` and `traceDmyIdTail<n>`), drawn while an attack event (type 1, the attack's
    /// window) lasts. No event of the clip names the effect.
    WeaponTrace { row: i32, slot: u8 },
}

/// An effect an animation event starts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectUse {
    /// The event's index in the clip's events.
    pub event: u32,
    /// The event's type number.
    pub kind: i64,
    pub effect: i32,
    /// What names the effect: the event, or a held weapon's row during the event.
    pub route: UseRoute,
    /// The reference id of the dummy polygon the effect stands at, where the event names one.
    pub dummy: Option<i32>,
    /// The further dummy polygons an effect runs through after `dummy`, in order: a blade event's tip, or the rest of a
    /// weapon trace's run ([`trace_run`]). Empty for an effect at one polygon.
    pub span: Vec<i32>,
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
/// (`core.clip_effects`). `lost` counts, by reason, the events that name an effect this could not say. `read` holds the
/// entities the read made for the uses (their effect files, the effects those spawn and the textures they name): the uses
/// name effects by number, so these are what keeps the files with the clip where only the clip is taken (a subset of a world
/// by the clip's closure, as a reader's load of one clip makes).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipEffectUses {
    pub uses: Vec<EffectUse>,
    pub lost: Vec<(String, u32)>,
    pub read: Vec<primase_core::Entity>,
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
    fn references(&self, out: &mut Vec<primase_core::Entity>) {
        out.extend(self.read.iter().copied());
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

/// What an event whose `Additional Spawn Condition` is not 0 is counted as. The condition is a runtime state the read does
/// not have (the template names the byte and no values): of Sekiro's player events of type 96, 36 of 2,532 hold one (2,
/// 26 of them effect 600030 at the katana's tip), and the stock game draws no 600030 in the Wolf's first katana attack
/// (`a050_300000`, filmed mod-free), so such an effect is left out, not played as if its condition held.
pub const SPAWN_CONDITION: &str =
    "effects whose event names an additional spawn condition (a runtime state not read)";

/// What an event whose `State Info` is not 0 is counted as: the field names a special-effect type (the template's
/// `SP_EFFECT_TYPE`), and the event applies only while the character has a special effect of that type, a runtime state the
/// read does not have. Alternatives sit side by side this way (Sekiro's `a050_300020` starts 404000, 404010 and 404020 at
/// one dummy polygon for one window, under states 153, 152 and 357), so playing them all would be wrong.
pub const STATE_CONDITION: &str =
    "effects whose event applies under a special-effect state (State Info, a runtime state not read)";

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
        if whole(values, "Additional Spawn Condition").is_some_and(|c| c != 0) {
            *lost.entry(SPAWN_CONDITION).or_default() += 1;
            continue;
        }
        if whole(values, "State Info").is_some_and(|s| s != 0) {
            *lost.entry(STATE_CONDITION).or_default() += 1;
            continue;
        }
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
            route: UseRoute::Event,
            dummy,
            span: whole(values, "Dummy Poly - Blade Tip ID")
                .filter(|&id| id > 0)
                .and_then(|id| i32::try_from(id).ok())
                .into_iter()
                .collect(),
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
        read: Vec::new(),
    }
}

/// One trace slot of a weapon row: an effect and the weapon's dummy polygons it runs through, from `traceDmyIdHead<n>` to
/// `traceDmyIdTail<n>`, as stored (-1 for none; [`trace_run`] reads them as the game does).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Trace {
    pub effect: i32,
    pub head: i32,
    pub tail: i32,
}

/// The trace slots of a held weapon: its hand slot, its `EquipParamWeapon` row, the row's slots that name an effect, by
/// slot number, and the reference ids of the dummy polygons of the weapon's model (its first, the blade's), which a
/// trace's run walks ([`trace_run`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponTraces {
    pub slot: HandSlot,
    pub row: i32,
    pub traces: Vec<(usize, Trace)>,
    pub dummies: BTreeSet<i32>,
}

/// The dummy polygons of `dummies` (a weapon model's reference ids) a trace runs through, in order, as Elden Ring's trace
/// slot takes them (its executable, decompiled), comparing the ids as unsigned numbers: none where head and tail are both
/// -1; from the head up to the tail where the tail is not -1 and above the head; from the head up, unbounded, where the
/// tail is -1; none otherwise (a tail at or below the head, so a tail of 0 and a head of -1 give none). The walk stops at
/// the first id the model lacks, and the game makes no trail of a run of fewer than two. Sekiro's rows all name a tail
/// above the head (300, 301), which this reads the same way; a head numbered as the character numbers a weapon's
/// polygons (`10000 + n`) is no id of the weapon's model and finds none.
pub fn trace_run(trace: &Trace, dummies: &BTreeSet<i32>) -> Vec<i32> {
    // the ids' bits, as the game compares them
    let (head, tail) = (trace.head as u32, trace.tail as u32);
    let (first, end) = match (head, tail) {
        (u32::MAX, u32::MAX) => return Vec::new(),
        (_, u32::MAX) => (head, u32::MAX),
        (_, _) if head < tail => (head, tail + 1),
        _ => return Vec::new(),
    };
    (first..end)
        .map(|id| id as i32)
        .take_while(|id| dummies.contains(id))
        .collect()
}

/// What an attack whose weapon trace runs through fewer than two of the weapon model's dummy polygons is counted as.
pub const SHORT_TRACE: &str = "attack events whose weapon trace finds fewer than two of the weapon model's dummy polygons (the game draws no trail)";

/// The trails of held weapons a clip draws: for each attack event (type 1, whose length is the attack's window) a use of the
/// first trace slot of the weapon in the right hand's first slot (else the first weapon held), running through the trace's
/// run of the weapon's dummy polygons ([`trace_run`], named by their global ids, [`HandSlot::global_id`]) and following
/// them for the event's length; a run of fewer than two is counted ([`SHORT_TRACE`]), as the game draws none. Which
/// weapon and which slot an attack draws is not read (the attack's behaviour row names it): the weapons' other slots are
/// counted in `lost`, as an attack event with no held weapon that has one is.
pub fn weapon_trace_uses(source: &SourceEvents, weapons: &[WeaponTraces], found: &mut ClipEffectUses) {
    let held = weapons
        .iter()
        .find(|w| !w.slot.left && w.slot.index == 0)
        .or_else(|| weapons.first())
        .and_then(|w| Some((w, w.traces.first()?)));
    let mut attacks = 0u32;
    let mut conditioned = 0u32;
    let mut short = 0u32;
    for (index, event) in source.events.iter().enumerate().filter(|(_, e)| e.kind == 1) {
        // an attack under a special-effect state is a runtime alternative (see STATE_CONDITION)
        let state = source
            .typed
            .get(index)
            .and_then(|t| whole(&t.values, "State Info"))
            .unwrap_or(0);
        if state != 0 {
            conditioned += 1;
            continue;
        }
        attacks += 1;
        let Some((w, &(slot, trace))) = held else {
            continue;
        };
        let run = trace_run(&trace, &w.dummies);
        if run.len() < 2 {
            short += 1;
            continue;
        }
        found.uses.push(EffectUse {
            event: u32::try_from(index).unwrap_or(u32::MAX),
            kind: 1,
            effect: trace.effect,
            route: UseRoute::WeaponTrace {
                row: w.row,
                slot: u8::try_from(slot).unwrap_or(u8::MAX),
            },
            dummy: Some(w.slot.global_id(run[0])),
            span: run[1..].iter().map(|&id| w.slot.global_id(id)).collect(),
            source: None,
            follow: true,
            repeating: true,
            start: event.start,
            end: (!event.open_ended()).then_some(event.end),
        });
    }
    let mut lost = |reason: &str, n: u32| {
        if n > 0 {
            found.lost.push((reason.to_owned(), n));
        }
    };
    if held.is_some() {
        lost(
            "attack events under a special-effect state (State Info, a runtime state not read): no trail drawn",
            conditioned,
        );
        lost(SHORT_TRACE, short);
    }
    if attacks == 0 {
        return;
    }
    match held {
        None => lost(
            "attack events with no held weapon whose row names a trace (no trail drawn)",
            attacks,
        ),
        Some((w, _)) => {
            let others: usize = weapons.iter().map(|o| o.traces.len()).sum::<usize>() - 1;
            if others > 0 {
                lost(
                    &format!(
                        "trace slots of the held weapons other than row {}'s first (which an attack draws is not read)",
                        w.row
                    ),
                    u32::try_from(others).unwrap_or(u32::MAX),
                );
            }
        }
    }
}

/// Where a held weapon is: the hand and which of the four weapons that hand holds (the engine numbers them 1 to 4 here, 0 to 3).
///
/// An animation event names a dummy polygon of a held weapon by a global id, the slot's base plus the polygon's number in the
/// weapon's own model: the right hand's weapons `10000`, `11000`, `12000` and `13000`, the left hand's `20000`, `21000`,
/// `22000` and `23000` (Elden Ring's engine keeps the bases in a table and adds `number % 1000`; the same table gives `1000` to
/// `4000` for four other kinds of polygon set, which no stock event names). :observed[the engine's mapping function and its table]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct HandSlot {
    pub left: bool,
    /// 0 to 3.
    pub index: u8,
}

impl HandSlot {
    /// The first of the global ids of the slot's polygons.
    pub fn base(self) -> i32 {
        (if self.left { 20_000 } else { 10_000 }) + 1_000 * i32::from(self.index.min(3))
    }

    /// The global id of the polygon `local` of the model held in this slot.
    pub fn global_id(self, local: i32) -> i32 {
        self.base() + local.rem_euclid(1_000)
    }

    /// The slot and the model's own polygon number a global id names, where it names a weapon's polygon.
    pub fn of_global_id(id: i32) -> Option<(HandSlot, i32)> {
        let (left, rest) = match id {
            10_000..14_000 => (false, id - 10_000),
            20_000..24_000 => (true, id - 20_000),
            _ => return None,
        };
        Some((
            HandSlot {
                left,
                index: u8::try_from(rest / 1_000).ok()?,
            },
            rest % 1_000,
        ))
    }
}

/// The weapons a character holds, by hand slot: each a row of the install's `EquipParamWeapon` (Sekiro's Kusabimaru
/// `8000000`, Elden Ring's Uchigatana `9000000`), the equipment the game keeps for the player. Which weapons a character
/// holds is the player's state, not an animation's or a param's: it is stated. The read reads each row's part
/// (`parts/wp_a_<equipModelId>`) and where its models stand ([`HeldWeapon`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EquippedWeapons {
    pub right: [Option<i32>; 4],
    pub left: [Option<i32>; 4],
}

impl EquippedWeapons {
    /// The weapon row `row` in the right hand's first slot only (the usual one-weapon scene).
    pub fn right_hand(row: i32) -> Self {
        let mut w = Self::default();
        w.right[0] = Some(row);
        w
    }

    /// Every weapon stated, with its slot, in slot order (right hand first).
    pub fn slots(&self) -> Vec<(HandSlot, i32)> {
        let mut out = Vec::new();
        for (left, list) in [(false, &self.right), (true, &self.left)] {
            for (index, row) in list.iter().enumerate() {
                if let Some(row) = row {
                    out.push((
                        HandSlot {
                            left,
                            index: index as u8,
                        },
                        *row,
                    ));
                }
            }
        }
        out
    }

    /// Whether no weapon is stated.
    pub fn is_empty(&self) -> bool {
        self.slots().is_empty()
    }
}

/// Where one model of a weapon stands on the character in each way of holding it: the character's dummy polygon (its
/// reference id), from the weapon's `WepAbsorpPosParam` row (`right_<n>`, `left_<n>`, `both_<n>`, `leftHang_<n>`,
/// `rightHang_<n>`, `leftBoth_<n>` for model `n`); `None` where the row names none (-1 or 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Holds {
    /// Held in the right hand.
    pub right: Option<i32>,
    /// Held in the left hand.
    pub left: Option<i32>,
    /// The right hand's weapon held with both hands.
    pub both: Option<i32>,
    /// The left hand's weapon hung while the right's is held with both hands.
    pub left_hang: Option<i32>,
    /// The right hand's weapon hung while the left's is held with both hands.
    pub right_hang: Option<i32>,
    /// The left hand's weapon held with both hands.
    pub left_both: Option<i32>,
}

/// A model of a weapon the character holds (`fromsoft.held_weapon`, on the root of the model's asset: a form of the
/// weapon's part): the hand slot, the weapon row, the model's number in the part (`wp_a_0300_1` is model 1) and where each
/// way of holding puts it. `hold-fromsoft-weapons` attaches it; `assemble-character{leave-out=...}` leaves it out of the
/// character's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct HeldWeapon {
    pub slot: HandSlot,
    pub row: i32,
    pub model: u8,
    pub holds: Holds,
}
impl Component for HeldWeapon {
    const NAME: &'static str = "fromsoft.held_weapon";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
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
    fn hand_slots_number_a_weapons_polygons_as_the_engine_does() {
        let right = HandSlot {
            left: false,
            index: 0,
        };
        let left2 = HandSlot { left: true, index: 1 };
        assert_eq!((right.global_id(300), left2.global_id(1_120)), (10_300, 21_120));
        assert_eq!(HandSlot::of_global_id(10_300), Some((right, 300)));
        assert_eq!(HandSlot::of_global_id(21_120), Some((left2, 120)));
        assert_eq!(HandSlot::of_global_id(300), None);
        assert_eq!(HandSlot::of_global_id(14_000), None);
        let weapons = EquippedWeapons::right_hand(8_000_000);
        assert_eq!(weapons.slots(), [(right, 8_000_000)]);
        assert!(!weapons.is_empty() && EquippedWeapons::default().is_empty());
    }

    /// An effect event with a spawn condition or under a special-effect state (runtime states not read) is counted, not used;
    /// one with neither is used.
    #[test]
    fn an_effect_with_a_spawn_condition_is_counted_not_used() {
        let condition = |c: i64| {
            let mut v = spawn(600030, 10301, true);
            v.push(value("Additional Spawn Condition", EventValueData::I64(c)));
            v
        };
        let mut stated = spawn(404_000, 10300, true);
        stated.push(value("State Info", EventValueData::I64(153)));
        let found = clip_effect_uses(&source(vec![
            event(96, 0.3, 0.6, condition(2)),
            event(96, 0.8, 0.9, condition(0)),
            event(96, 0.4, 0.5, stated),
        ]));
        assert_eq!(found.uses.len(), 1);
        assert_eq!((found.uses[0].start, found.uses[0].route), (0.8, UseRoute::Event));
        assert_eq!(
            found.lost,
            [(STATE_CONDITION.to_owned(), 1), (SPAWN_CONDITION.to_owned(), 1)]
        );
    }

    /// Each attack event (type 1) draws the trail of the weapon in the right hand's first slot, its row's first trace slot,
    /// at its head polygon by global id and for the event's length; the other slots, and attacks with no trace, are counted.
    #[test]
    fn attack_events_draw_the_held_weapons_trail_for_their_window() {
        let right = HandSlot {
            left: false,
            index: 0,
        };
        let trace = |effect, head, tail| Trace { effect, head, tail };
        let weapons = [WeaponTraces {
            slot: right,
            row: 8_000_000,
            traces: vec![(0, trace(401_000, 300, 301)), (3, trace(401_001, -1, -1))],
            dummies: BTreeSet::from([100, 300, 301, 302]),
        }];
        let state = |s: i64| vec![value("State Info", EventValueData::I64(s))];
        let clip = source(vec![
            event(1, 0.433, 0.533, state(0)),
            event(96, 0.5, 0.6, spawn(400, 80, true)),
            event(1, 1.1, 1.2, Vec::new()),
            // an attack under a special-effect state draws no trail, counted
            event(1, 1.1, 1.2, state(358)),
        ]);
        let mut found = clip_effect_uses(&clip);
        weapon_trace_uses(&clip, &weapons, &mut found);
        let traces: Vec<_> = found
            .uses
            .iter()
            .filter(|u| matches!(u.route, UseRoute::WeaponTrace { .. }))
            .map(|u| {
                (
                    u.event,
                    u.effect,
                    u.dummy,
                    u.span.clone(),
                    u.repeating,
                    u.start,
                    u.end,
                )
            })
            .collect();
        assert_eq!(
            traces,
            [
                (0, 401_000, Some(10_300), vec![10_301], true, 0.433, Some(0.533)),
                (2, 401_000, Some(10_300), vec![10_301], true, 1.1, Some(1.2)),
            ]
        );
        assert!(found.uses.iter().any(|u| u.route
            == UseRoute::WeaponTrace {
                row: 8_000_000,
                slot: 0
            }));
        assert!(
            found
                .lost
                .iter()
                .any(|(r, n)| r.contains("other than row 8000000's first") && *n == 1)
        );
        assert!(
            found
                .lost
                .iter()
                .any(|(r, n)| r.starts_with("attack events under a special-effect state") && *n == 1)
        );
        // no weapon held: the attacks are counted, nothing is drawn
        let mut none = clip_effect_uses(&clip);
        weapon_trace_uses(&clip, &[], &mut none);
        assert!(none.uses.iter().all(|u| u.route == UseRoute::Event));
        assert!(
            none.lost
                .iter()
                .any(|(r, n)| r.starts_with("attack events with no held weapon") && *n == 2)
        );
        // a weapon whose model lacks the trace's second polygon draws no trail: its attacks are counted
        let mut bare = weapons.clone();
        bare[0].dummies = BTreeSet::from([300]);
        let mut found = clip_effect_uses(&clip);
        weapon_trace_uses(&clip, &bare, &mut found);
        assert!(found.uses.iter().all(|u| u.route == UseRoute::Event));
        assert!(found.lost.iter().any(|(r, n)| r == SHORT_TRACE && *n == 2));
        // a clip with no attack is left as it is
        let quiet = source(vec![event(96, 0.5, 0.6, spawn(400, 80, true))]);
        let mut found = clip_effect_uses(&quiet);
        let before = found.clone();
        weapon_trace_uses(&quiet, &weapons, &mut found);
        assert_eq!(found, before);
    }

    /// A trace runs from its head up to its tail, or up while the model has the next id where no tail is given; it stops at
    /// the first id the model lacks, and a tail at or below the head gives none (Elden Ring's trace slot).
    #[test]
    fn a_trace_runs_through_the_weapons_dummy_polygons_from_its_head() {
        let blade = BTreeSet::from([100, 120, 300, 301, 302, 303, 310]);
        let run = |head, tail| {
            trace_run(
                &Trace {
                    effect: 401_000,
                    head,
                    tail,
                },
                &blade,
            )
        };
        // the Uchigatana's (300, -1): the guard to the tip
        assert_eq!(run(300, -1), [300, 301, 302, 303]);
        // Sekiro's (300, 301), and a tail past the model's last id stops at it
        assert_eq!(run(300, 301), [300, 301]);
        assert_eq!(run(300, 305), [300, 301, 302, 303]);
        assert_eq!(run(310, -1), [310]);
        // a tail at or below the head, 0 among them, gives none, as a head of -1 does
        assert!(run(301, 300).is_empty());
        assert!(run(301, 301).is_empty());
        assert!(run(300, 0).is_empty());
        assert!(run(-1, 301).is_empty());
        assert!(run(-1, -1).is_empty());
        assert!(run(10_300, -1).is_empty());
    }

    #[test]
    fn a_blade_events_number_gets_the_offset_of_its_hand_and_other_events_keep_theirs() {
        let blade = |effect: i32, dummy: i32, source: Option<i32>| EffectUse {
            event: 0,
            kind: 118,
            effect,
            route: UseRoute::Event,
            dummy: Some(dummy),
            span: Vec::new(),
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
