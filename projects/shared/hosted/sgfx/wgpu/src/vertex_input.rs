//! Vertex fetch formats without a direct WebGPU representation.
//!
//! Packed SNORM data stays in its original vertex buffer. A wrapper around the
//! selected vertex entry point performs signed extraction and normalization.
use super::*;
use naga::{Expression as E, Handle, ScalarKind, Span, Type, TypeInner};

pub(super) fn lower_packed_inputs(
    source: &ir::ShaderSource,
    name: &str,
    locations: &[u32],
) -> Result<naga::Module> {
    let error =
        |error: &dyn std::fmt::Debug| Error::Validation(format!("packed vertex input: {error:?}"));
    let mut module = match source {
        ir::ShaderSource::Wgsl(source) => {
            naga::front::wgsl::parse_str(source).map_err(|e| error(&e))?
        }
        ir::ShaderSource::SpirV(words) => naga::front::spv::Frontend::new(
            words.iter().copied(),
            &naga::front::spv::Options {
                adjust_coordinate_space: false,
                ..Default::default()
            },
        )
        .parse()
        .map_err(|e| error(&e))?,
    };
    let index = module
        .entry_points
        .iter()
        .position(|entry| entry.stage == naga::ShaderStage::Vertex && entry.name == name)
        .ok_or(Error::InvalidState)?;
    let mut inner = std::mem::take(&mut module.entry_points[index].function);
    let mut wrapper = naga::Function {
        arguments: inner.arguments.clone(),
        result: inner.result.clone(),
        ..Default::default()
    };
    for arg in &mut wrapper.arguments {
        arg.ty = input_type(&mut module.types, arg.ty, arg.binding.as_ref(), locations)?;
    }
    let mut args = Vec::new();
    for (index, (input, original)) in wrapper
        .arguments
        .clone()
        .iter()
        .zip(&inner.arguments)
        .enumerate()
    {
        let value = expression(&mut wrapper, E::FunctionArgument(index as u32));
        args.push(input_value(
            &module.types,
            original.ty,
            input.ty,
            value,
            &mut wrapper,
        )?);
    }
    inner.name = Some(format!("sgfx_vertex_fetch_{}", module.functions.len()));
    for arg in &mut inner.arguments {
        arg.binding = None;
    }
    if let Some(result) = &mut inner.result {
        result.binding = None;
    }
    let function = module.functions.append(inner, Span::UNDEFINED);
    let result = wrapper.result.as_ref().map(|_| {
        wrapper
            .expressions
            .append(E::CallResult(function), Span::UNDEFINED)
    });
    wrapper.body.push(
        naga::Statement::Call {
            function,
            arguments: args,
            result,
        },
        Span::UNDEFINED,
    );
    wrapper
        .body
        .push(naga::Statement::Return { value: result }, Span::UNDEFINED);
    module.entry_points[index].function = wrapper;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::PUSH_CONSTANT,
    )
    .validate(&module)
    .map_err(|e| error(&e))?;
    Ok(module)
}

fn input_type(
    types: &mut naga::UniqueArena<Type>,
    original: Handle<Type>,
    binding: Option<&naga::Binding>,
    locations: &[u32],
) -> Result<Handle<Type>> {
    if let Some(naga::Binding::Location { location, .. }) = binding
        && locations.contains(location)
    {
        if !matches!(
            types[original].inner,
            TypeInner::Vector {
                scalar: naga::Scalar {
                    kind: ScalarKind::Float,
                    width: 4
                },
                ..
            }
        ) {
            return Err(Error::Unsupported(UnsupportedFeature::Pipeline));
        }
        return Ok(types.insert(
            Type {
                name: None,
                inner: TypeInner::Scalar(naga::Scalar::U32),
            },
            Span::UNDEFINED,
        ));
    }
    if let TypeInner::Struct { mut members, span } = types[original].inner.clone() {
        let mut changed = false;
        for member in &mut members {
            let ty = input_type(types, member.ty, member.binding.as_ref(), locations)?;
            changed |= ty != member.ty;
            member.ty = ty;
        }
        if changed {
            return Ok(types.insert(
                Type {
                    name: None,
                    inner: TypeInner::Struct { members, span },
                },
                Span::UNDEFINED,
            ));
        }
    }
    Ok(original)
}

fn expression(function: &mut naga::Function, value: E) -> Handle<E> {
    let emit = !value.needs_pre_emit();
    let handle = function.expressions.append(value, Span::UNDEFINED);
    if emit {
        function.body.push(
            naga::Statement::Emit(naga::Range::new_from_bounds(handle, handle)),
            Span::UNDEFINED,
        );
    }
    handle
}

fn input_value(
    types: &naga::UniqueArena<Type>,
    original: Handle<Type>,
    input: Handle<Type>,
    value: Handle<E>,
    function: &mut naga::Function,
) -> Result<Handle<E>> {
    if original == input {
        return Ok(value);
    }
    if let (
        TypeInner::Struct { members, .. },
        TypeInner::Struct {
            members: inputs, ..
        },
    ) = (&types[original].inner, &types[input].inner)
    {
        let mut components = Vec::new();
        for (index, (member, input)) in members.iter().zip(inputs).enumerate() {
            let member_value = expression(
                function,
                E::AccessIndex {
                    base: value,
                    index: index as u32,
                },
            );
            components.push(input_value(
                types,
                member.ty,
                input.ty,
                member_value,
                function,
            )?);
        }
        return Ok(expression(
            function,
            E::Compose {
                ty: original,
                components,
            },
        ));
    }
    let TypeInner::Vector { size, .. } = types[original].inner else {
        return Err(Error::InvalidState);
    };
    let signed = expression(
        function,
        E::As {
            expr: value,
            kind: ScalarKind::Sint,
            convert: None,
        },
    );
    let mut components = Vec::new();
    for (offset, width) in [(0, 10), (10, 10), (20, 10), (30, 2)]
        .into_iter()
        .take(size as usize)
    {
        let left = expression(
            function,
            E::Literal(naga::Literal::U32(32 - offset - width)),
        );
        let right = expression(function, E::Literal(naga::Literal::U32(32 - width)));
        let shifted = expression(
            function,
            E::Binary {
                op: naga::BinaryOperator::ShiftLeft,
                left: signed,
                right: left,
            },
        );
        let extended = expression(
            function,
            E::Binary {
                op: naga::BinaryOperator::ShiftRight,
                left: shifted,
                right,
            },
        );
        let float = expression(
            function,
            E::As {
                expr: extended,
                kind: ScalarKind::Float,
                convert: Some(4),
            },
        );
        let divisor = expression(
            function,
            E::Literal(naga::Literal::F32(((1u32 << (width - 1)) - 1) as f32)),
        );
        let divided = expression(
            function,
            E::Binary {
                op: naga::BinaryOperator::Divide,
                left: float,
                right: divisor,
            },
        );
        let min = expression(function, E::Literal(naga::Literal::F32(-1.0)));
        components.push(expression(
            function,
            E::Math {
                fun: naga::MathFunction::Max,
                arg: divided,
                arg1: Some(min),
                arg2: None,
                arg3: None,
            },
        ));
    }
    Ok(expression(
        function,
        E::Compose {
            ty: original,
            components,
        },
    ))
}
