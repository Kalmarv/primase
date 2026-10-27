//! The GPU half of the surface reading ([`primase_shader::surface`]): the rows a surface packs to, and the WGSL that
//! decodes them, read back by naga's front end and validated, its row constants the crate's.
use primase_core::shader::surface::Surface;
use primase_core::shader::{LaneAffine, NormalEncoding, NormalSpace, Occluded, OutputRole, ScalarEncoding};
use primase_shader::surface::{self, ROWS, WGSL, rows};

fn read(outputs: &[(u8, OutputRole)]) -> Surface {
    Surface::read(outputs.iter().map(|(lanes, role)| (*lanes, role)))
}

/// A G-buffer with every term, in five targets: each row names its target plus one, its first lane, its kind and its
/// encoding's power, root and scale, or the normal encoding's numbers.
#[test]
fn a_surface_packs_one_row_per_term() {
    let bias = 0x3f00_2008;
    let outputs = [
        (
            0b0111,
            OutputRole::Normal {
                encoding: NormalEncoding::XyzBiased { bias_bits: bias },
                space: NormalSpace::World,
            },
        ),
        (
            0b0001,
            OutputRole::Gloss {
                encoding: ScalarEncoding::square_of(0.95),
            },
        ),
        (0b0010, OutputRole::Metallic),
        (0b0111, OutputRole::Albedo),
        (
            0b0111,
            OutputRole::Reflectance {
                encoding: ScalarEncoding::square_root_of(1.0),
            },
        ),
        (
            0b0001,
            OutputRole::SpecularIntensity {
                encoding: ScalarEncoding::square_root_of(1.0),
            },
        ),
        (
            0b0100,
            OutputRole::Emission {
                encoding: ScalarEncoding::linear_over(16.0),
            },
        ),
        (
            0b1000,
            OutputRole::Occlusion {
                of: Occluded::AllLight,
                encoding: ScalarEncoding::LINEAR,
            },
        ),
    ];
    // outputs 0 to 7 are in targets 0, 1, 1, 2, 3, 3, 4, 4
    let targets = [0u32, 1, 1, 2, 3, 3, 4, 4];
    let packed = rows(&read(&outputs), |o| targets.get(o).copied());
    let one = 1.0f32.to_bits();
    // y: the first lane, the width << 4, the kind << 8, the power << 16 and the root << 24
    let y = |first: u32, width: u32, kind: u32, power: u32, root: u32| {
        first | width << 4 | kind << 8 | power << 16 | root << 24
    };
    assert_eq!(packed[surface::BASE], [3, y(0, 3, 1, 1, 1), 0, 0]);
    assert_eq!(packed[surface::NORMAL], [1, y(0, 3, 3, 1, 1), bias, 0]);
    // a gloss: square_of(0.95) is power 1, root 2
    assert_eq!(
        packed[surface::ROUGHNESS],
        [2, y(0, 1, 2, 1, 2), 0.95f32.to_bits(), 0]
    );
    // the metallic in lane 1
    assert_eq!(packed[surface::METALLIC], [2, y(1, 1, 1, 1, 1), one, 0]);
    assert_eq!(packed[surface::REFLECTANCE], [4, y(0, 3, 1, 2, 1), one, 0]);
    assert_eq!(packed[surface::SPECULAR], [4, y(0, 1, 1, 2, 1), one, 0]);
    // one lane of emission in lane 2, a scale of the albedo
    assert_eq!(
        packed[surface::EMISSION],
        [5, y(2, 1, 2, 1, 1), 16.0f32.to_bits(), 0]
    );
    assert_eq!(packed[surface::OCCLUSION], [5, y(3, 1, 1, 1, 1), one, 0]);
    // the occlusion of every light is stated, so there is no ambient-only row
    assert_eq!(packed[surface::AMBIENT_OCCLUSION], [0; 4]);

    // an occlusion of the environment's ambient light alone has its own row, the occlusion row empty: a fourth power
    // times 4 in lane 0 of target 3
    let ambient = ScalarEncoding {
        power: 4,
        root: 1,
        scale_bits: 4.0f32.to_bits(),
    };
    let packed = rows(
        &read(&[(
            0b0001,
            OutputRole::Occlusion {
                of: Occluded::NaturalAmbient,
                encoding: ambient,
            },
        )]),
        |_| Some(3),
    );
    assert_eq!(packed[surface::OCCLUSION], [0; 4]);
    assert_eq!(
        packed[surface::AMBIENT_OCCLUSION],
        [4, y(0, 1, 1, 4, 1), 4.0f32.to_bits(), 0]
    );
}

