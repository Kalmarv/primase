//! Read the lightweight scope candidates; choices and heavy payload loads are separate operations.
use super::area::{Options as ReaderOptions, Parents, StateClasses};
use super::{groups::DisplayGroups, sky::Sky, texture_switch::TextureQuality};
use crate::sekiro::area_plan::{AreaCandidates, AreaSelection};
use crate::sekiro::source::StateReadScope;
use crate::sekiro::{DrawParams, Region};
use fromsoft_formats::{Archive, Game, Result, dcx::Decoder, msb::MapLayout};
use primase_core::World;
use primase_core::pass::{Choice, Given, OptionSpec, Options, TextForm};

/// Every authored drawn-part point and candidate draw-param identity of the named area.
/// No region, GPARAM file or preview policy is selected here.
pub fn read_area_candidates(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    area: &str,
) -> Result<World> {
    let layout = MapLayout::read(&archive.read(&fromsoft_formats::paths::map_layout(area), decoder)?)?;
    let mut world = World::new();
    world.insert_resource(AreaCandidates {
        game,
        area: area.into(),
        drawn_points: fromsoft_formats::msb::drawn_part_points(&layout)?,
        draw_params: fromsoft_formats::area::draw_params(archive, game, area),
    });
    Ok(world)
}

impl AreaSelection {
    /// The region an area read keeps: the selected rectangle with the caller's `margin` (how far past it a placement's
    /// model may reach and still be kept); none where the selection reads the whole area.
    pub fn read_region(&self, margin: f32) -> Option<Region> {
        self.region.map(|rect| Region { rect, margin })
    }

    /// The draw-param environment an area read loads: the file the selection chose, or none.
    pub fn read_draw_params(&self) -> DrawParams {
        self.draw_param.clone().map_or(DrawParams::None, DrawParams::File)
    }
}

/// The choices of an area read that its registered `select-fromsoft-area` selection does not state, with the options
/// the read takes. Every field is the program's.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaRead {
    /// How far past the selected region a placement's model may reach and still be kept.
    pub margin: f32,
    /// Raw state data families to retain for `resolve-fromsoft-state`.
    pub state_inputs: StateReadScope,
    /// Which world states to bring in.
    pub states: StateClasses,
    /// Whether the parts' draw groups become presence axes.
    pub display_groups: DisplayGroups,
    /// Whether the larger maps above the map also place their parts that belong to it.
    pub parents: Parents,
    /// The collision part whose scene config names the draw-param file the area is lit with, in place of the file the
    /// selection chose (an area selected with no environment).
    pub environment_collision: Option<String>,
    /// Whether the animation binder of each kept object model is read.
    pub object_animations: bool,
    /// Whether the layout's regions and events of every type are read as `fromsoft.map_records`.
    pub map_records: bool,
    /// Whether each map collision file's partner in the `f` collision binder is read with it.
    pub f_collision: bool,
    /// Whether the point and spot lights of effect files are read where they stand.
    pub effect_lights: bool,
    /// Whether the sky is read with the area.
    pub sky: Sky,
    /// Whether the frame the game's renderer draws is read as the world's `core.render_graph`.
    pub render_graph: bool,
    /// Whether authored environment metadata is read.
    pub irradiance: bool,
    /// Whether the effects themselves are read where they stand.
    pub effects: bool,
    /// Whether each read texture's low-resolution twin is read too.
    pub low_textures: bool,
    /// The texture quality Elden Ring's switch is read at, with `low_textures`.
    pub texture_quality: TextureQuality,
}

impl AreaRead {
    /// The area reader's options: the selection's region (with the margin) and draw-param environment (the collision
    /// part's, where one is stated), and the choices stated here.
    pub fn options(&self, selection: &AreaSelection) -> ReaderOptions {
        ReaderOptions {
            state_inputs: self.state_inputs,
            region: selection.read_region(self.margin),
            states: self.states,
            display_groups: self.display_groups,
            parents: self.parents,
            draw_param: match &self.environment_collision {
                Some(collision) => DrawParams::Collision(collision.clone()),
                None => selection.read_draw_params(),
            },
            object_animations: self.object_animations,
            map_records: self.map_records,
            f_collision: self.f_collision,
            effect_lights: self.effect_lights,
            sky: self.sky,
            render_graph: self.render_graph,
            irradiance: self.irradiance,
            effects: self.effects,
            low_textures: self.low_textures,
            texture_quality: self.texture_quality,
        }
    }
}

