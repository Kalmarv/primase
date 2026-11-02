//! A Blender shader node graph as data, and the graphs of core's materials.
//!
//! A [`Graph`] is the node tree of one Blender material: each node by its Blender type (`bl_idname`), the properties
//! set on it in order, the values of its unlinked inputs and its links, every socket by its identifier. The save stage
//! builds it as it is (`build.py` interprets nothing), so what a material draws is decided here, where it is tested.
//!
//! Values are built through [`Float`] and [`Vec3`], which are a constant or a node's output: an operation of constants
//! is folded here in f32 (the same arithmetic Cycles' math nodes do), and multiplying by 1 or adding 0 makes no node.
//!
//! - [`texture`]: a texture slot sampled as core states it (UV set, transforms, wrap, filters).
//! - [`pbr`]: a `pbr.material`'s surface, alpha and sidedness, and the material's outputs.
//! - [`layered`]: a `core.layered` blend of `pbr.material` layers, drawn with its host's alpha and sidedness.

pub mod layered;
pub mod pbr;
pub mod texture;

use serde::Serialize;
use serde_json::{Value, json};

/// One output socket of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Socket {
    pub node: usize,
    /// The output's identifier.
    pub output: &'static str,
}

/// A scalar: a constant, or a node's output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Float {
    Const(f32),
    Out(Socket),
}

/// A colour or vector of three: a constant, or a node's output (a colour output's alpha is not part of it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vec3 {
    Const([f32; 3]),
    Out(Socket),
}

impl From<f32> for Float {
    fn from(x: f32) -> Self {
        Float::Const(x)
    }
}

impl From<Socket> for Float {
    fn from(s: Socket) -> Self {
        Float::Out(s)
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(x: [f32; 3]) -> Self {
        Vec3::Const(x)
    }
}

impl From<Socket> for Vec3 {
    fn from(s: Socket) -> Self {
        Vec3::Out(s)
    }
}

/// A node.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    /// Blender's node type (`bl_idname`).
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Properties, set in this order (a mix node's `data_type` before what its sockets depend on).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<(&'static str, Value)>,
    /// The values of its unlinked inputs, by identifier.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<(&'static str, Value)>,
    /// For an image texture node: its image, by its index in the scene's image list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<usize>,
}

/// A link from a node's output to another's input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Link {
    pub from: usize,
    pub output: &'static str,
    pub to: usize,
    pub input: &'static str,
}

/// A material's settings outside its node tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    /// `surface_render_method`: `BLENDED` for a blended material, `DITHERED` otherwise (EEVEE's; Cycles blends by
    /// the surface's alpha whatever it is).
    pub surface_render_method: &'static str,
    /// `use_backface_culling` (EEVEE and the viewport; the graph itself makes back faces transparent for Cycles).
    pub use_backface_culling: bool,
}

/// One Blender material's node tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}

/// The names of the render passes ([AOVs](https://docs.blender.org/manual/en/latest/render/layers/passes.html)) every
/// material writes, each the first surface the camera sees with coverage ([`PASS_ALPHA_THRESHOLD`]).
pub mod aov {
    /// The surface's base colour (linear, before lighting): `pbr.material`'s base colour times its texture and the
    /// vertex colour it takes, or what a blend or detail composite makes of it.
    pub const BASE_COLOR: &str = "base_color";
    /// The surface's emitted radiance (linear): emissive colour times its texture and strength.
    pub const EMISSION: &str = "emission";
    /// 1 where a surface covers the pixel: its alpha (as its mode uses it) at least [`super::PASS_ALPHA_THRESHOLD`],
    /// and front-facing unless double-sided.
    pub const COVERAGE: &str = "coverage";
}

/// The alpha from which Cycles writes a surface's passes (the view layer's `pass_alpha_threshold`) rather than let the
/// camera ray pass on. Cycles adds the AOV writes of every surface the ray passes through, so each material writes
/// its passes times its coverage at this same threshold: what a pixel holds is the first covering surface's.
pub const PASS_ALPHA_THRESHOLD: f32 = 0.5;

impl Graph {
    pub(crate) fn new() -> Self {
        Graph {
            nodes: Vec::new(),
            links: Vec::new(),
        }
    }

