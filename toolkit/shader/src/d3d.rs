//! D3D's enum numbers and packing rules, mapped into the vocabulary ([`primase_core::shader`]). The DXBC and DXIL
//! readers share them: both state signatures and constant buffers in D3D's terms. Nothing here is evaluation; a
//! number the table does not map is refused with its name (an `Err` the reader reports), never guessed.

use primase_core::shader::{
    ConstantType, D3dSignature, InterpMode, InterpSampling, Interpolation, Label, Majorness, ScalarType,
    Stage, SystemValue, Value, Varying, VaryingProvenance,
};

/// One element of a D3D signature as it is stored (`ISGN`, `OSGN`, `ISG1`, `OSG1`).
#[derive(Clone, Copy, Debug)]
pub struct Element<'a> {
    pub name: &'a str,
    pub index: u32,
    /// `D3D_NAME`.
    pub system_value: u32,
    /// `D3D_REGISTER_COMPONENT_TYPE`.
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    /// The signature's second mask: for an input the lanes always read, for an output the lanes never written.
    pub read_write_mask: u8,
}

/// `D3D_NAME`'s name for a value, where the enum has one.
pub fn name_of(value: u32) -> Option<&'static str> {
    Some(match value {
        0 => "UNDEFINED",
        1 => "POSITION",
        2 => "CLIP_DISTANCE",
        3 => "CULL_DISTANCE",
        4 => "RENDER_TARGET_ARRAY_INDEX",
        5 => "VIEWPORT_ARRAY_INDEX",
        6 => "VERTEX_ID",
        7 => "PRIMITIVE_ID",
        8 => "INSTANCE_ID",
        9 => "IS_FRONT_FACE",
        10 => "SAMPLE_INDEX",
        11..=23 => "FINAL_*_TESSFACTOR",
        64 => "TARGET",
        65 => "DEPTH",
        66 => "COVERAGE",
        67 => "DEPTH_GREATER_EQUAL",
        68 => "DEPTH_LESS_EQUAL",
        69 => "STENCIL_REF",
        70 => "INNER_COVERAGE",
        _ => return None,
    })
}

fn refused(what: &str, value: u32, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{what} {value} ({name}) is not mapped"),
        None => format!("{what} {value} is not a known value"),
    }
}

/// The system value a `D3D_NAME` number stands for in `stage`'s input or output signature; `Ok(None)` for a plain
/// varying (0).
fn system_from_number(
    stage: Stage,
    output: bool,
    value: u32,
    index: u32,
) -> Result<Option<SystemValue>, String> {
    use SystemValue::*;
    Ok(Some(match (stage, output, value) {
        (_, _, 0) => return Ok(None),
        (Stage::Vertex, true, 1) => ClipPosition,
        (Stage::Pixel, false, 1) => FragCoord,
        (Stage::Vertex, true, 2) | (Stage::Pixel, false, 2) => ClipDistance,
        (Stage::Vertex, true, 3) | (Stage::Pixel, false, 3) => CullDistance,
        (Stage::Vertex, true, 4) | (Stage::Pixel, false, 4) => RenderTargetArrayIndex,
        (Stage::Vertex, true, 5) | (Stage::Pixel, false, 5) => ViewportArrayIndex,
        (Stage::Vertex, false, 6) => VertexIndex,
        (Stage::Pixel, false, 7) => PrimitiveIndex,
        (Stage::Vertex, false, 8) => InstanceIndex,
        (Stage::Pixel, false, 9) => FrontFacing,
        (Stage::Pixel, false, 10) => SampleIndex,
        (Stage::Pixel, true, 64) => Target(index),
        (Stage::Pixel, true, 65) => Depth,
        (Stage::Pixel, _, 66) => Coverage,
        (Stage::Pixel, true, 67) => DepthGreaterEqual,
        (Stage::Pixel, true, 68) => DepthLessEqual,
        (Stage::Pixel, true, 69) => StencilRef,
        (Stage::Pixel, false, 70) => InnerCoverage,
        _ => {
            return Err(format!(
                "{} as {} of the {stage:?} stage",
                refused("D3D_NAME", value, name_of(value)),
                if output { "an output" } else { "an input" },
            ));
        }
    }))
}