/// An area read as a program states it in text (`read-fromsoft-area{area=m10_00_00_00,margin=30,...}`): the area,
/// where the text names one, and every choice of the read ([`AreaRead`]), none left to a default. A text that names no
/// area reads whichever area the program running it is given.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaReadText {
    /// The area read (`m10_00_00_00`), or none: the program running the text names it.
    pub area: Option<String>,
    pub read: AreaRead,
}

impl TextForm for AreaReadText {
    const NAME: &'static str = "read-fromsoft-area";
    const OPTIONS: &'static [OptionSpec] = &[
        OptionSpec::optional(
            "area",
            "m10_00_00_00",
            "the area read, as its layout is named",
            "the program running the text names the area",
        ),
        OptionSpec::required(
            "margin",
            "30",
            "metres, 0 or more: how far past the selected region a placement's model may reach and still be kept",
        ),
        OptionSpec::choice::<bool>(
            "state-weather",
            "whether every installed WeatherParam row is read, for a weather selection",
        ),
        OptionSpec::choice::<bool>(
            "state-common",
            "whether the common event program and every candidate draw-param file are read, for a state selection",
        ),
        OptionSpec::choice::<StateClasses>("states", "which world states are brought in"),
        OptionSpec::choice::<DisplayGroups>(
            "display-groups",
            "whether the parts' draw groups become presence axes",
        ),
        OptionSpec::choice::<Parents>(
            "parents",
            "whether the larger maps above the map also place their parts that belong to it",
        ),
        OptionSpec::required(
            "environment-collision",
            "none",
            "the collision part whose scene config names the draw-param file the area is lit with, in place of the file \
             the selection chose; none: the selection's file",
        ),
        OptionSpec::choice::<bool>(
            "object-animations",
            "whether the animation binder of each kept object model is read",
        ),
        OptionSpec::choice::<bool>(
            "map-records",
            "whether the layout's regions and events of every type are read as map records",
        ),
        OptionSpec::choice::<bool>(
            "f-collision",
            "whether each map collision file's partner in the f collision binder is read with it",
        ),
        OptionSpec::choice::<bool>(
            "effect-lights",
            "whether the point and spot lights of effect files are read where they stand",
        ),
        OptionSpec::choice::<Sky>("sky", "whether the sky is read with the area"),
        OptionSpec::choice::<bool>(
            "render-graph",
            "whether the frame the game's renderer draws is read as the world's render graph",
        ),
        OptionSpec::choice::<bool>("irradiance", "whether authored environment metadata is read"),
        OptionSpec::choice::<bool>(
            "effects",
            "whether the effects themselves are read where they stand",
        ),
        OptionSpec::choice::<bool>(
            "low-textures",
            "whether each read texture's low-resolution twin is read too",
        ),
        OptionSpec::choice::<TextureQuality>(
            "texture-quality",
            "the texture quality Elden Ring's switch is read at, with low-textures",
        ),
    ];

    fn make(given: &Given) -> std::result::Result<Self, String> {
        let area = match given.optional("area") {
            Some(area) if area.trim().is_empty() => {
                return Err("area is empty: name the area or leave it out".into());
            }
            area => area.map(str::to_owned),
        };
        let margin = given.required("margin");
        let margin = margin
            .parse::<f32>()
            .ok()
            .filter(|m| m.is_finite() && *m >= 0.)
            .ok_or_else(|| format!("margin={margin} is not a number of metres, 0 or more"))?;
        let environment_collision = match given.required("environment-collision") {
            "none" => None,
            "" => return Err("environment-collision is empty: name the collision part, or none".into()),
            part => Some(part.to_owned()),
        };
        Ok(AreaReadText {
            area,
            read: AreaRead {
                margin,
                state_inputs: StateReadScope {
                    weather: given.choice("state-weather"),
                    common: given.choice("state-common"),
                },
                states: given.choice("states"),
                display_groups: given.choice("display-groups"),
                parents: given.choice("parents"),
                environment_collision,
                object_animations: given.choice("object-animations"),
                map_records: given.choice("map-records"),
                f_collision: given.choice("f-collision"),
                effect_lights: given.choice("effect-lights"),
                sky: given.choice("sky"),
                render_graph: given.choice("render-graph"),
                irradiance: given.choice("irradiance"),
                effects: given.choice("effects"),
                low_textures: given.choice("low-textures"),
                texture_quality: given.choice("texture-quality"),
            },
        })
    }

    fn options(&self) -> Options {
        let read = &self.read;
        let mut options: Options = [
            ("margin", read.margin.to_string()),
            ("state-weather", read.state_inputs.weather.text().into()),
            ("state-common", read.state_inputs.common.text().into()),
            ("states", read.states.text().into()),
            ("display-groups", read.display_groups.text().into()),
            ("parents", read.parents.text().into()),
            (
                "environment-collision",
                read.environment_collision
                    .clone()
                    .unwrap_or_else(|| "none".into()),
            ),
            ("object-animations", read.object_animations.text().into()),
            ("map-records", read.map_records.text().into()),
            ("f-collision", read.f_collision.text().into()),
            ("effect-lights", read.effect_lights.text().into()),
            ("sky", read.sky.text().into()),
            ("render-graph", read.render_graph.text().into()),
            ("irradiance", read.irradiance.text().into()),
            ("effects", read.effects.text().into()),
            ("low-textures", read.low_textures.text().into()),
            ("texture-quality", read.texture_quality.text().into()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
        if let Some(area) = &self.area {
            options.insert("area".into(), area.clone());
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{parse_text_form, text_form, validate_text_form};

    /// Two reads that differ in every field, the second naming no area.
    fn reads() -> [AreaReadText; 2] {
        [
            AreaReadText {
                area: Some("m10_00_00_00".into()),
                read: AreaRead {
                    margin: 30.,
                    state_inputs: StateReadScope {
                        weather: false,
                        common: true,
                    },
                    states: StateClasses::None,
                    display_groups: DisplayGroups::Axes,
                    parents: Parents::None,
                    environment_collision: Some("h001000".into()),
                    object_animations: false,
                    map_records: true,
                    f_collision: false,
                    effect_lights: true,
                    sky: Sky::Include,
                    render_graph: false,
                    irradiance: true,
                    effects: false,
                    low_textures: true,
                    texture_quality: TextureQuality::Max,
                },
            },
            AreaReadText {
                area: None,
                read: AreaRead {
                    margin: 2.5,
                    state_inputs: StateReadScope {
                        weather: true,
                        common: false,
                    },
                    states: StateClasses::Event,
                    display_groups: DisplayGroups::None,
                    parents: Parents::Include,
                    environment_collision: None,
                    object_animations: true,
                    map_records: false,
                    f_collision: true,
                    effect_lights: false,
                    sky: Sky::None,
                    render_graph: true,
                    irradiance: false,
                    effects: true,
                    low_textures: false,
                    texture_quality: TextureQuality::Low,
                },
            },
        ]
    }

    /// The text reads back as the read it was printed from, every field, and the two reads print different values for
    /// every option (so no field is left out of the text).
    #[test]
    fn an_area_read_reads_back_from_its_text() {
        validate_text_form::<AreaReadText>();
        let [one, other] = reads();
        for read in [&one, &other] {
            let text = text_form(read);
            assert_eq!(&parse_text_form::<AreaReadText>(&text).unwrap(), read, "{text}");
        }
        let (a, b) = (one.options(), other.options());
        for spec in AreaReadText::OPTIONS {
            let key = spec.key();
            if key != "area" {
                assert_ne!(a.get(key), b.get(key), "{key} prints the same for both reads");
            }
        }
        assert_eq!(a.len(), AreaReadText::OPTIONS.len());
        assert_eq!(b.len(), AreaReadText::OPTIONS.len() - 1);
    }

    /// Nothing is defaulted: a missing option, an unknown one, and a value the read cannot take are refused, naming the
    /// option.
    #[test]
    fn an_area_read_text_refuses_naming_the_option() {
        let text = text_form(&reads()[0]);
        let refusal = |text: &str| parse_text_form::<AreaReadText>(text).unwrap_err().to_string();
        assert!(refusal(&text.replace("margin=30,", "")).contains("margin is required"));
        assert!(
            refusal(&text.replace("sky=include", "sky=some")).contains("sky=some is none of none or include")
        );
        assert!(
            refusal(&text.replace("margin=30", "margin=-1")).contains("margin=-1 is not a number of metres")
        );
        assert!(refusal(&text.replace("}", ",tile=1}")).contains("unknown option tile"));
        assert!(refusal(&text.replace("area=m10_00_00_00", "area=")).contains("area is empty"));
    }
}