/// A term the surface does not have, and a term whose output the consumer has no target for, is a row of zeros; the
/// normal encodings with numbers carry them in z and w.
#[test]
fn a_missing_term_is_a_row_of_zeros_and_normal_numbers_ride_in_z_and_w() {
    let minor = LaneAffine::new(0.5, 0.5);
    let outputs = [
        (
            0b1111,
            OutputRole::Normal {
                encoding: NormalEncoding::CubeFace {
                    minor,
                    payload: LaneAffine::new(0.25, 0.1),
                },
                space: NormalSpace::World,
            },
        ),
        (0b0111, OutputRole::Albedo),
    ];
    let packed = rows(&read(&outputs), |o| (o == 0).then_some(6));
    assert_eq!(
        packed[surface::NORMAL],
        [
            7,
            4 << 4 | 8 << 8 | 1 << 16 | 1 << 24,
            minor.scale_bits,
            minor.bias_bits
        ]
    );
    // the albedo's output has no target: no row
    for row in [
        surface::BASE,
        surface::ROUGHNESS,
        surface::METALLIC,
        surface::REFLECTANCE,
        surface::SPECULAR,
        surface::EMISSION,
        surface::OCCLUSION,
        surface::AMBIENT_OCCLUSION,
    ] {
        assert_eq!(packed[row], [0; 4], "row {row}");
    }
    let octahedral = read(&[(
        0b0011,
        OutputRole::Normal {
            encoding: NormalEncoding::Octahedral {
                stored: LaneAffine::UNORM,
                read: LaneAffine::SIGNED,
            },
            space: NormalSpace::View,
        },
    )]);
    assert_eq!(
        rows(&octahedral, |_| Some(0))[surface::NORMAL],
        [
            1,
            2 << 4 | 6 << 8 | 1 << 16 | 1 << 24,
            LaneAffine::SIGNED.scale_bits,
            LaneAffine::SIGNED.bias_bits
        ]
    );
}

/// The WGSL parses and validates on its own, with a pixel entry point that calls every function, and its row constants
/// are the crate's.
#[test]
fn the_wgsl_validates_and_its_row_constants_are_the_crates() {
    let text = format!(
        "{WGSL}
@group(0) @binding(0) var<uniform> table: array<vec4<u32>, {rows}>;
@fragment
fn main(@location(0) value: vec4<f32>) -> @location(0) vec4<f32> {{
    let albedo = primase_base(table[primase_surface_base], value);
    let n = primase_normal(table[primase_surface_normal], value);
    let r = primase_roughness(table[primase_surface_roughness], value);
    let m = primase_scalar(table[primase_surface_metallic], value);
    let f0 = primase_colour(table[primase_surface_reflectance], value);
    let s = primase_scalar(table[primase_surface_specular], value);
    let e = primase_emission(table[primase_surface_emission], value, albedo);
    let o = primase_scalar(table[primase_surface_occlusion], value);
    let a = primase_scalar(table[primase_surface_ambient_occlusion], value);
    let shown = primase_stored(table[primase_surface_roughness], value);
    return vec4<f32>(albedo + n + f0 + e + shown, r + m + s + o + a);
}}
",
        rows = ROWS
    );
    let module = naga::front::wgsl::parse_str(&text).unwrap_or_else(|e| panic!("the WGSL parses: {e}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("the WGSL validates: {e:?}"));
    let constant = |name: &str| -> u32 {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("the WGSL declares {name}"));
        match module.global_expressions[c.init] {
            naga::Expression::Literal(naga::Literal::U32(v)) => v,
            ref other => panic!("{name} is {other:?}, not a u32 literal"),
        }
    };
    let expected = [
        ("primase_surface_rows", ROWS),
        ("primase_surface_base", surface::BASE),
        ("primase_surface_normal", surface::NORMAL),
        ("primase_surface_roughness", surface::ROUGHNESS),
        ("primase_surface_metallic", surface::METALLIC),
        ("primase_surface_reflectance", surface::REFLECTANCE),
        ("primase_surface_specular", surface::SPECULAR),
        ("primase_surface_emission", surface::EMISSION),
        ("primase_surface_occlusion", surface::OCCLUSION),
        ("primase_surface_ambient_occlusion", surface::AMBIENT_OCCLUSION),
    ];
    for (name, value) in expected {
        assert_eq!(constant(name) as usize, value, "{name}");
    }
}
