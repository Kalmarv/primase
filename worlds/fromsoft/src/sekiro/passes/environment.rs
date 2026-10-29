//! `lower-fromsoft-environment`: the area's draw-param environment (`fromsoft.draw_param`, the groups at the value sets
//! its parts name) as the core environment: the light set's first directional light a directional `core.light` named as
//! the sun by `core.environment`, and its hemisphere colours the environment's gradient ambient. Every other field read
//! is counted as not lowered, by group and the reason: core states fog and exposure (`core.atmosphere`) but no sky or tone
//! curve, and a field's meaning or unit that no source establishes (the fog and tone keys wait for
//! `tests/draw_param_census.rs`) is not guessed into one.
//!
//! The light set's fields (`LightSet ParamEditor`; the keys are the files' own):
//! - `Directional Light Angle0` (two floats, radians) and `Directional Light DiffColor0` (four): the sun's direction and
//!   its diffuse colour, as a frame of the running game (Sekiro 1.06, m11_01 at 18:00) shows them reaching the deferred
//!   sun program `GXLightAcc_DDirLightShadowed1` (its `cbLight` light sets): the angle `(rx, ry)` is the direction
//!   towards the light `(sin ry cos rx, -sin rx, cos ry cos rx)` in the file's frame (the program lights a surface by
//!   `saturate(dot(normal, direction))` and takes its half vector from it), so the light travels the opposite way; the
//!   colour `(r, g, b, w)` is the radiance `(r^2 w, g^2 w, b^2 w)` (the program's constant is `(r^2 w, g^2 w, b^2 w, w^2)`,
//!   the same squaring as BTL light records). Both held on every directional and specular colour of the frame's two light
//!   sets, to float precision. The sun's light is the squared colour, its intensity the weight `w` times the program's
//!   calibration (`lux-per-unit`: the lux of a radiance of 1). A sun whose colour is black or whose weight is 0 (or not
//!   finite, or negative) is none, counted. Elden Ring stores the same fields and is lowered the same way; no Elden Ring
//!   frame has shown its light set reaching a program yet (stated in [`Pass::approximates`]).
//! - `Directional Light Hemi Color Up` and `Hemi Color Down`: the ambient's sky and ground colours, read as linear (no
//!   frame has shown how they reach a program); the equator colour is not stored and is taken as their mean (counted).
//!   The ambient's intensity is `lux-per-unit`, at the scale of a directional light's (the same approximation
//!   `lower-unity-ambient` states). The census files store no `Hemi Color` field at all, so a Sekiro read has no ambient
//!   from this pass.
//!
//! The fills (`Angle1`, `LocalLight` lights) are counted as not lowered. Later files of a map (`_0010` ...) store only the
//! fields they change.
//!
//! The `VolumetricFog ParamEditor` group's fields the census shows (`VolumetricFog Global FixedDensity`, the height
//! density keys, `SkyLight SkyColor`, `DirLight DirectColor`, `GameScale` ...) are kept in `core.atmosphere`'s
//! `volumetric` with the file's own names and values, unconverted: the densities are in a unit the group's `GameScale`
//! sets and no source establishes, so the omission is counted and the renderer draws nothing from it. This is the
//! faithful carrying of the values, not a distance-fog approximation.
use super::settings::Factor;
use crate::sekiro::source::DrawParam;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Ambient, AmbientLight, Atmosphere, Component, Environment, Light, LightKind, Name, Resource, Source,
    Transform, VolumetricFog, World,
};

const LIGHT_SET: &str = "LightSet ParamEditor";
const ANGLE: &str = "Directional Light Angle0";
const DIFFUSE: &str = "Directional Light DiffColor0";
const HEMI_UP: &str = "Directional Light Hemi Color Up";
const HEMI_DOWN: &str = "Directional Light Hemi Color Down";
const VOLUMETRIC: &str = "VolumetricFog ParamEditor";

/// What the lab and the notices say of a volumetric fog this pass keeps: the one place the statement is made.
const VOLUMETRIC_NOT_DRAWN: &str = "volumetric fog kept in core.atmosphere's volumetric fog with the file's own names and values, unconverted: a froxel volume whose densities are in a unit the group's GameScale sets and no source establishes, so no fog is drawn from it until a capture of the game establishes the unit";

fn float(group: &crate::sekiro::source::DrawGroup, key: &str) -> Option<f32> {
    match group.value(key) {
        Some(fromsoft_formats::gparam::Value::Float(x)) if (*x as f32).is_finite() => Some(*x as f32),
        _ => None,
    }
}

