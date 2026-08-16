//! HIR pretty-printing for compiler development.
//!
//! [`render`] produces an indented, line-per-node dump of the lowered
//! program. It shows resolved names as `name@symN` (the declared name from
//! the symbol table plus the stable ID) and deliberately omits spans, so
//! canonically equivalent programs render identically — the property the
//! HIR test suite is built on. The format is a developer diagnostic, not a
//! stable protocol.

use std::fmt::Write as _;

use tuo_resolve::{Resolution, SymbolId};

use crate::hir::{
    Arm, BindingDef, Block, ConstDef, EnumDef, Expr, ExprKind, Field, Function, Hir, ImplDef,
    InterfaceDef, Item, Lit, Pat, PatKind, Res, SpecDef, SpecStmt, Stmt, StmtKind, StructDef, Ty,
    TyKind, TypeParam, Variant,
};

/// Render `hir` as an indented development dump, resolving symbol names
/// through `resolution`.
#[must_use]
pub fn render(hir: &Hir, resolution: &Resolution) -> String {
    let mut printer = Printer {
        resolution,
        out: String::new(),
        depth: 0,
    };
    for item in &hir.items {
        printer.item(item);
    }
    printer.out
}

struct Printer<'a> {
    resolution: &'a Resolution,
    out: String,
    depth: usize,
}