    /// A node of `kind` with `props`.
    pub(crate) fn node(&mut self, kind: &'static str, props: &[(&'static str, Value)]) -> usize {
        self.nodes.push(Node {
            kind,
            props: props.to_vec(),
            inputs: Vec::new(),
            image: None,
        });
        self.nodes.len() - 1
    }

    pub(crate) fn link(&mut self, from: Socket, to: usize, input: &'static str) {
        self.links.push(Link {
            from: from.node,
            output: from.output,
            to,
            input,
        });
    }

    fn set(&mut self, node: usize, input: &'static str, value: Value) {
        self.nodes[node].inputs.push((input, value));
    }

    /// Input `input` of `node`, a scalar socket.
    pub(crate) fn float_in(&mut self, node: usize, input: &'static str, v: Float) {
        match v {
            Float::Const(x) => self.set(node, input, json!(x)),
            Float::Out(s) => self.link(s, node, input),
        }
    }

    /// Input `input` of `node`, a colour socket (alpha 1 for a constant).
    pub(crate) fn color_in(&mut self, node: usize, input: &'static str, v: Vec3) {
        match v {
            Vec3::Const([r, g, b]) => self.set(node, input, json!([r, g, b, 1.0])),
            Vec3::Out(s) => self.link(s, node, input),
        }
    }

    /// Input `input` of `node`, a vector socket.
    pub(crate) fn vector_in(&mut self, node: usize, input: &'static str, v: Vec3) {
        match v {
            Vec3::Const(x) => self.set(node, input, json!(x)),
            Vec3::Out(s) => self.link(s, node, input),
        }
    }

    /// A Math node of `operation` (unclamped) on `inputs` (`Value`, `Value_001`, `Value_002`).
    fn math_node(&mut self, operation: &str, inputs: &[Float]) -> Float {
        let n = self.node(
            "ShaderNodeMath",
            &[("operation", json!(operation)), ("use_clamp", json!(false))],
        );
        for (v, id) in inputs.iter().zip(["Value", "Value_001", "Value_002"]) {
            self.float_in(n, id, *v);
        }
        Float::Out(Socket {
            node: n,
            output: "Value",
        })
    }

    pub(crate) fn mul(&mut self, a: Float, b: Float) -> Float {
        match (a, b) {
            (Float::Const(x), Float::Const(y)) => Float::Const(x * y),
            (Float::Const(1.0), v) | (v, Float::Const(1.0)) => v,
            _ => self.math_node("MULTIPLY", &[a, b]),
        }
    }

    pub(crate) fn add(&mut self, a: Float, b: Float) -> Float {
        match (a, b) {
            (Float::Const(x), Float::Const(y)) => Float::Const(x + y),
            (Float::Const(0.0), v) | (v, Float::Const(0.0)) => v,
            _ => self.math_node("ADD", &[a, b]),
        }
    }

    pub(crate) fn sub(&mut self, a: Float, b: Float) -> Float {
        match (a, b) {
            (Float::Const(x), Float::Const(y)) => Float::Const(x - y),
            (v, Float::Const(0.0)) => v,
            _ => self.math_node("SUBTRACT", &[a, b]),
        }
    }

    /// 1 where `a < b`, else 0 (Cycles' `LESS_THAN`).
    pub(crate) fn less_than(&mut self, a: Float, b: Float) -> Float {
        match (a, b) {
            (Float::Const(x), Float::Const(y)) => Float::Const(f32::from(u8::from(x < y))),
            _ => self.math_node("LESS_THAN", &[a, b]),
        }
    }

    /// 1 where `a >= b`, else 0: `1 - (a < b)`.
    pub(crate) fn at_least(&mut self, a: Float, b: Float) -> Float {
        let below = self.less_than(a, b);
        self.sub(Float::Const(1.0), below)
    }

    /// `v` held within 0 to 1 (a Clamp node).
    pub(crate) fn clamp01(&mut self, v: Float) -> Float {
        match v {
            Float::Const(x) => Float::Const(x.clamp(0.0, 1.0)),
            Float::Out(_) => {
                let n = self.node("ShaderNodeClamp", &[("clamp_type", json!("MINMAX"))]);
                self.float_in(n, "Value", v);
                self.float_in(n, "Min", Float::Const(0.0));
                self.float_in(n, "Max", Float::Const(1.0));
                Float::Out(Socket {
                    node: n,
                    output: "Result",
                })
            }
        }
    }

    /// A Math node of a unary or binary `operation` that is not folded.
    pub(crate) fn math(&mut self, operation: &str, inputs: &[Float]) -> Float {
        self.math_node(operation, inputs)
    }

    /// A Vector Math node of `operation` on `inputs` (`Vector`, `Vector_001`, `Vector_002`), its `Vector` output.
    pub(crate) fn vector_math(&mut self, operation: &str, inputs: &[Vec3]) -> Vec3 {
        let n = self.node("ShaderNodeVectorMath", &[("operation", json!(operation))]);
        for (v, id) in inputs.iter().zip(["Vector", "Vector_001", "Vector_002"]) {
            self.vector_in(n, id, *v);
        }
        Vec3::Out(Socket {
            node: n,
            output: "Vector",
        })
    }

    /// `a + b`.
    pub(crate) fn vadd(&mut self, a: Vec3, b: Vec3) -> Vec3 {
        match (a, b) {
            (Vec3::Const(x), Vec3::Const(y)) => Vec3::Const([x[0] + y[0], x[1] + y[1], x[2] + y[2]]),
            (Vec3::Const([0.0, 0.0, 0.0]), v) | (v, Vec3::Const([0.0, 0.0, 0.0])) => v,
            _ => self.vector_math("ADD", &[a, b]),
        }
    }

    /// The length of `v` (a Vector Math node's `Value`).
    pub(crate) fn vlength(&mut self, v: Vec3) -> Float {
        if let Vec3::Const(x) = v {
            return Float::Const(x.iter().map(|c| c * c).sum::<f32>().sqrt());
        }
        let n = self.node("ShaderNodeVectorMath", &[("operation", json!("LENGTH"))]);
        self.vector_in(n, "Vector", v);
        Float::Out(Socket {
            node: n,
            output: "Value",
        })
    }

    /// `a * b` per component.
    pub(crate) fn vmul(&mut self, a: Vec3, b: Vec3) -> Vec3 {
        match (a, b) {
            (Vec3::Const(x), Vec3::Const(y)) => Vec3::Const([x[0] * y[0], x[1] * y[1], x[2] * y[2]]),
            (Vec3::Const([1.0, 1.0, 1.0]), v) | (v, Vec3::Const([1.0, 1.0, 1.0])) => v,
            _ => self.vector_math("MULTIPLY", &[a, b]),
        }
    }

    /// `a * s`.
    pub(crate) fn vscale(&mut self, a: Vec3, s: Float) -> Vec3 {
        match (a, s) {
            (Vec3::Const(x), Float::Const(k)) => Vec3::Const(x.map(|c| c * k)),
            (v, Float::Const(1.0)) => v,
            _ => {
                let n = self.node("ShaderNodeVectorMath", &[("operation", json!("SCALE"))]);
                self.vector_in(n, "Vector", a);
                self.float_in(n, "Scale", s);
                Vec3::Out(Socket {
                    node: n,
                    output: "Vector",
                })
            }
        }
    }

    /// The three components of `v` (Separate XYZ, which reads a colour's R, G and B).
    pub(crate) fn separate(&mut self, v: Vec3) -> [Float; 3] {
        match v {
            Vec3::Const(x) => x.map(Float::Const),
            Vec3::Out(_) => {
                let n = self.node("ShaderNodeSeparateXYZ", &[]);
                self.vector_in(n, "Vector", v);
                ["X", "Y", "Z"].map(|output| Float::Out(Socket { node: n, output }))
            }
        }
    }

    /// The vector of three components.
    pub(crate) fn combine(&mut self, c: [Float; 3]) -> Vec3 {
        if let [Float::Const(x), Float::Const(y), Float::Const(z)] = c {
            return Vec3::Const([x, y, z]);
        }
        let n = self.node("ShaderNodeCombineXYZ", &[]);
        for (v, id) in c.into_iter().zip(["X", "Y", "Z"]) {
            self.float_in(n, id, v);
        }
        Vec3::Out(Socket {
            node: n,
            output: "Vector",
        })
    }

    /// A geometry attribute by name: its colour and alpha.
    pub(crate) fn attribute(&mut self, name: &str) -> (Vec3, Float) {
        let n = self.node(
            "ShaderNodeAttribute",
            &[
                ("attribute_type", json!("GEOMETRY")),
                ("attribute_name", json!(name)),
            ],
        );
        (
            Vec3::Out(Socket {
                node: n,
                output: "Color",
            }),
            Float::Out(Socket {
                node: n,
                output: "Alpha",
            }),
        )
    }

    /// An output of a node with no inputs that matter here (Geometry, Light Path).
    pub(crate) fn input_node(&mut self, kind: &'static str, output: &'static str) -> Socket {
        let node = self.node(kind, &[]);
        Socket { node, output }
    }

    /// A shader node of `kind` with its `inputs` set; its output `output`.
    pub(crate) fn shader(
        &mut self,
        kind: &'static str,
        output: &'static str,
        floats: &[(&'static str, Float)],
        colors: &[(&'static str, Vec3)],
    ) -> Socket {
        let node = self.node(kind, &[]);
        for (id, v) in floats {
            self.float_in(node, id, *v);
        }
        for (id, v) in colors {
            self.color_in(node, id, *v);
        }
        Socket { node, output }
    }

    /// A Mix Shader of `a` and `b` by `fac` (0: `a`).
    pub(crate) fn mix_shader(&mut self, fac: Float, a: Socket, b: Socket) -> Socket {
        let node = self.node("ShaderNodeMixShader", &[]);
        self.float_in(node, "Fac", fac);
        self.link(a, node, "Shader");
        self.link(b, node, "Shader_001");
        Socket {
            node,
            output: "Shader",
        }
    }

    /// An AOV output of `name` taking a colour.
    pub(crate) fn aov_color(&mut self, name: &str, v: Vec3) {
        let n = self.node("ShaderNodeOutputAOV", &[("aov_name", json!(name))]);
        self.color_in(n, "Color", v);
    }

    /// An AOV output of `name` taking a value.
    pub(crate) fn aov_value(&mut self, name: &str, v: Float) {
        let n = self.node("ShaderNodeOutputAOV", &[("aov_name", json!(name))]);
        self.float_in(n, "Value", v);
    }

    /// The material output drawing `surface` in every engine.
    pub(crate) fn output(&mut self, surface: Socket) {
        let n = self.node("ShaderNodeOutputMaterial", &[("target", json!("ALL"))]);
        self.link(surface, n, "Surface");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Operations of constants are folded in f32 and make no node; an identity operand makes none either.
    #[test]
    fn constants_fold_and_identities_make_no_node() {
        let mut g = Graph::new();
        assert_eq!(g.mul(0.5.into(), 0.25.into()), Float::Const(0.125));
        assert_eq!(g.at_least(0.5.into(), 0.5.into()), Float::Const(1.0));
        assert_eq!(g.at_least(0.25.into(), 0.5.into()), Float::Const(0.0));
        assert_eq!(
            g.vmul([0.5, 1.0, 2.0].into(), [2.0, 3.0, 0.25].into()),
            Vec3::Const([1.0, 3.0, 0.5])
        );
        assert!(g.nodes.is_empty());
        let (c, a) = g.attribute("COLOR_0");
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.vmul(c, [1.0; 3].into()), c);
        assert_eq!(g.mul(1.0.into(), a), a);
        assert_eq!(g.sub(a, 0.0.into()), a);
        assert_eq!(g.vscale(c, 1.0.into()), c);
        assert_eq!(g.nodes.len(), 1);
        // a node's operand makes one node, its inputs set and linked by identifier
        let Float::Out(s) = g.mul(a, 0.5.into()) else {
            panic!("not a node")
        };
        assert_eq!(g.nodes[s.node].inputs, [("Value_001", json!(0.5))]);
        assert_eq!(
            g.links,
            [Link {
                from: 0,
                output: "Alpha",
                to: s.node,
                input: "Value"
            }]
        );
    }
}
