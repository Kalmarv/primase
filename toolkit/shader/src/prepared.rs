//! A shader and its bound data, independent of a game's binding recipe and of an evaluation's inputs.

use std::collections::BTreeMap;

use crate::{
    Constants, Error, Evaluation, Execution, Grid, Input, Invocation, PixelShader, Raw, Sampler, Texture,
};

/// A pixel program and the resources its caller prepared. Preparation does not execute the program, bake a
/// texture, choose inputs, or interpret outputs. A binding recipe owns those choices and reports them separately.
///
/// The shader stays concrete: DXBC consumers can inspect its instructions, while generic consumers use
/// [`Self::as_ref`]. This type neither translates bytecode nor implies that a resource is GPU-uploadable:
/// [`Texture::Image`] is an opaque CPU sampling implementation; [`Texture::Constant`] states its value directly.
/// Sampler state, addressing, transfer and footprint remain the supplied image implementation's policy.
#[derive(Debug)]
pub struct PreparedProgram<S> {
    shader: S,
    constants: Constants,
    textures: BTreeMap<u32, Texture>,
}

impl<S> PreparedProgram<S> {
    /// Keep exactly the supplied bindings. Missing resources are not invented; evaluation reports them when used.
    pub fn new(shader: S, constants: Constants, textures: BTreeMap<u32, Texture>) -> Self {
        Self {
            shader,
            constants,
            textures,
        }
    }

    pub fn shader(&self) -> &S {
        &self.shader
    }
    pub fn constants(&self) -> &Constants {
        &self.constants
    }
    /// Explicitly update a draw's constants without parsing its shader or preparing its images again.
    pub fn constants_mut(&mut self) -> &mut Constants {
        &mut self.constants
    }
    pub fn textures(&self) -> &BTreeMap<u32, Texture> {
        &self.textures
    }
    pub fn into_parts(self) -> (S, Constants, BTreeMap<u32, Texture>) {
        (self.shader, self.constants, self.textures)
    }
}

impl<S: PixelShader> PreparedProgram<S> {
    pub fn as_ref(&self) -> PreparedProgramRef<'_> {
        PreparedProgramRef::new(&self.shader, &self.constants, &self.textures)
    }
}

/// The same prepared-program contract borrowed from an owner. It does not copy shaders, constants or images.
/// This also supports callers whose shader and material bindings have separate lifetimes or owners.
#[derive(Clone, Copy, Debug)]
pub struct PreparedProgramRef<'a> {
    pub shader: &'a dyn PixelShader,
    pub constants: &'a Constants,
    pub textures: &'a BTreeMap<u32, Texture>,
}

impl<'a> PreparedProgramRef<'a> {
    pub fn new(
        shader: &'a dyn PixelShader,
        constants: &'a Constants,
        textures: &'a BTreeMap<u32, Texture>,
    ) -> Self {
        Self {
            shader,
            constants,
            textures,
        }
    }

    /// Grid evaluation keeps the caller's targets and invocation mode; no G-buffer interpretation is implied.
    pub fn evaluate_raw(
        &self,
        inputs: &BTreeMap<(String, u32), Input>,
        grid: Grid,
        targets: &[(u32, u8)],
        invocation: Invocation,
    ) -> Result<Raw, Error> {
        crate::evaluate_raw(
            self.shader,
            self.constants,
            self.textures,
            inputs,
            grid,
            targets,
            invocation,
        )
    }

    pub fn evaluate(
        &self,
        inputs: &BTreeMap<(String, u32), Input>,
        grid: Grid,
        targets: &[(u32, u8)],
    ) -> Result<Evaluation, Error> {
        crate::evaluate(self.shader, self.constants, self.textures, inputs, grid, targets)
    }

    /// The caller explicitly requests periodic cross-fading; preparation itself never makes a shader periodic.
    pub fn evaluate_periodic(
        &self,
        inputs: &BTreeMap<(String, u32), Input>,
        cells: u32,
        texels: u32,
        targets: &[(u32, u8)],
    ) -> Result<Evaluation, Error> {
        crate::evaluate_periodic(
            self.shader,
            self.constants,
            self.textures,
            inputs,
            cells,
            texels,
            targets,
        )
    }

    /// One register-level invocation. Outputs, written masks and discard status are returned unchanged. There are
    /// no invented normals, UVs or derivative values: the caller supplies the input registers explicitly.
    pub fn execute(&self, inputs: &[[u32; 4]]) -> Result<Execution, Error> {
        let mut sampler = crate::evaluate::Bound(self.textures);
        self.execute_with_sampler(inputs, &mut sampler)
    }

    /// Use an explicitly supplied resource implementation (including loads, storage or atomics). This overrides
    /// the bound texture map for this invocation; it does not change the prepared data.
    pub fn execute_with_sampler(
        &self,
        inputs: &[[u32; 4]],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        let buffers: Vec<_> = self.constants.buffers().collect();
        self.shader.execute(inputs, &buffers, sampler)
    }

    pub fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
    ) -> Result<Execution, Error> {
        let buffers: Vec<_> = self.constants.buffers().collect();
        self.shader.execute_with_derivatives(
            inputs,
            derivatives,
            &buffers,
            &mut crate::evaluate::Bound(self.textures),
        )
    }

    pub fn execute_quad(&self, inputs: [&[[u32; 4]]; 4]) -> Result<[Execution; 4], Error> {
        let buffers: Vec<_> = self.constants.buffers().collect();
        self.shader
            .execute_quad(inputs, &buffers, &mut crate::evaluate::Bound(self.textures))
    }
}