/// The system value an `SV_` semantic name stands for in `stage`'s signature (the compiler states some, such as
/// `SV_Target`, by name alone).
fn system_from_name(stage: Stage, output: bool, name: &str, index: u32) -> Result<SystemValue, String> {
    let key = name[3..].to_ascii_lowercase();
    let number = match key.as_str() {
        "position" => 1,
        "clipdistance" => 2,
        "culldistance" => 3,
        "rendertargetarrayindex" => 4,
        "viewportarrayindex" => 5,
        "vertexid" => 6,
        "primitiveid" => 7,
        "instanceid" => 8,
        "isfrontface" => 9,
        "sampleindex" => 10,
        "target" => 64,
        "depth" => 65,
        "coverage" => 66,
        "depthgreaterequal" => 67,
        "depthlessequal" => 68,
        "stencilref" => 69,
        "innercoverage" => 70,
        _ => return Err(format!("system-value semantic {name} is not mapped")),
    };
    system_from_number(stage, output, number, index)?
        .ok_or_else(|| format!("system-value semantic {name} is not mapped"))
}

/// `D3D_REGISTER_COMPONENT_TYPE`.
fn component(value: u32) -> Result<ScalarType, String> {
    match value {
        1 => Ok(ScalarType::Uint),
        2 => Ok(ScalarType::Sint),
        3 => Ok(ScalarType::Float),
        0 => Err("signature component type 0 (unknown) is not mapped".to_owned()),
        _ => Err(refused("signature component type", value, None)),
    }
}

/// One signature element of `stage`, unpacked from its register: the label, the system value (the stored
/// `D3D_NAME`, or the `SV_` name when the compiler states it by name), the value type from the component type and the
/// lanes it occupies, and the register and lanes kept in provenance. `interp` is left for the caller, who has the
/// program's declarations.
pub fn varying(stage: Stage, output: bool, e: &Element<'_>) -> Result<Varying, String> {
    let at = |detail: String| format!("{}{}: {detail}", e.name, e.index);
    if e.mask == 0 || e.mask > 15 || e.read_write_mask > 15 {
        return Err(at(format!(
            "lane masks {:#x} and {:#x} are not within the register's four lanes",
            e.mask, e.read_write_mask
        )));
    }
    let named = e.name.len() > 3 && e.name.as_bytes()[..3].eq_ignore_ascii_case(b"SV_");
    let system = match system_from_number(stage, output, e.system_value, e.index).map_err(at)? {
        Some(s) => {
            if named && system_from_name(stage, output, e.name, e.index).map_err(at)? != s {
                return Err(at(format!(
                    "the semantic disagrees with its stored D3D_NAME {}",
                    e.system_value
                )));
            }
            Some(s)
        }
        // the compiler states an output's system value (SV_Target, SV_Depth) by name alone; an input whose stored
        // D3D_NAME is undefined is a plain varying whatever its semantic is called (Unity's pixel programs carry an
        // `SV_InstanceID` that way)
        None if named && output => Some(system_from_name(stage, output, e.name, e.index).map_err(at)?),
        None => None,
    };
    let written = e.mask & !e.read_write_mask;
    Ok(Varying {
        label: Label {
            name: e.name.to_owned(),
            index: e.index,
        },
        system,
        value: Value {
            scalar: component(e.component_type).map_err(at)?,
            width: e.mask.count_ones() as u8,
        },
        interp: None,
        used: if output { written } else { e.read_write_mask } != 0,
        provenance: VaryingProvenance {
            d3d: Some(D3dSignature {
                register: e.register,
                mask: e.mask,
                read_write_mask: e.read_write_mask,
            }),
        },
    })
}

