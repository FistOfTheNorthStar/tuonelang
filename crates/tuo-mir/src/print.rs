//! The MIR pretty-printer behind `tuo debug mir` and the golden test
//! suite.
//!
//! The format is a diagnostic aid, not a stable protocol — but it is
//! **deterministic**: equal MIR renders identically, so golden tests can
//! diff it. Spans are deliberately omitted.

use std::fmt::Write as _;

use tuo_resolve::Resolution;

use crate::mir::{
    AggregateKind, Arg, BasicBlock, Callee, Const, Function, Operand, PassMode, Place, Program,
    Projection, Rvalue, Statement, Terminator,
};

/// Render a whole lowered program.
#[must_use]
pub fn render(program: &Program, resolution: &Resolution) -> String {
    let mut out = String::new();
    for skipped in &program.skipped {
        let _ = writeln!(
            out,
            "// not lowered: fn {}: {}",
            skipped.name, skipped.reason
        );
    }
    if !program.skipped.is_empty() && !program.functions.is_empty() {
        out.push('\n');
    }
    for (index, function) in program.functions.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        render_function(&mut out, function, resolution);
    }
    out
}

/// Render one function.
pub(crate) fn render_function(out: &mut String, function: &Function, resolution: &Resolution) {
    let params: Vec<String> = function
        .params
        .iter()
        .enumerate()
        .map(|(index, mode)| {
            let local = &function.locals[index];
            let mode = match mode {
                PassMode::Value => "take",
                PassMode::Borrow => "in",
                PassMode::BorrowMut => "mut",
            };
            format!("_{index}: {mode} {}", local.ty.render(resolution))
        })
        .collect();
    let _ = writeln!(
        out,
        "fn {}({}) -> {} {{",
        function.name,
        params.join(", "),
        function.ret.render(resolution)
    );
    for (index, local) in function
        .locals
        .iter()
        .enumerate()
        .skip(function.params.len())
    {
        let name = local
            .name
            .as_ref()
            .map(|name| format!(" // {name}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "    let _{index}: {};{name}",
            local.ty.render(resolution)
        );
    }
    for (index, block) in function.blocks.iter().enumerate() {
        let _ = writeln!(out, "bb{index}:");
        render_block(out, block, resolution);
    }
    out.push_str("}\n");
}

fn render_block(out: &mut String, block: &BasicBlock, resolution: &Resolution) {
    for statement in &block.statements {
        let _ = writeln!(out, "    {}", render_statement(statement, resolution));
    }
    let _ = writeln!(out, "    {}", render_terminator(&block.terminator));
}

fn render_statement(statement: &Statement, resolution: &Resolution) -> String {
    match statement {
        Statement::Assign { place, rvalue } => {
            format!(
                "{} = {}",
                render_place(place),
                render_rvalue(rvalue, resolution)
            )
        }
        Statement::Call { dest, callee, args } => {
            let args: Vec<String> = args.iter().map(render_arg).collect();
            let target = match callee {
                Callee::Direct(symbol) => resolution.symbol(*symbol).name.clone(),
                Callee::Indirect(operand) => render_operand(operand),
            };
            format!(
                "{} = call {}({})",
                render_place(dest),
                target,
                args.join(", ")
            )
        }
        Statement::Effect { op, args, dest } => {
            let args: Vec<String> = args.iter().map(render_arg).collect();
            format!(
                "{} = effect {}({})",
                render_place(dest),
                op.name(),
                args.join(", ")
            )
        }
        Statement::HeapMutate {
            op,
            target,
            args,
            dest,
        } => {
            let mut parts = vec![format!("borrow mut {}", render_place(target))];
            parts.extend(args.iter().map(render_operand));
            format!(
                "{} = heap_mut {}({})",
                render_place(dest),
                op.name(),
                parts.join(", ")
            )
        }
        Statement::Drop { place } => format!("drop {}", render_place(place)),
    }
}

fn render_arg(arg: &Arg) -> String {
    match arg {
        Arg::Value(operand) => render_operand(operand),
        Arg::Borrow(place) => format!("borrow {}", render_place(place)),
        Arg::BorrowMut(place) => format!("borrow mut {}", render_place(place)),
    }
}

fn render_rvalue(rvalue: &Rvalue, resolution: &Resolution) -> String {
    match rvalue {
        Rvalue::Use(operand) => render_operand(operand),
        Rvalue::Unary { op, operand } => {
            let name = match op {
                crate::mir::UnOp::Neg => "neg",
                crate::mir::UnOp::Not => "not",
                crate::mir::UnOp::BitNot => "bitnot",
            };
            format!("{name}({})", render_operand(operand))
        }
        Rvalue::Binary { op, lhs, rhs } => {
            let name = match op {
                crate::mir::BinOp::Add => "add",
                crate::mir::BinOp::Sub => "sub",
                crate::mir::BinOp::Mul => "mul",
                crate::mir::BinOp::Div => "div",
                crate::mir::BinOp::Rem => "rem",
                crate::mir::BinOp::Eq => "eq",
                crate::mir::BinOp::Ne => "ne",
                crate::mir::BinOp::Lt => "lt",
                crate::mir::BinOp::Le => "le",
                crate::mir::BinOp::Gt => "gt",
                crate::mir::BinOp::Ge => "ge",
                crate::mir::BinOp::BitAnd => "bitand",
                crate::mir::BinOp::BitOr => "bitor",
                crate::mir::BinOp::BitXor => "bitxor",
                crate::mir::BinOp::Shl => "shl",
                crate::mir::BinOp::Shr => "shr",
            };
            format!("{name}({}, {})", render_operand(lhs), render_operand(rhs))
        }
        Rvalue::Cast {
            kind,
            operand,
            to: _,
        } => {
            let name = match kind {
                crate::mir::CastKind::IntToInt => "int_to_int",
                crate::mir::CastKind::IntToFloat => "int_to_float",
                crate::mir::CastKind::FloatToInt => "float_to_int",
                crate::mir::CastKind::FloatToFloat => "float_to_float",
            };
            format!("cast {name}({})", render_operand(operand))
        }
        Rvalue::Aggregate { kind, fields } => {
            let fields: Vec<String> = fields.iter().map(render_operand).collect();
            match kind {
                AggregateKind::Adt { ty, variant } => {
                    format!(
                        "{} @{variant} {{ {} }}",
                        ty.render(resolution),
                        fields.join(", ")
                    )
                }
                AggregateKind::Range => format!("range({})", fields.join(", ")),
                AggregateKind::Array { element, len } => format!(
                    "[{}; {len}] [ {} ]",
                    element.render(resolution),
                    fields.join(", ")
                ),
            }
        }
        Rvalue::Discriminant(place) => format!("discriminant({})", render_place(place)),
        Rvalue::Len(place) => format!("len({})", render_place(place)),
        Rvalue::StrOp { op, args } => {
            let args: Vec<String> = args.iter().map(render_operand).collect();
            format!("str_{}({})", op.name(), args.join(", "))
        }
        Rvalue::HeapOp { op, subject, args } => {
            let mut parts = Vec::new();
            if let Some(place) = subject {
                parts.push(format!("borrow {}", render_place(place)));
            }
            parts.extend(args.iter().map(render_operand));
            format!("heap_{}({})", op.name(), parts.join(", "))
        }
    }
}

fn render_operand(operand: &Operand) -> String {
    match operand {
        Operand::Copy(place) => format!("copy {}", render_place(place)),
        Operand::Move(place) => format!("move {}", render_place(place)),
        Operand::Const(constant) => format!("const {}", render_const(constant)),
    }
}

fn render_const(constant: &Const) -> String {
    match constant {
        Const::Unit => "()".to_owned(),
        Const::Bool(value) => value.to_string(),
        Const::Int(value, kind) => format!("{value}_{}", kind.name().to_lowercase()),
        Const::Float(value, kind) => format!("{value:?}_{}", kind.name().to_lowercase()),
        Const::Char(value) => format!("{value:?}"),
        Const::Str(value) => format!("{value:?}"),
        // A function value (ADR-0008 Tier 1): rendered by its symbol id, since
        // this dev-tool dump threads no resolution through operands.
        Const::Fn(symbol) => format!("fn {symbol}"),
    }
}

fn render_place(place: &Place) -> String {
    let mut out = format!("_{}", place.local.0);
    for projection in &place.projection {
        match projection {
            Projection::Field(index) => {
                let _ = write!(out, ".{index}");
            }
            Projection::VariantField { variant, field } => {
                let _ = write!(out, ".v{variant}.{field}");
            }
            Projection::Index(local) => {
                let _ = write!(out, "[_{}]", local.0);
            }
        }
    }
    out
}

fn render_terminator(terminator: &Terminator) -> String {
    match terminator {
        Terminator::Return(operand) => format!("return {}", render_operand(operand)),
        Terminator::Goto(block) => format!("goto bb{}", block.0),
        Terminator::Branch {
            cond,
            then_block,
            else_block,
        } => format!(
            "branch {} ? bb{} : bb{}",
            render_operand(cond),
            then_block.0,
            else_block.0
        ),
        Terminator::Switch {
            discr,
            arms,
            otherwise,
        } => {
            let arms: Vec<String> = arms
                .iter()
                .map(|(value, block)| format!("{value} => bb{}", block.0))
                .collect();
            format!(
                "switch {} {{ {}, otherwise => bb{} }}",
                render_operand(discr),
                arms.join(", "),
                otherwise.0
            )
        }
        Terminator::Assert { cond, trap, target } => format!(
            "assert {} [{}] -> bb{}",
            render_operand(cond),
            trap.label(),
            target.0
        ),
        Terminator::Trap(trap) => format!("trap [{}]", trap.label()),
    }
}
