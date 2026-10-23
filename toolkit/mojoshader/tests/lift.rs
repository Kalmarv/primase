#![allow(clippy::unwrap_used)]
//! Programs in MojoShader's dialect, written as it writes them, lifted: they validate and are written as WGSL; what the
//! dialect does not have is refused by name.
use primase_mojoshader::{Attribute, Constant, Hints, Sampler, lift};
use primase_shader::{Stage, emit};

const GAMMA: &str = "#ifdef GL_ES
precision mediump float;
precision mediump int;
#endif

uniform vec4 ps_uniforms_vec4[1];
vec4 ps_r0;
vec4 ps_r1;
#define ps_c0 ps_uniforms_vec4[0]
uniform sampler2D ps_s0;
varying vec4 vFrontColor;
#define ps_v0 vFrontColor
#define ps_oC0 gl_FragColor
varying vec4 vTexCoord0;
#define ps_v1 vTexCoord0

void main()
{
	ps_r0 = texture2D(ps_s0, ps_v1.xy);
	ps_r0 = ps_r0 * ps_v0;
	ps_r1.x = log2(ps_r0.x);
	ps_r1.y = log2(ps_r0.y);
	ps_r1.z = log2(ps_r0.z);
	ps_oC0.w = ps_r0.w;
	ps_r0.xyz = ps_r1.xyz * ps_c0.xxx;
	ps_oC0.x = exp2(ps_r0.x);
	ps_oC0.y = exp2(ps_r0.y);
	ps_oC0.z = exp2(ps_r0.z);
}
";

const VERTEX: &str = "#ifdef GL_ES
precision highp float;
precision mediump int;
#endif

uniform vec4 vs_uniforms_vec4[4];
uniform vec4 posFixup;
const vec4 vs_c4 = vec4(1.0, 0.0, 0.0, 0.0);
vec4 vs_r0;
#define vs_c0 vs_uniforms_vec4[0]
#define vs_c1 vs_uniforms_vec4[1]
#define vs_c2 vs_uniforms_vec4[2]
#define vs_c3 vs_uniforms_vec4[3]
attribute vec4 vs_v0;
#define vs_o0 gl_Position
attribute vec4 vs_v1;
varying vec4 vFrontColor;
#define vs_o1 vFrontColor
attribute vec4 vs_v2;
varying vec4 vTexCoord0;
#define vs_o2 vTexCoord0

void main()
{
	vs_r0 = (vs_v0.xyzx * vs_c4.xxxy) + vs_c4.yyyx;
	vs_o0.x = dot(vs_r0, vs_c0);
	vs_o0.y = dot(vs_r0, vs_c1);
	vs_o0.z = dot(vs_r0, vs_c2);
	vs_o0.w = dot(vs_r0, vs_c3);
	vs_o1 = vs_v1;
	vs_o2.xy = vs_v2.xy;
	gl_Position.y = gl_Position.y * posFixup.y;
	gl_Position.xy += posFixup.zw * gl_Position.ww;
	gl_Position.z = gl_Position.z * 2.0 - gl_Position.w;
}
";

/// Relative addressing through `ps_a0`, a `rep` loop, `if`/`else` and `discard` (pixel).
const CONTROL: &str = "uniform vec4 ps_uniforms_vec4[8];
const ivec4 ps_i0 = ivec4(3, 0, 0, 0);
const vec4 ps_c8 = vec4(0.5, 1.0, 2.0, 0.0);
vec4 ps_r0;
vec4 ps_r1;
ivec4 ps_a0;
#define ps_c0 ps_uniforms_vec4[0]
#define ARRAYBASE_0 2
varying vec4 vTexCoord0;
#define ps_v0 vTexCoord0
#define ps_oC0 gl_FragData[0]
#define ps_oC1 gl_FragData[1]

void main()
{
	ps_r0 = ps_c8.wwww;
	for (int rep0 = 0; rep0 < ps_i0.x; rep0++) {
		ps_r0 = ps_r0 + ps_c0;
	}
	ps_a0 = ivec4(floor(abs(ps_v0.yxzw) + vec4(0.5)) * sign(ps_v0.yxzw));
	ps_r1 = ps_uniforms_vec4[ARRAYBASE_0 + ps_a0.x];
	if (-ps_r1.x != ps_r1.x) {
		ps_r1.xy = ps_r1.yx;
	} else {
		ps_r1 = ((ps_r1.w >= 0.0) ? ps_r1 : ps_c8);
	}
	if (any(lessThan(ps_r1.xyz, vec3(0.0)))) discard;
	ps_oC0 = ps_r0 + ps_r1;
	ps_oC1 = ps_r1;
}
";

