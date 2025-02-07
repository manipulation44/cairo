use casm::casm;
use casm::operand::{ap_cell_ref, CellRef, DerefOrImmediate};
use num_bigint::ToBigInt;
use sierra::extensions::array::ArrayConcreteLibFunc;
use sierra::extensions::felt::FeltOperator;
use sierra::extensions::ConcreteLibFunc;
use sierra::ids::ConcreteTypeId;
use utils::try_extract_matches;

use super::{
    CompiledInvocation, CompiledInvocationBuilder, InvocationError, ReferenceExpressionView,
};
use crate::invocations::ProgramInfo;
use crate::references::{
    BinOpExpression, CellExpression, ReferenceExpression, ReferenceValue, ReferencesError,
};

/// Builds instructions for Sierra array operations.
pub fn build(
    libfunc: &ArrayConcreteLibFunc,
    builder: CompiledInvocationBuilder<'_>,
) -> Result<CompiledInvocation, InvocationError> {
    match libfunc {
        ArrayConcreteLibFunc::New(_) => build_array_new(builder),
        ArrayConcreteLibFunc::Append(_) => build_array_append(builder),
    }
}

/// Handles instruction for creating a new array.
fn build_array_new(
    builder: CompiledInvocationBuilder<'_>,
) -> Result<CompiledInvocation, InvocationError> {
    if !builder.refs.is_empty() {
        return Err(InvocationError::WrongNumberOfArguments {
            expected: 0,
            actual: builder.refs.len(),
        });
    }

    Ok(builder.build(
        casm! {
            %{ memory[ap + 0] = segments.add() %}
            ap += 1;
        }
        .instructions,
        vec![],
        [[ReferenceExpression {
            cells: vec![
                CellExpression::Deref(ap_cell_ref(-1)),
                CellExpression::Deref(ap_cell_ref(-1)),
            ],
        }]
        .into_iter()]
        .into_iter(),
    ))
}

/// Handles instruction for appending an element to an array.
fn build_array_append(
    builder: CompiledInvocationBuilder<'_>,
) -> Result<CompiledInvocation, InvocationError> {
    let (mut array_view, element_to_append) = match builder.refs {
        [
            ReferenceValue { expression: expr_arr, .. },
            ReferenceValue { expression: expr_elem, .. },
        ] => {
            let concrete_array_type = &builder.libfunc.param_signatures()[0].ty;
            let array_view =
                ArrayView::try_get_view(expr_arr, &builder.program_info, concrete_array_type)
                    .map_err(|_| InvocationError::InvalidReferenceExpressionForArgument)?;
            let elem_val = match expr_elem
                .try_unpack_single()
                .map_err(|_| InvocationError::InvalidReferenceExpressionForArgument)?
            {
                CellExpression::Deref(op) => DerefOrImmediate::Deref(op),
                CellExpression::Immediate(op) => DerefOrImmediate::from(op),
                _ => return Err(InvocationError::InvalidReferenceExpressionForArgument),
            };
            (array_view, elem_val)
        }
        refs => {
            return Err(InvocationError::WrongNumberOfArguments {
                expected: 2,
                actual: refs.len(),
            });
        }
    };
    if array_view.end_offset != 0 {
        // TODO(Gil): handle when DoubleDeref will support a BinOp variant, e.g. [[ap+1]+1]
        return Err(InvocationError::NotImplemented(builder.invocation.clone()));
    }
    match element_to_append {
        DerefOrImmediate::Immediate(_) => {
            // TODO(Gil): handle when assertion of immediate to DoubleDeref (e.g. [[ap+0]] = 1)
            // will be supported.
            Err(InvocationError::NotImplemented(builder.invocation.clone()))
        }
        DerefOrImmediate::Deref(op) => {
            let instructions = casm! { op = [[array_view.end]]; }.instructions;
            array_view.end_offset += 1;
            let output_expressions =
                [vec![array_view.to_reference_expression()].into_iter()].into_iter();
            Ok(builder.build(instructions, vec![], output_expressions))
        }
    }
}

/// A struct representing an actual array value in the Sierra program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrayView {
    /// A ref to the cell in which the start of the array address is stored.
    pub start: CellRef,
    /// A ref to the cell in which the last stored end_of_the_array_address is stored.
    /// The end of the array is the next cell to write to (i.e. \[\[end\] + end_offset\] is not
    /// initialized).
    pub end: CellRef,
    /// The number of elements appended to the array since the last store. The real end of the
    /// array is in the address \[end\] + end_offset.
    pub end_offset: u16,
}

impl ReferenceExpressionView for ArrayView {
    type Error = ReferencesError;

    fn try_get_view(
        expr: &ReferenceExpression,
        _program_info: &ProgramInfo<'_>,
        _concrete_type_id: &ConcreteTypeId,
    ) -> Result<Self, Self::Error> {
        if expr.cells.len() != 2 {
            return Err(ReferencesError::InvalidReferenceTypeForArgument);
        };
        let start = try_extract_matches!(expr.cells[0], CellExpression::Deref)
            .ok_or(ReferencesError::InvalidReferenceTypeForArgument)?;
        let (end, end_offset) = match &expr.cells[1] {
            CellExpression::Deref(op) => (*op, 0u16),
            CellExpression::BinOp(binop) => {
                if binop.op != FeltOperator::Add {
                    return Err(ReferencesError::InvalidReferenceTypeForArgument);
                }
                (
                    binop.a,
                    u16::try_from(
                        try_extract_matches!(&binop.b, DerefOrImmediate::Immediate)
                            .ok_or(ReferencesError::InvalidReferenceTypeForArgument)?,
                    )
                    .unwrap(),
                )
            }
            _ => {
                return Err(ReferencesError::InvalidReferenceTypeForArgument);
            }
        };
        Ok(ArrayView { start, end, end_offset })
    }

    fn to_reference_expression(self) -> ReferenceExpression {
        let start_ref = CellExpression::Deref(self.start);
        if self.end_offset == 0 {
            ReferenceExpression { cells: vec![start_ref, CellExpression::Deref(self.end)] }
        } else {
            ReferenceExpression {
                cells: vec![
                    CellExpression::Deref(self.start),
                    CellExpression::BinOp(BinOpExpression {
                        op: FeltOperator::Add,
                        a: self.end,
                        b: DerefOrImmediate::Immediate(self.end_offset.to_bigint().unwrap()),
                    }),
                ],
            }
        }
    }
}