/// A pixel input's interpolation from a program declaration's `D3D_INTERPOLATION_MODE` (the instruction token's
/// bits 11 to 14).
pub fn interpolation(mode: u32) -> Result<Interpolation, String> {
    let (mode, sampling) = match mode {
        1 => (InterpMode::Flat, InterpSampling::Center),
        2 => (InterpMode::Perspective, InterpSampling::Center),
        3 => (InterpMode::Perspective, InterpSampling::Centroid),
        4 => (InterpMode::Linear, InterpSampling::Center),
        5 => (InterpMode::Linear, InterpSampling::Centroid),
        6 => (InterpMode::Perspective, InterpSampling::Sample),
        7 => (InterpMode::Linear, InterpSampling::Sample),
        _ => return Err(refused("interpolation mode", mode, None)),
    };
    Ok(Interpolation { mode, sampling })
}

/// `D3D_SHADER_VARIABLE_TYPE` of a constant-buffer scalar.
pub fn scalar_type(code: u32) -> Result<ScalarType, String> {
    match code {
        1 => Ok(ScalarType::Bool),
        2 => Ok(ScalarType::Sint),
        3 => Ok(ScalarType::Float),
        19 => Ok(ScalarType::Uint),
        39 => Ok(ScalarType::Double),
        _ => Err(refused("constant scalar type", code, None)),
    }
}

/// Bytes of one D3D constant register.
const REGISTER: u32 = 16;

fn lanes(what: &str, n: u32) -> Result<u8, String> {
    u8::try_from(n)
        .ok()
        .filter(|n| (1..=4).contains(n))
        .ok_or_else(|| format!("{what} of {n} is not within 1 to 4"))
}

/// A vector of `width` lanes (1 to 4); a vector of one lane is a scalar.
pub fn vector(scalar: ScalarType, width: u32) -> Result<ConstantType, String> {
    Ok(match lanes("vector width", width)? {
        1 => ConstantType::Scalar(scalar),
        width => ConstantType::Vector(Value { scalar, width }),
    })
}

/// A matrix under D3D packing: each line (a row for `Row`, a column for `Column`) starts a 16-byte register.
pub fn matrix(scalar: ScalarType, rows: u32, columns: u32, major: Majorness) -> Result<ConstantType, String> {
    Ok(ConstantType::Matrix {
        scalar,
        rows: lanes("matrix rows", rows)?,
        columns: lanes("matrix columns", columns)?,
        major,
        stride: REGISTER,
    })
}