impl Printer<'_> {
    /// Emit one indented line.
    fn line(&mut self, text: &str) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// Emit a line, then run `body` one level deeper.
    fn nest(&mut self, text: &str, body: impl FnOnce(&mut Self)) {
        self.line(text);
        self.depth += 1;
        body(self);
        self.depth -= 1;
    }

    /// `name@symN` for a symbol, or the builtin/`Self`/error spelling.
    fn res(&self, res: &Res) -> String {
        match res {
            Res::Symbol(id) => self.symbol(*id),
            Res::Builtin(name) => name.clone(),
            Res::SelfType => "Self".to_owned(),
            Res::Err => "{err}".to_owned(),
        }
    }

    fn symbol(&self, id: SymbolId) -> String {
        format!("{}@{id}", self.resolution.symbol(id).name)
    }

    // ------------------------------------------------------------------
    // Items
    // ------------------------------------------------------------------

    fn item(&mut self, item: &Item) {
        match item {
            Item::Fn(function) => self.function("fn", function),
            Item::Struct(def) => self.struct_def(def),
            Item::Enum(def) => self.enum_def(def),
            Item::Interface(def) => self.interface_def(def),
            Item::Impl(def) => self.impl_def(def),
            Item::Const(def) => self.const_def(def),
            Item::Spec(def) => self.spec_def(def),
        }
    }

    fn function(&mut self, keyword: &str, function: &Function) {
        let header = format!("{keyword} {}", self.res(&function.symbol));
        self.nest(&header, |printer| {
            printer.type_params(&function.type_params);
            for param in &function.params {
                let mut line = format!(
                    "param {} {}",
                    printer.res(&param.symbol),
                    param.mode.keyword()
                );
                if param.is_receiver {
                    line.push_str(" receiver");
                }
                printer.nest(&line, |printer| {
                    if let Some(ty) = &param.ty {
                        printer.ty(ty);
                    }
                });
            }
            printer.nest("ret", |printer| printer.ty(&function.ret));
            if let Some(body) = &function.body {
                printer.nest("body", |printer| printer.block(body));
            }
        });
    }

    fn type_params(&mut self, params: &[TypeParam]) {
        for param in params {
            let mut line = format!("type-param {}", self.res(&param.symbol));
            if !param.bounds.is_empty() {
                let bounds: Vec<String> =
                    param.bounds.iter().map(|bound| self.res(bound)).collect();
                write!(line, ": {}", bounds.join(" + ")).expect("write to String");
            }
            self.line(&line);
        }
    }

    fn fields(&mut self, fields: &[Field]) {
        for field in fields {
            let name = format!("field {}", field.name);
            self.nest(&name, |printer| {
                if let Some(ty) = &field.ty {
                    printer.ty(ty);
                }
            });
        }
    }

    fn struct_def(&mut self, def: &StructDef) {
        let header = format!("struct {}", self.res(&def.symbol));
        self.nest(&header, |printer| {
            printer.type_params(&def.type_params);
            printer.fields(&def.fields);
        });
    }

    fn enum_def(&mut self, def: &EnumDef) {
        let header = format!("enum {}", self.res(&def.symbol));
        self.nest(&header, |printer| {
            printer.type_params(&def.type_params);
            for Variant {
                symbol,
                span: _,
                fields,
            } in &def.variants
            {
                let line = format!("variant {}", printer.res(symbol));
                printer.nest(&line, |printer| printer.fields(fields));
            }
        });
    }

    fn interface_def(&mut self, def: &InterfaceDef) {
        let header = format!("interface {}", self.res(&def.symbol));
        self.nest(&header, |printer| {
            printer.type_params(&def.type_params);
            for member in &def.members {
                printer.function("member", member);
            }
        });
    }

    fn impl_def(&mut self, def: &ImplDef) {
        let header = format!("impl {}", self.res(&def.interface));
        self.nest(&header, |printer| {
            printer.type_params(&def.type_params);
            for arg in &def.interface_args {
                printer.nest("interface-arg", |printer| printer.ty(arg));
            }
            if let Some(target) = &def.target {
                printer.nest("for", |printer| printer.ty(target));
            }
            for function in &def.functions {
                printer.function("fn", function);
            }
        });
    }

    fn const_def(&mut self, def: &ConstDef) {
        let header = format!("const {}", self.res(&def.symbol));
        self.nest(&header, |printer| {
            if let Some(ty) = &def.ty {
                printer.ty(ty);
            }
            if let Some(value) = &def.value {
                printer.nest("value", |printer| printer.expr(value));
            }
        });
    }

    fn spec_def(&mut self, def: &SpecDef) {
        let mut header = format!("spec {:?}", def.name);
        if let Some(target) = def.target {
            write!(header, " targets {}", self.symbol(target)).expect("write to String");
        }
        self.nest(&header, |printer| {
            for statement in &def.statements {
                printer.spec_stmt(statement);
            }
        });
    }

    fn spec_stmt(&mut self, statement: &SpecStmt) {
        match statement {
            SpecStmt::Given(bindings) => self.nest("given", |printer| {
                for binding in bindings {
                    let line = format!("binding {}", printer.res(&binding.symbol));
                    printer.nest(&line, |printer| {
                        if let Some(ty) = &binding.ty {
                            printer.ty(ty);
                        }
                        if let Some(init) = &binding.init {
                            printer.nest("init", |printer| printer.expr(init));
                        }
                    });
                }
            }),
            SpecStmt::When(stmt) => self.nest("when", |printer| printer.stmt(stmt)),
            SpecStmt::Then(expr) => self.nest("then", |printer| printer.expr(expr)),
            SpecStmt::Assert(expr) => self.nest("assert", |printer| printer.expr(expr)),
            SpecStmt::Stmt(stmt) => self.stmt(stmt),
        }
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    fn ty(&mut self, ty: &Ty) {
        match &ty.kind {
            TyKind::Unit => self.line("type ()"),
            TyKind::SelfType => self.line("type Self"),
            TyKind::Path { res, args } => {
                let line = format!("type {}", self.res(res));
                self.nest(&line, |printer| {
                    for arg in args {
                        printer.ty(arg);
                    }
                });
            }
            TyKind::Wrapper { wrapper, args } => {
                let line = format!("type {}", wrapper.name());
                self.nest(&line, |printer| {
                    for arg in args {
                        printer.ty(arg);
                    }
                });
            }
            TyKind::FixedArray { element, len } => {
                let line = format!("type [_; {len}]");
                self.nest(&line, |printer| printer.ty(element));
            }
            TyKind::Fn { params, ret } => {
                self.nest("type fn", |printer| {
                    for param in params {
                        printer.nest(&format!("param {}", param.mode.keyword()), |p| {
                            p.ty(&param.ty);
                        });
                    }
                    printer.nest("return", |p| p.ty(ret));
                });
            }
            TyKind::Err => self.line("type {err}"),
        }
    }

    // ------------------------------------------------------------------
    // Blocks, statements, patterns
    // ------------------------------------------------------------------

    fn block(&mut self, block: &Block) {
        self.nest("block", |printer| {
            for stmt in &block.stmts {
                printer.stmt(stmt);
            }
            if let Some(tail) = &block.tail {
                printer.nest("tail", |printer| printer.expr(tail));
            }
        });
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(BindingDef {
                mutable,
                pat,
                ty,
                init,
            }) => {
                let keyword = if *mutable { "var" } else { "let" };
                self.nest(keyword, |printer| {
                    printer.pat(pat);
                    if let Some(ty) = ty {
                        printer.ty(ty);
                    }
                    if let Some(init) = init {
                        printer.nest("init", |printer| printer.expr(init));
                    }
                });
            }
            StmtKind::Const(def) => self.const_def(def),
            StmtKind::Expr(expr) => self.nest("stmt", |printer| printer.expr(expr)),
            StmtKind::Err => self.line("stmt {err}"),
        }
    }

    fn pat(&mut self, pat: &Pat) {
        match &pat.kind {
            PatKind::Wildcard => self.line("pat _"),
            PatKind::Lit(lit) => {
                let line = format!("pat lit {}", lit_text(lit));
                self.line(&line);
            }
            PatKind::Binding(res) => {
                let line = format!("pat bind {}", self.res(res));
                self.line(&line);
            }
            PatKind::Ctor { ctor, fields, rest } => {
                let mut line = format!("pat ctor {}", self.res(ctor));
                if *rest {
                    line.push_str(" ..");
                }
                self.nest(&line, |printer| {
                    for field in fields {
                        let name = format!("field {}", field.name);
                        printer.nest(&name, |printer| printer.pat(&field.pat));
                    }
                });
            }
            PatKind::Or(alternatives) => self.nest("pat or", |printer| {
                for alternative in alternatives {
                    printer.pat(alternative);
                }
            }),
            PatKind::Err => self.line("pat {err}"),
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per expression form; splitting would obscure the dispatch"
    )]
    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Lit(lit) => {
                let line = format!("lit {}", lit_text(lit));
                self.line(&line);
            }
            ExprKind::Path { res, args } => {
                let line = format!("path {}", self.res(res));
                if args.is_empty() {
                    self.line(&line);
                } else {
                    self.nest(&line, |printer| {
                        for arg in args {
                            printer.ty(arg);
                        }
                    });
                }
            }
            ExprKind::Array(elements) => {
                self.nest("array-lit", |printer| {
                    for element in elements {
                        printer.expr(element);
                    }
                });
            }
            ExprKind::ArrayRepeat { value, len } => {
                let line = format!("array-repeat {len}");
                self.nest(&line, |printer| printer.expr(value));
            }
            ExprKind::StructLit { ctor, args, fields } => {
                let line = format!("struct-lit {}", self.res(ctor));
                self.nest(&line, |printer| {
                    for arg in args {
                        printer.ty(arg);
                    }
                    for field in fields {
                        let name = format!("field {}", field.name);
                        printer.nest(&name, |printer| printer.expr(&field.value));
                    }
                });
            }
            ExprKind::Unary { op, operand } => {
                let line = format!("unary {}", op.symbol());
                self.nest(&line, |printer| printer.expr(operand));
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let line = format!("binary {}", op.symbol());
                self.nest(&line, |printer| {
                    printer.expr(lhs);
                    printer.expr(rhs);
                });
            }
            ExprKind::Range { lo, hi } => self.nest("range", |printer| {
                printer.expr(lo);
                printer.expr(hi);
            }),
            ExprKind::Assign { target, value } => self.nest("assign", |printer| {
                printer.expr(target);
                printer.expr(value);
            }),
            ExprKind::Call { callee, args } => self.nest("call", |printer| {
                printer.expr(callee);
                for arg in args {
                    printer.nest("arg", |printer| printer.expr(arg));
                }
            }),
            ExprKind::MethodCall {
                receiver,
                name,
                args,
            } => {
                let line = format!("method-call {name}");
                self.nest(&line, |printer| {
                    printer.expr(receiver);
                    for arg in args {
                        printer.nest("arg", |printer| printer.expr(arg));
                    }
                });
            }
            ExprKind::Field { receiver, name } => {
                let line = format!("field-access {name}");
                self.nest(&line, |printer| printer.expr(receiver));
            }
            ExprKind::Index { base, index } => self.nest("index", |printer| {
                printer.expr(base);
                printer.expr(index);
            }),
            ExprKind::Try(inner) => self.nest("try", |printer| printer.expr(inner)),
            ExprKind::Cast { value, ty } => self.nest("cast", |printer| {
                printer.expr(value);
                printer.ty(ty);
            }),
            ExprKind::If { cond, then, els } => self.nest("if", |printer| {
                printer.nest("cond", |printer| printer.expr(cond));
                printer.nest("then", |printer| printer.block(then));
                if let Some(els) = els {
                    printer.nest("else", |printer| printer.expr(els));
                }
            }),
            ExprKind::Match { scrutinee, arms } => self.nest("match", |printer| {
                printer.nest("scrutinee", |printer| printer.expr(scrutinee));
                for Arm {
                    pat,
                    guard,
                    value,
                    span: _,
                } in arms
                {
                    printer.nest("arm", |printer| {
                        printer.pat(pat);
                        if let Some(guard) = guard {
                            printer.nest("guard", |printer| printer.expr(guard));
                        }
                        printer.nest("value", |printer| printer.expr(value));
                    });
                }
            }),
            ExprKind::While { cond, body } => self.nest("while", |printer| {
                printer.nest("cond", |printer| printer.expr(cond));
                printer.block(body);
            }),
            ExprKind::Loop { label, body } => {
                let line = label
                    .as_ref()
                    .map_or_else(|| "loop".to_owned(), |label| format!("loop {label}"));
                self.nest(&line, |printer| printer.block(body));
            }
            ExprKind::For { pat, iter, body } => self.nest("for", |printer| {
                printer.pat(pat);
                printer.nest("in", |printer| printer.expr(iter));
                printer.block(body);
            }),
            ExprKind::Unsafe(body) => self.nest("unsafe", |printer| printer.block(body)),
            ExprKind::Return(value) => match value {
                Some(value) => self.nest("return", |printer| printer.expr(value)),
                None => self.line("return"),
            },
            ExprKind::Break { label, value } => {
                let line = label
                    .as_ref()
                    .map_or_else(|| "break".to_owned(), |label| format!("break {label}"));
                match value {
                    Some(value) => self.nest(&line, |printer| printer.expr(value)),
                    None => self.line(&line),
                }
            }
            ExprKind::Continue { label } => {
                let line = label.as_ref().map_or_else(
                    || "continue".to_owned(),
                    |label| format!("continue {label}"),
                );
                self.line(&line);
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::Err => self.line("{err}"),
        }
    }
}

/// The display form of a literal.
fn lit_text(lit: &Lit) -> String {
    match lit {
        Lit::Unit => "()".to_owned(),
        Lit::Bool(value) => value.to_string(),
        Lit::Int(digits) => digits.clone(),
        Lit::Float(digits) => digits.clone(),
        Lit::Char(text) => format!("'{text}'"),
        Lit::Str(text) => format!("{text:?}"),
    }
}