fn hints() -> Hints {
    let attribute = |glsl_name: &str, semantic: &str| Attribute {
        glsl_name: glsl_name.into(),
        semantic: semantic.into(),
        index: 0,
    };
    Hints {
        target: "test".into(),
        constants: vec![Constant {
            name: "GammaInverse".into(),
            offset: 0,
            bytes: 16,
            width: 1,
        }],
        samplers: vec![Sampler {
            glsl_name: "ps_s0".into(),
            slot: 3,
            name: "Albedo".into(),
        }],
        attributes: vec![
            attribute("vs_v0", "POSITION"),
            attribute("vs_v1", "COLOR"),
            attribute("vs_v2", "TEXCOORD"),
        ],
    }
}

#[test]
fn a_pixel_program_lifts_validates_and_is_written() {
    let p = lift(GAMMA, Stage::Pixel, &hints()).unwrap();
    p.validate().unwrap();
    let wgsl = emit::wgsl(&p).unwrap();
    assert!(wgsl.contains("textureSample"), "{wgsl}");
    assert!(wgsl.contains("exp2"), "{wgsl}");
    let names: Vec<_> = p
        .interface
        .resources
        .iter()
        .map(|r| r.provenance.name.as_str())
        .collect();
    assert_eq!(names, ["ps_uniforms_vec4", "Albedo", "Albedo (ps_s0)"]);
    let inputs: Vec<_> = p
        .interface
        .inputs
        .iter()
        .map(|v| (v.label.name.as_str(), v.label.index))
        .collect();
    assert_eq!(inputs, [("COLOR", 0), ("TEXCOORD", 0)]);
    assert_eq!(p.interface.outputs.len(), 1);
}

#[test]
fn a_vertex_program_drops_the_clip_space_fix_of_the_api() {
    let p = lift(VERTEX, Stage::Vertex, &hints()).unwrap();
    p.validate().unwrap();
    assert!(p.notes.iter().any(|n| n.contains("posFixup")), "{:?}", p.notes);
    let wgsl = emit::wgsl(&p).unwrap();
    assert!(!wgsl.contains("posFixup"), "{wgsl}");
    let outputs: Vec<_> = p
        .interface
        .outputs
        .iter()
        .map(|v| (v.label.name.as_str(), v.label.index))
        .collect();
    assert_eq!(outputs, [("POSITION", 0), ("COLOR", 0), ("TEXCOORD", 0)]);
    let inputs: Vec<_> = p.interface.inputs.iter().map(|v| v.label.name.as_str()).collect();
    assert_eq!(inputs, ["POSITION", "COLOR", "TEXCOORD"]);
}

#[test]
fn loops_relative_addressing_branches_and_discard_lift() {
    let p = lift(CONTROL, Stage::Pixel, &Hints::default()).unwrap();
    p.validate().unwrap();
    let wgsl = emit::wgsl(&p).unwrap();
    assert!(wgsl.contains("loop"), "{wgsl}");
    assert!(wgsl.contains("discard"), "{wgsl}");
    assert_eq!(p.interface.outputs.len(), 2);
}

#[test]
fn what_the_dialect_does_not_have_is_refused_by_name() {
    let refused = |source: &str, stage| lift(source, stage, &hints()).unwrap_err().detail;
    let with = |body: &str| {
        format!(
            "uniform vec4 ps_uniforms_vec4[1];\nvec4 ps_r0;\n#define ps_oC0 gl_FragColor\nvoid main()\n{{\n{body}\n}}\n"
        )
    };
    assert!(refused(&with("ps_r0 = tanh(ps_r0);"), Stage::Pixel).contains("tanh"));
    assert!(refused(&with("while (true) { }"), Stage::Pixel).contains("while"));
    assert!(refused(&with("ps_oC0 = gl_FragCoord;"), Stage::Pixel).contains("gl_FragCoord"));
    assert!(refused("#version 450\nvoid main() {}", Stage::Pixel).contains("version"));
    assert!(refused(&with("ps_r0 = vec4(1.0);"), Stage::Compute).contains("not in the dialect"));
    // the clip-space fix is dropped only whole
    let vs = VERTEX.replace(
        "gl_Position.xy += posFixup.zw * gl_Position.ww;",
        "gl_Position.xy += posFixup.zw;",
    );
    assert!(refused(&vs, Stage::Vertex).contains("posFixup"));
}