fn vec4(group: &crate::sekiro::source::DrawGroup, key: &str) -> Option<[f32; 4]> {
    match group.value(key) {
        Some(fromsoft_formats::gparam::Value::Vec4(c)) if c.iter().all(|x| x.is_finite()) => Some(*c),
        _ => None,
    }
}

/// The group's volumetric fog fields the census shows, as stored, and how many of the group's fields they are.
fn volumetric(group: &crate::sekiro::source::DrawGroup) -> (VolumetricFog, usize) {
    let v = VolumetricFog {
        enabled: match group.value("VolumetricFog Enable") {
            Some(fromsoft_formats::gparam::Value::Bool(b)) => Some(*b),
            _ => None,
        },
        game_scale: float(group, "VolumetricFog GameScale"),
        near_depth: float(group, "VolumetricFog NearDepth"),
        far_depth: float(group, "VolumetricFog FarDepth"),
        near_offset: float(group, "VolumetricFog NearOffset"),
        fixed_density: float(group, "VolumetricFog Global FixedDensity"),
        scattering_g: float(group, "VolumetricFog Global ScatteringG"),
        local_light_scale: float(group, "VolumetricFog Global LocalLightScale"),
        local_light_dist_scale: float(group, "VolumetricFog Global LocalLightDistScale"),
        height_fog_distribution: float(group, "VolumetricFog Global HeightFogDistribution"),
        height_fog_offset: float(group, "VolumetricFog Global HeightFogOffset"),
        height_fog_density_scale: float(group, "VolumetricFog Global HeightFogDensity Scale"),
        sky_colour: vec4(group, "VolumetricFog SkyLight SkyColor"),
        sky_start_dist: float(group, "VolumetricFog SkyLight StartDist"),
        directional_light_bank: match group.value("VolumetricFog DirLight LightBankId") {
            Some(fromsoft_formats::gparam::Value::Uint(b)) => u32::try_from(*b).ok(),
            _ => None,
        },
        directional_direct_colour: vec4(group, "VolumetricFog DirLight DirectColor"),
        directional_opposite_colour: vec4(group, "VolumetricFog DirLight OppositeColor"),
        directional_start_dist: float(group, "VolumetricFog DirLight StartDist"),
    };
    let kept = v.fields().len();
    (v, kept)
}

/// `lower-fromsoft-environment{lux-per-unit}`: see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerFromSoftEnvironment {
    /// Lux of the sun, and the ambient's intensity, for a light colour of 1.
    lux_per_unit: Factor,
}

impl LowerFromSoftEnvironment {
    pub fn new(lux_per_unit: Factor) -> Self {
        LowerFromSoftEnvironment { lux_per_unit }
    }
    pub fn lux_per_unit(&self) -> Factor {
        self.lux_per_unit
    }
    fn from_given(g: &Given) -> Result<Self, String> {
        Ok(Self::new(Factor::parse(
            "lux-per-unit",
            g.required("lux-per-unit"),
        )?))
    }
}

/// `lower-fromsoft-environment`'s declaration ([`primase_core::pass::PassRegistry::add`]): the scale of the stored
/// light colours is not established, so the calibration is the program's.
pub const LOWER_FROMSOFT_ENVIRONMENT: PassSpec = PassSpec {
    name: "lower-fromsoft-environment",
    options: &[OptionSpec::required(
        "lux-per-unit",
        "1",
        "at least 0: the lux of a directional light, and the ambient's intensity, for a stored light colour of 1, the program's calibration",
    )],
    make: |g| Ok(Box::new(LowerFromSoftEnvironment::from_given(g)?)),
};

/// A colour's red, green and blue and its fourth float as stored, if the value is a four-float vector of finite,
/// non-negative components.
fn colour(value: Option<&fromsoft_formats::gparam::Value>) -> Option<([f32; 3], f32)> {
    let fromsoft_formats::gparam::Value::Vec4(c) = value? else {
        return None;
    };
    c.iter()
        .all(|x| x.is_finite() && *x >= 0.)
        .then_some(([c[0], c[1], c[2]], c[3]))
}

