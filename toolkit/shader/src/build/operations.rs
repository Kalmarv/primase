//! Operations asked of the consumer by calling a function of the module that has no body (see [`crate::ops`]).

use naga::{Expression, FunctionArgument, GlobalVariable, Handle, ImageDimension, Statement, TypeInner};

use super::{Builder, Kind, Val};

impl Builder {
    /// Declare a function with no body, the module's name for an operation a consumer does, that returns a value of
    /// `result`: it returns a zero (see [`crate::ops`]).
    pub fn declare_value_operation(
        &mut self,
        name: &str,
        arguments: Vec<FunctionArgument>,
        result: Handle<naga::Type>,
    ) -> Handle<naga::Function> {
        let mut function = naga::Function {
            name: Some(name.to_owned()),
            arguments,
            result: Some(naga::FunctionResult {
                ty: result,
                binding: None,
            }),
            ..naga::Function::default()
        };
        let zero = function
            .expressions
            .append(Expression::ZeroValue(result), naga::Span::UNDEFINED);
        function
            .body
            .push(Statement::Return { value: Some(zero) }, naga::Span::UNDEFINED);
        self.module.functions.append(function, naga::Span::UNDEFINED)
    }

    /// An atomic operation `op` (one of [`crate::ops::IMAGE_ATOMIC_OPS`]) on one texel of the storage texture `image`,
    /// whose old word is the result: a call of the function [`crate::ops::image_atomic_name`] says, made the first
    /// time it is asked for. `coordinate` is `i32` lanes, `layer` the `i32` array index of an arrayed texture, `value`
    /// a `u32` or an `i32`; the result has the value's kind.
    pub fn image_atomic_fetch(
        &mut self,
        image: Handle<GlobalVariable>,
        op: &str,
        coordinate: Val,
        layer: Option<Handle<Expression>>,
        value: Val,
    ) -> Val {
        let image_type = self.module.global_variables[image].ty;
        let TypeInner::Image { dim, arrayed, .. } = self.module.types[image_type].inner else {
            self.fail("an image atomic on something that is not a texture");
            return value;
        };
        if arrayed != layer.is_some()
            || coordinate.kind != Kind::I
            || !matches!(value.kind, Kind::U | Kind::I)
        {
            self.fail("an image atomic with a coordinate, layer or value of the wrong shape");
            return value;
        }
        let dimension = match dim {
            ImageDimension::D1 => "1d",
            ImageDimension::D2 => "2d",
            ImageDimension::D3 => "3d",
            ImageDimension::Cube => "cube",
        };
        let name = crate::ops::image_atomic_name(op, dimension, arrayed, value.kind == Kind::I);
        let existing = self
            .module
            .functions
            .iter()
            .find(|(_, f)| f.name.as_deref() == Some(name.as_str()))
            .map(|(function, _)| function);
        let function = match existing {
            Some(function) => function,
            None => {
                let coordinate_type = self.ty_of(Kind::I, coordinate.width);
                let layer_type = self.ty_of(Kind::I, 1);
                let value_type = self.ty_of(value.kind, 1);
                let mut arguments = vec![("image", image_type), ("coordinate", coordinate_type)];
                if arrayed {
                    arguments.push(("layer", layer_type));
                }
                arguments.push(("value", value_type));
                let arguments = arguments
                    .into_iter()
                    .map(|(name, ty)| FunctionArgument {
                        name: Some(name.to_owned()),
                        ty,
                        binding: None,
                    })
                    .collect();
                self.declare_value_operation(&name, arguments, value_type)
            }
        };
        let image = self.global_ptr(image);
        let mut arguments = vec![image, coordinate.expr];
        arguments.extend(layer);
        arguments.push(value.expr);
        let result = self.result_expr(Expression::CallResult(function));
        self.push(Statement::Call {
            function,
            arguments,
            result: Some(result),
        });
        Val {
            expr: result,
            kind: value.kind,
            width: 1,
        }
    }
}