/// An array under D3D packing: each element starts a 16-byte register, and the last takes only its own bytes.
pub fn array(element: ConstantType, count: u32) -> Result<ConstantType, String> {
    if count == 0 {
        return Err("an array of no elements".to_owned());
    }
    let size = element.size().ok_or("array element size overflow")?;
    let stride = size
        .checked_add(REGISTER - 1)
        .map(|s| s & !(REGISTER - 1))
        .ok_or("array element size overflow")?;
    let array = ConstantType::Array {
        element: Box::new(element),
        count,
        stride,
    };
    array.size().ok_or("array size overflow")?;
    Ok(array)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(name: &str, sv: u32, mask: u8, rw: u8) -> Element<'_> {
        Element {
            name,
            index: 0,
            system_value: sv,
            component_type: 3,
            register: 1,
            mask,
            read_write_mask: rw,
        }
    }

    /// Pixel inputs: the stored `D3D_NAME` or the `SV_` name makes the system value, a plain semantic has none.
    #[test]
    fn signature_elements_map_by_number_and_by_name() {
        let position = varying(Stage::Pixel, false, &element("SV_Position", 1, 15, 15)).unwrap();
        assert_eq!(position.system, Some(SystemValue::FragCoord));
        assert_eq!(position.value.width, 4);
        let uv = varying(Stage::Pixel, false, &element("TEXCOORD", 0, 0b1100, 0b0100)).unwrap();
        assert_eq!((uv.system, uv.value.width, uv.used), (None, 2, true));
        let d3d = uv.provenance.d3d.unwrap();
        assert_eq!((d3d.register, d3d.mask, d3d.read_write_mask), (1, 0b1100, 0b0100));
        // the compiler states SV_Target by name alone, in the output signature
        let mut target = element("SV_Target", 0, 15, 0);
        target.index = 2;
        let target = varying(Stage::Pixel, true, &target).unwrap();
        assert_eq!(target.system, Some(SystemValue::Target(2)));
        assert!(target.used);
        // an output's second mask is the lanes never written: all four never written means unused
        assert!(
            !varying(Stage::Pixel, true, &element("TEXCOORD", 0, 15, 15))
                .unwrap()
                .used
        );
        let front = varying(
            Stage::Pixel,
            false,
            &Element {
                component_type: 1,
                ..element("SV_IsFrontFace", 9, 1, 1)
            },
        )
        .unwrap();
        assert_eq!(front.system, Some(SystemValue::FrontFacing));
        assert_eq!(front.value.scalar, ScalarType::Uint);
        let clip = varying(Stage::Vertex, true, &element("SV_Position", 1, 15, 0)).unwrap();
        assert_eq!(clip.system, Some(SystemValue::ClipPosition));
    }

    #[test]
    fn what_the_table_does_not_map_is_refused_by_name() {
        let refusal = |stage, output, e: Element<'_>| varying(stage, output, &e).unwrap_err();
        // a tessellation factor, a name that disagrees with its number, an unknown SV_ name, a position written by a
        // pixel stage, an unknown component type and an empty mask
        assert!(
            refusal(Stage::Pixel, false, element("SV_TessFactor", 11, 1, 1)).contains("FINAL_*_TESSFACTOR")
        );
        assert!(refusal(Stage::Pixel, false, element("SV_IsFrontFace", 1, 1, 1)).contains("disagrees"));
        assert!(refusal(Stage::Pixel, false, element("SV_GroupID", 0, 1, 1)).contains("SV_GroupID"));
        assert!(refusal(Stage::Pixel, true, element("SV_Position", 1, 15, 0)).contains("POSITION"));
        let mut odd = element("TEXCOORD", 0, 15, 15);
        odd.component_type = 7;
        assert!(refusal(Stage::Pixel, false, odd).contains("component type 7"));
        assert!(refusal(Stage::Pixel, false, element("TEXCOORD", 0, 0, 0)).contains("lane masks"));
    }

    #[test]
    fn interpolation_modes_split_into_mode_and_sampling() {
        assert_eq!(
            interpolation(5).unwrap(),
            Interpolation {
                mode: InterpMode::Linear,
                sampling: InterpSampling::Centroid
            }
        );
        assert_eq!(interpolation(1).unwrap().mode, InterpMode::Flat);
        assert!(interpolation(0).is_err() && interpolation(8).is_err());
    }

    /// D3D packing: matrix lines and array elements start a 16-byte register, the last taking only its bytes.
    #[test]
    fn packing_strides_follow_the_registers() {
        let f = ScalarType::Float;
        assert_eq!(matrix(f, 3, 3, Majorness::Row).unwrap().size(), Some(44));
        assert_eq!(matrix(f, 4, 4, Majorness::Column).unwrap().size(), Some(64));
        let offsets = array(vector(f, 2).unwrap(), 3).unwrap();
        assert_eq!(offsets.size(), Some(40));
        let ConstantType::Array { stride, .. } = offsets else {
            panic!("an array")
        };
        assert_eq!(stride, 16);
        // a 64-byte matrix needs no padding between elements
        let ConstantType::Array { stride, .. } = array(matrix(f, 4, 4, Majorness::Row).unwrap(), 2).unwrap()
        else {
            panic!("an array")
        };
        assert_eq!(stride, 64);
        assert!(
            vector(f, 5).is_err()
                && matrix(f, 0, 1, Majorness::Row).is_err()
                && array(vector(f, 1).unwrap(), 0).is_err()
        );
        assert!(scalar_type(39).is_ok() && scalar_type(10).is_err());
    }
}