/// The direction the sun's light travels, in the file's frame, from its stored angle `(rx, ry)` in radians: the
/// opposite of the direction towards the light, `(sin ry cos rx, -sin rx, cos ry cos rx)`.
fn travel(angle: [f32; 2]) -> [f32; 3] {
    let (sx, cx) = angle[0].sin_cos();
    let (sy, cy) = angle[1].sin_cos();
    [-sy * cx, sx, -cy * cx]
}

/// Why the fields of a group are not lowered, by the group's key: what core has no place for.
fn why_not_lowered(group: &str) -> &'static str {
    match group {
        LIGHT_SET => {
            "specular, sharpness, occlusion, second directional and local lights: no core.light fields"
        }
        VOLUMETRIC => {
            "a volumetric fog key this pass does not know: only the keys of the census are kept in core.atmosphere's volumetric fog"
        }
        "FogVolume ParamEditor" => {
            "Elden Ring's fog volume (30 fields at the value sets the census found): its keys and units are not established, and core.atmosphere's volumetric fog carries Sekiro's VolumetricFog keys only"
        }
        "FogParamEditor" | "Fog Volume" => {
            "fog: core.atmosphere states fog, but no census file stores this group, so its keys and units are not established"
        }
        "Tone Map[Yebis]" => {
            "tone map: every census file enables auto exposure (adapting between its stored minimum and maximum), so no static exposure in stops exists for core.atmosphere; contrast, white balance and colour are a tone curve core states none of"
        }
        "Color Grading" | "ColorGrading[Yebis]" | "HDR Color Correction" => {
            "colour grading: core states none"
        }
        "LightScattering : ParamEditor"
        | "Env Map:Editor"
        | "EnvironmentConfig ParamEditor"
        | "SeaWave:Editor" => "sky, scattering and environment-map references: core has no sky",
        _ => "post-processing and rendering settings: no core meaning",
    }
}

impl Pass for LowerFromSoftEnvironment {
    fn name(&self) -> &'static str {
        "lower-fromsoft-environment"
    }
    fn options(&self) -> Options {
        Options::from([("lux-per-unit".to_owned(), self.lux_per_unit.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &[DrawParam::NAME]
    }
    /// An area read without a draw-param file holds no environment: nothing to lower, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    /// The sun is an entity: its light, where it stands, its name and the value set it came from.
    fn provides(&self) -> &[&'static str] {
        &[
            Light::NAME,
            Transform::NAME,
            Name::NAME,
            Source::NAME,
            Environment::NAME,
            Atmosphere::NAME,
        ]
    }
    fn approximates(&self) -> &'static str {
        "the sun's intensity at the calibrated lux per unit of radiance; Elden Ring's light set read with the transfer a Sekiro frame shows; hemisphere colours as linear; the ambient's equator the mean of its hemispheres"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(env) = world.resource::<DrawParam>().cloned() else {
            log.count("areas with no draw-param environment", 1);
            return Ok(());
        };
        // a field stored at several hours has one value here, its first stored sample, unless the program sampled an hour
        let several = env
            .groups
            .iter()
            .filter(|g| g.chosen != crate::sekiro::source::SetChosen::Unresolved)
            .map(|g| {
                let mut samples = std::collections::BTreeMap::<&str, usize>::new();
                for f in &g.fields {
                    *samples.entry(f.key.as_str()).or_default() += 1;
                }
                samples.values().filter(|n| **n > 1).count() as u64
            })
            .sum::<u64>();
        if several > 0 {
            log.approximate(
                "draw-param fields stored at several hours of the day lowered at their first stored sample (`sample-fromsoft-draw-params` chooses an hour)",
                several,
            );
        }
        let lux = self.lux_per_unit.get() as f32;
        let mut sun = None;
        let mut ambient = None;
        let mut lowered = std::collections::BTreeMap::<&str, usize>::new();
        if let Some(set) = env.group(LIGHT_SET) {
            let angle = match set.value(ANGLE) {
                Some(fromsoft_formats::gparam::Value::Vec2(a)) if a.iter().all(|x| x.is_finite()) => Some(*a),
                _ => None,
            };
            match (angle, colour(set.value(DIFFUSE))) {
                (Some(angle), Some((rgb, weight))) if weight > 0. && rgb.iter().any(|x| *x > 0.) => {
                    let transform = Transform::facing([0.; 3], crate::pose::FRAME.direction(travel(angle)))
                        .ok_or("the sun's angle gives no direction")?;
                    let e = world.spawn();
                    world.insert(
                        e,
                        Light {
                            kind: LightKind::Directional,
                            color: rgb.map(|c| c * c),
                            intensity: lux * weight,
                            range: None,
                            cast_shadows: None,
                            radius: None,
                            specular: None,
                        },
                    );
                    world.insert(e, transform);
                    world.insert(e, Name("sun".into()));
                    world.insert(e, Source(format!("{} {} {}", env.file, LIGHT_SET, set.id)));
                    sun = Some(e);
                    *lowered.entry(LIGHT_SET).or_default() += 2;
                    log.count("suns lowered", 1);
                    log.approximate(
                        "the sun's intensity at the calibrated lux per unit of radiance",
                        1,
                    );
                }
                (Some(_), Some(_)) => {
                    log.omit(
                        "directional light 0 of the light set is black or has intensity 0: no sun",
                        1,
                    );
                }
                _ => {
                    log.omit(
                        "the light set stores no usable directional light 0 (an angle, and a finite non-negative diffuse colour): no sun",
                        1,
                    );
                }
            }
            match (colour(set.value(HEMI_UP)), colour(set.value(HEMI_DOWN))) {
                (Some((sky, _)), Some((ground, _))) => {
                    let equator = [0, 1, 2].map(|i| (sky[i] + ground[i]) / 2.);
                    ambient = Some(Ambient {
                        intensity: lux,
                        light: AmbientLight::Gradient { sky, equator, ground },
                    });
                    *lowered.entry(LIGHT_SET).or_default() += 2;
                    log.count("gradient ambients lowered", 1);
                    log.approximate(
                        "hemisphere colours read as linear (no frame of the game has shown how they reach a program)",
                        1,
                    );
                    log.approximate(
                        "the ambient's equator colour taken as the mean of its hemispheres",
                        1,
                    );
                    log.approximate("ambient intensity at the calibrated lux per unit, the scale of a directional light's", 1);
                }
                _ => {
                    log.omit(
                        "the light set stores no usable hemisphere colours (finite and non-negative): no ambient",
                        1,
                    );
                }
            }
        } else {
            log.omit(
                "the draw-param file has no light set group at a chosen value set: no sun, no ambient",
                1,
            );
        }
        if sun.is_some() || ambient.is_some() {
            let mut environment = world.resource::<Environment>().cloned().unwrap_or_default();
            environment.sun = sun.or(environment.sun);
            if ambient.is_some() {
                environment.ambient = ambient;
            }
            world.insert_resource(environment);
        }
        if let Some(group) = env.group(VOLUMETRIC) {
            let (fog, kept) = volumetric(group);
            if kept > 0 {
                let mut atmosphere = world.resource::<Atmosphere>().cloned().unwrap_or_default();
                atmosphere.volumetric = Some(fog);
                world.insert_resource(atmosphere);
                *lowered.entry(VOLUMETRIC).or_default() += kept;
                log.count("volumetric fogs kept in the source's units", 1);
                log.omit(VOLUMETRIC_NOT_DRAWN, 1);
            }
        }
        // every field read and not lowered, by group and why
        for group in &env.groups {
            let left = group.fields.len()
                - lowered
                    .get(group.key.as_str())
                    .copied()
                    .unwrap_or(0)
                    .min(group.fields.len());
            if left > 0 {
                log.omit(
                    &format!(
                        "draw-param fields of {} read, not lowered ({})",
                        group.key,
                        why_not_lowered(&group.key)
                    ),
                    left as u64,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::source::{DrawField, DrawGroup, SetChosen};
    use fromsoft_formats::gparam::Value;

    fn field(key: &str, value: Value) -> DrawField {
        DrawField {
            key: key.into(),
            name: key.into(),
            value,
            time: None,
        }
    }

    fn group(key: &str, id: i32, fields: Vec<DrawField>) -> DrawGroup {
        DrawGroup {
            key: key.into(),
            name: key.into(),
            id,
            chosen: SetChosen::ByParts,
            fields,
        }
    }

    fn light_set(angle: [f32; 2], diffuse: [f32; 4]) -> DrawGroup {
        group(
            LIGHT_SET,
            7,
            vec![
                field(ANGLE, Value::Vec2(angle)),
                field(DIFFUSE, Value::Vec4(diffuse)),
                field("Directional Light SpecColor0", Value::Vec4([1.; 4])),
                field(HEMI_UP, Value::Vec4([0.25, 0.5, 1.0, 1.0])),
                field(HEMI_DOWN, Value::Vec4([0.05, 0.1, 0.0, 1.0])),
                field("Directional Light Sharpness0", Value::Float(8.0)),
            ],
        )
    }

    fn world_with(groups: Vec<DrawGroup>) -> World {
        let mut w = World::new();
        w.insert_resource(DrawParam {
            file: "m11_01_0720".into(),
            version: 5,
            groups,
        });
        w
    }

    /// The pass as a pipeline runs it, so what it adds is checked against what it declares.
    fn run(w: &mut World, lux: f64) -> PassLog {
        use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                w,
                &Pipeline {
                    passes: vec![Box::new(LowerFromSoftEnvironment::new(Factor::new(lux).unwrap()))],
                },
                &mut report,
            )
            .unwrap();
        assert!(w.verify_resources().is_empty());
        report.steps.remove(0).log
    }

    /// The direction a light shines, its transform's local -Z.
    fn shines(w: &World, e: primase_core::Entity) -> [f32; 3] {
        let c = w.get::<primase_core::Transform>(e).unwrap().cols[2];
        [-c[0], -c[1], -c[2]]
    }

    fn close(a: [f32; 3], b: [f32; 3]) {
        assert!(
            a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5),
            "{a:?} vs {b:?}"
        );
    }

    #[test]
    fn the_light_set_becomes_the_sun_and_a_gradient_ambient_and_the_rest_is_counted() {
        let mut w = world_with(vec![
            light_set([-std::f32::consts::FRAC_PI_2, 0.], [1.0, 0.5, 0.25, 1.0]),
            group("FogParamEditor", 0, vec![field("Fog-Dist", Value::Float(3.0))]),
            group(
                "Tone Map[Yebis]",
                0,
                vec![
                    field("GS Presents - ToneMap-Exposure", Value::Float(1.5)),
                    field("GS Presents - Contrast", Value::Float(1.0)),
                ],
            ),
        ]);
        let log = run(&mut w, 100.0);
        let env = w.resource::<Environment>().unwrap().clone();
        let sun = env.sun.expect("a sun");
        let light = w.get::<Light>(sun).unwrap();
        assert_eq!(light.kind, LightKind::Directional);
        // the colour squared, the weight times the calibration
        assert_eq!((light.color, light.intensity), ([1.0, 0.25, 0.0625], 100.0));
        // rx = -pi/2: the light is straight above, so it shines straight down
        close(shines(&w, sun), [0., -1., 0.]);
        assert_eq!(w.get::<Name>(sun).unwrap().0, "sun");
        assert_eq!(
            w.get::<Source>(sun).unwrap().0,
            "m11_01_0720 LightSet ParamEditor 7"
        );
        assert_eq!(
            env.ambient,
            Some(Ambient {
                intensity: 100.0,
                light: AmbientLight::Gradient {
                    sky: [0.25, 0.5, 1.0],
                    equator: [0.15, 0.3, 0.5],
                    ground: [0.05, 0.1, 0.0],
                },
            })
        );
        assert_eq!(
            (
                log.counts["suns lowered"],
                log.counts["gradient ambients lowered"]
            ),
            (1, 1)
        );
        // four of the light set's six fields are lowered; the rest, the fog and the tone are counted by group
        assert_eq!(
            log.omitted[&format!(
                "draw-param fields of {LIGHT_SET} read, not lowered ({})",
                why_not_lowered(LIGHT_SET)
            )],
            2
        );
        assert_eq!(
            log.omitted[&format!(
                "draw-param fields of FogParamEditor read, not lowered ({})",
                why_not_lowered("FogParamEditor")
            )],
            1
        );
        assert_eq!(
            log.omitted[&format!(
                "draw-param fields of Tone Map[Yebis] read, not lowered ({})",
                why_not_lowered("Tone Map[Yebis]")
            )],
            2
        );
        assert_eq!(log.omitted.len(), 3, "{:?}", log.omitted);
        assert_eq!(log.approximations.len(), 4, "{:?}", log.approximations);
    }

    #[test]
    fn zero_angles_put_the_light_at_plus_z_of_the_file_and_a_yaw_turns_it_about_the_vertical() {
        // (0, 0): towards the light is +Z of the file, so the light travels along its -Z, the world's +Z
        let mut w = world_with(vec![light_set([0., 0.], [1.; 4])]);
        run(&mut w, 1.0);
        let sun = w.resource::<Environment>().unwrap().sun.unwrap();
        close(shines(&w, sun), [0., 0., 1.]);
        // ry = pi/2: towards the light is +X, so the light travels along -X (the frame keeps x)
        let mut w = world_with(vec![light_set([0., std::f32::consts::FRAC_PI_2], [1.; 4])]);
        run(&mut w, 1.0);
        let sun = w.resource::<Environment>().unwrap().sun.unwrap();
        close(shines(&w, sun), [-1., 0., 0.]);
    }

    #[test]
    // the file stores -3.141593 (f32 0xc0490fdc), one unit in the last place from -pi: the stored value, not pi
    #[allow(clippy::approx_constant)]
    fn the_light_set_of_a_frame_of_the_game_gives_the_constants_its_sun_program_read() {
        // Sekiro 1.06, Ashina Castle gate: m11_01_0000's light set (value id 0) at 18:00 is the second light set of the
        // frame's `cbLight`, which `GXLightAcc_DDirLightShadowed1` read as the direction towards the light and the
        // radiance (the w lane of its colour is w^2, not a radiance)
        let mut w = world_with(vec![light_set(
            [-0.436332, -3.141593],
            [1.0, 0.784314, 0.588235, 1.25],
        )]);
        run(&mut w, 1.0);
        let sun = w.resource::<Environment>().unwrap().sun.unwrap();
        let light = w.get::<Light>(sun).unwrap();
        let radiance = light.color.map(|c| c * light.intensity);
        for (got, frame) in radiance.iter().zip([1.25f32, 0.7689356, 0.4325255]) {
            assert!((got - frame).abs() <= 1e-6 * frame.abs().max(1.0), "{radiance:?}");
        }
        let towards = crate::pose::FRAME.direction_to_source(shines(&w, sun).map(|x| -x));
        close(towards, [3.529293e-07, 0.422618, -0.9063079]);
    }

    #[test]
    fn the_fourth_float_of_the_colour_scales_the_suns_intensity() {
        // m11_01_0000's main light: (0.687284, 0.757668, 0.828053) x 2.0, its angle in radians (-20 degrees pitch)
        let mut w = world_with(vec![light_set(
            [-0.349066, 0.0],
            [0.687284, 0.757668, 0.828053, 2.0],
        )]);
        run(&mut w, 100.0);
        let sun = w.resource::<Environment>().unwrap().sun.unwrap();
        let light = w.get::<Light>(sun).unwrap();
        assert_eq!(
            (light.color, light.intensity),
            (
                [0.687284f32 * 0.687284, 0.757668 * 0.757668, 0.828053 * 0.828053],
                200.0
            )
        );
        // a pitch of -20 degrees tilts the light by 20 degrees about the horizontal: it shines along Z, a little vertically
        let d = shines(&w, sun);
        assert!((d[1].abs() - 20f32.to_radians().sin()).abs() < 1e-4, "{d:?}");
    }

    #[test]
    fn a_black_unusable_or_absent_light_is_no_sun_and_counted() {
        for (diffuse, why) in [
            (
                [0., 0., 0., 1.],
                "directional light 0 of the light set is black or has intensity 0: no sun",
            ),
            (
                [1., 1., 1., 0.],
                "directional light 0 of the light set is black or has intensity 0: no sun",
            ),
            (
                [f32::NAN, 0., 0., 1.],
                "the light set stores no usable directional light 0 (an angle, and a finite non-negative diffuse colour): no sun",
            ),
            (
                [-1., 0., 0., 1.],
                "the light set stores no usable directional light 0 (an angle, and a finite non-negative diffuse colour): no sun",
            ),
        ] {
            let mut w = world_with(vec![light_set([10., 20.], diffuse)]);
            let log = run(&mut w, 1.0);
            assert_eq!(w.resource::<Environment>().unwrap().sun, None, "{diffuse:?}");
            assert!(
                w.resource::<Environment>().unwrap().ambient.is_some(),
                "the hemispheres are still read"
            );
            assert_eq!(log.omitted[why], 1, "{diffuse:?}");
            assert_eq!(w.iter::<Light>().count(), 0);
        }
        // no light set at all, and no draw param at all
        let mut w = world_with(vec![group("Bloom", 0, vec![field("A", Value::Float(1.0))])]);
        let log = run(&mut w, 1.0);
        assert!(w.resource::<Environment>().is_none());
        assert_eq!(
            log.omitted["the draw-param file has no light set group at a chosen value set: no sun, no ambient"],
            1
        );
        // a pipeline skips the pass where there is no draw param (it may lack its input); run alone, it says so
        let mut bare = World::new();
        let mut log = PassLog::default();
        LowerFromSoftEnvironment::new(Factor::new(1.0).unwrap())
            .run(&mut bare, &mut log)
            .unwrap();
        assert_eq!(log.counts["areas with no draw-param environment"], 1);
        assert!(bare.resource::<Environment>().is_none());
    }

    #[test]
    fn a_volumetric_fog_group_is_kept_in_the_files_units_and_counted_as_not_drawn() {
        // m11_01_0000's values (tests/draw_param_census.rs): GameScale 12, FixedDensity 0, height fog 0.075
        let mut w = world_with(vec![group(
            VOLUMETRIC,
            0,
            vec![
                field("VolumetricFog Enable", Value::Bool(true)),
                field("VolumetricFog GameScale", Value::Float(12.0)),
                field("VolumetricFog Global FixedDensity", Value::Float(0.0)),
                field(
                    "VolumetricFog Global HeightFogDensity Scale",
                    Value::Float(0.07500000298023224),
                ),
                field(
                    "VolumetricFog SkyLight SkyColor",
                    Value::Vec4([0.062745, 0.070588, 0.078431, 0.0]),
                ),
                field("VolumetricFog DirLight LightBankId", Value::Uint(111010000)),
                field("VolumetricFog Something Unknown", Value::Float(1.0)),
            ],
        )]);
        let log = run(&mut w, 1.0);
        let atmosphere = w.resource::<Atmosphere>().unwrap();
        assert_eq!(
            (atmosphere.fog.clone(), atmosphere.exposure.clone()),
            (None, None)
        );
        let v = atmosphere.volumetric.as_ref().unwrap();
        assert_eq!(
            (
                v.enabled,
                v.game_scale,
                v.fixed_density,
                v.height_fog_density_scale,
                v.directional_light_bank
            ),
            (Some(true), Some(12.0), Some(0.0), Some(0.075), Some(111010000))
        );
        assert_eq!(v.sky_colour, Some([0.062745, 0.070588, 0.078431, 0.0]));
        assert_eq!(v.far_depth, None);
        assert_eq!(log.counts["volumetric fogs kept in the source's units"], 1);
        assert_eq!(log.omitted[VOLUMETRIC_NOT_DRAWN], 1);
        // the one key the pass does not know is still counted as read and not lowered
        assert_eq!(
            log.omitted[&format!(
                "draw-param fields of {VOLUMETRIC} read, not lowered ({})",
                why_not_lowered(VOLUMETRIC)
            )],
            1
        );
    }

    #[test]
    fn a_volumetric_group_that_stores_nothing_leaves_the_atmosphere_alone() {
        let mut w = world_with(vec![group(VOLUMETRIC, 0, vec![])]);
        run(&mut w, 1.0);
        assert!(w.resource::<Atmosphere>().is_none());
    }

    #[test]
    fn an_existing_environment_keeps_what_the_file_does_not_state() {
        let mut w = world_with(vec![group("Bloom", 0, vec![])]);
        let other = w.spawn();
        w.insert(
            other,
            Light {
                kind: LightKind::Directional,
                color: [1.; 3],
                intensity: 1.0,
                range: None,
                cast_shadows: None,
                radius: None,
                specular: None,
            },
        );
        w.insert_resource(Environment {
            sun: Some(other),
            ambient: None,
        });
        run(&mut w, 1.0);
        assert_eq!(w.resource::<Environment>().unwrap().sun, Some(other));
    }

    #[test]
    fn the_calibration_is_the_programs_with_no_default() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(LOWER_FROMSOFT_ENVIRONMENT.instance(&o(&[])).is_err());
        for bad in ["-1", "nan", "inf", "x", ""] {
            assert!(
                LOWER_FROMSOFT_ENVIRONMENT
                    .instance(&o(&[("lux-per-unit", bad)]))
                    .is_err(),
                "{bad}"
            );
        }
        assert!(
            LOWER_FROMSOFT_ENVIRONMENT
                .instance(&o(&[("lux-per-unit", "1"), ("x", "1")]))
                .is_err()
        );
        let both = o(&[("lux-per-unit", "1771")]);
        assert_eq!(
            LOWER_FROMSOFT_ENVIRONMENT.instance(&both).unwrap().options(),
            both
        );
    }
}
