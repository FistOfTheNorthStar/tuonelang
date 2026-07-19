//! Debug rendering of the typed AST: one construct per line, indented by
//! depth, driven entirely through the typed accessors (so the output doubles
//! as a lowering exercise of every view).
//!
//! The format is a developer diagnostic (`tuo debug ast`), **not** a stable
//! protocol — it may change freely.

use std::fmt::Write as _;

use crate::Ast;
use crate::expr::{ElseBranch, Expr};
use crate::item::{FnDecl, Item, SpecStatement};
use crate::pat::Pattern;
use crate::stmt::{Block, Statement};
use crate::ty::TypeRef;

/// Render the typed AST of a whole file.
#[must_use]
pub fn render(ast: Ast<'_>) -> String {
    let mut p = Printer::default();
    let file = ast.file();
    p.line(0, "SourceFile");
    if let Some(module) = file.module() {
        let path: Vec<&str> = module
            .path()
            .map(|p| p.segments().collect())
            .unwrap_or_default();
        p.line(1, &format!("ModuleDecl {}", path.join("::")));
    }
    for item in file.items() {
        p.item(1, item);
    }
    p.out
}

/// Truncate recovered-material snippets to keep lines readable.
fn snippet(text: &str) -> String {
    const MAX: usize = 60;
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() > MAX {
        let cut = (0..=MAX).rev().find(|&i| compact.is_char_boundary(i));
        compact.truncate(cut.unwrap_or(0));
        compact.push('…');
    }
    compact
}

#[derive(Default)]
struct Printer {
    out: String,
}

impl Printer {
    fn line(&mut self, depth: usize, text: &str) {
        let indent = "  ".repeat(depth);
        writeln!(self.out, "{indent}{text}").expect("write to String cannot fail");
    }

    fn item(&mut self, depth: usize, item: Item<'_>) {
        match item {
            Item::Import(import) => {
                let path: Vec<&str> = import
                    .path()
                    .map(|p| p.segments().collect())
                    .unwrap_or_default();
                let mut line = format!("ImportDecl {}", path.join("::"));
                let leaves: Vec<String> = import
                    .leaves()
                    .map(|leaf| match (leaf.name(), leaf.alias()) {
                        (Some(name), Some(alias)) => format!("{name} as {alias}"),
                        (Some(name), None) => name.to_owned(),
                        _ => String::new(),
                    })
                    .collect();
                if !leaves.is_empty() {
                    let _ = write!(line, " group=[{}]", leaves.join(", "));
                }
                if let Some(alias) = import.alias() {
                    let _ = write!(line, " as={alias}");
                }
                self.line(depth, &line);
            }
            Item::Fn(func) => self.fn_decl(depth, func),
            Item::Struct(decl) => {
                let mut line = format!("StructDecl {}", decl.name().unwrap_or("?"));
                if decl.is_pub() {
                    line.push_str(" pub");
                }
                self.generics_suffix(&mut line, decl.generics());
                self.line(depth, &line);
                for field in decl.fields() {
                    self.field(depth + 1, field.name(), field.ty(), field.is_pub());
                }
            }
            Item::Enum(decl) => {
                let mut line = format!("EnumDecl {}", decl.name().unwrap_or("?"));
                if decl.is_pub() {
                    line.push_str(" pub");
                }
                self.generics_suffix(&mut line, decl.generics());
                self.line(depth, &line);
                for variant in decl.variants() {
                    self.line(
                        depth + 1,
                        &format!("VariantDecl {}", variant.name().unwrap_or("?")),
                    );
                    for field in variant.fields() {
                        self.field(depth + 2, field.name(), field.ty(), field.is_pub());
                    }
                }
            }
            Item::Interface(decl) => {
                let mut line = format!("InterfaceDecl {}", decl.name().unwrap_or("?"));
                if decl.is_pub() {
                    line.push_str(" pub");
                }
                self.generics_suffix(&mut line, decl.generics());
                self.line(depth, &line);
                for member in decl.members() {
                    self.fn_decl(depth + 1, member);
                }
            }
            Item::Impl(decl) => {
                let interface = decl
                    .interface()
                    .map(|path| path.text().to_owned())
                    .unwrap_or_else(|| "?".to_owned());
                let target = decl
                    .target()
                    .map(|ty| ty.text().to_owned())
                    .unwrap_or_else(|| "?".to_owned());
                self.line(depth, &format!("ImplDecl `{interface}` for `{target}`"));
                for func in decl.functions() {
                    self.fn_decl(depth + 1, func);
                }
            }
            Item::Const(decl) => {
                let ty = decl.ty().map(TypeRef::text).unwrap_or("?");
                self.line(
                    depth,
                    &format!("ConstDecl {} ty=`{ty}`", decl.name().unwrap_or("?")),
                );
                if let Some(value) = decl.value() {
                    self.expr(depth + 1, value);
                }
            }
            Item::Spec(spec) => {
                self.line(depth, &format!("SpecDecl {}", spec.name().unwrap_or("?")));
                for statement in spec.statements() {
                    self.spec_statement(depth + 1, statement);
                }
            }
            Item::Error(error) => {
                self.line(depth, &format!("Error «{}»", snippet(error.text())));
            }
        }
    }

    fn field(&mut self, depth: usize, name: Option<&str>, ty: Option<TypeRef<'_>>, is_pub: bool) {
        let mut line = format!("FieldDecl {}", name.unwrap_or("?"));
        if is_pub {
            line.push_str(" pub");
        }
        let _ = write!(line, " ty=`{}`", ty.map(TypeRef::text).unwrap_or("?"));
        self.line(depth, &line);
    }

    fn generics_suffix(
        &mut self,
        line: &mut String,
        generics: Option<crate::item::GenericParams<'_>>,
    ) {
        if let Some(generics) = generics {
            let _ = write!(line, " generics=`{}`", generics.text());
        }
    }

    fn fn_decl(&mut self, depth: usize, func: FnDecl<'_>) {
        let mut line = format!("FnDecl {}", func.name().unwrap_or("?"));
        if func.is_pub() {
            line.push_str(" pub");
        }
        if func.is_signature() {
            line.push_str(" (signature)");
        }
        self.generics_suffix(&mut line, func.generics());
        if let Some(clause) = func.where_clause() {
            let _ = write!(line, " where=`{}`", clause.text());
        }
        self.line(depth, &line);
        for param in func.params() {
            let mut param_line = format!(
                "Param {} {}",
                param.mode().unwrap_or("?"),
                param.name().unwrap_or("?"),
            );
            if let Some(ty) = param.ty() {
                let _ = write!(param_line, " ty=`{}`", ty.text());
            }
            self.line(depth + 1, &param_line);
        }
        if let Some(ret) = func.return_type() {
            self.line(depth + 1, &format!("Returns `{}`", ret.text()));
        }
        if let Some(body) = func.body() {
            self.block(depth + 1, body);
        }
    }

    fn spec_statement(&mut self, depth: usize, statement: SpecStatement<'_>) {
        match statement {
            SpecStatement::Given(clause) => {
                self.line(depth, "Given");
                for binding in clause.bindings() {
                    let ty = binding.ty().map(TypeRef::text).unwrap_or("?");
                    self.line(
                        depth + 1,
                        &format!("GivenBinding {} ty=`{ty}`", binding.name().unwrap_or("?")),
                    );
                    if let Some(init) = binding.initializer() {
                        self.expr(depth + 2, init);
                    }
                }
            }
            SpecStatement::When(clause) => {
                self.line(depth, "When");
                if let Some(binding) = clause.binding() {
                    self.binding(depth + 1, binding, binding.is_var());
                } else if let Some(expr) = clause.expr() {
                    self.expr(depth + 1, expr);
                }
            }
            SpecStatement::Then(clause) => {
                self.line(depth, "Then");
                if let Some(expr) = clause.expr() {
                    self.expr(depth + 1, expr);
                }
            }
            SpecStatement::Assert(clause) => {
                self.line(depth, "Assert");
                if let Some(expr) = clause.expr() {
                    self.expr(depth + 1, expr);
                }
            }
            SpecStatement::Let(binding) => self.binding(depth, binding, false),
            SpecStatement::Var(binding) => self.binding(depth, binding, true),
            SpecStatement::Expr(statement) => {
                self.line(depth, "ExprStatement");
                if let Some(expr) = statement.expr() {
                    self.expr(depth + 1, expr);
                }
            }
            SpecStatement::Error(error) => {
                self.line(depth, &format!("Error «{}»", snippet(error.text())));
            }
        }
    }

    fn block(&mut self, depth: usize, block: Block<'_>) {
        self.line(depth, "Block");
        for statement in block.statements() {
            self.statement(depth + 1, statement);
        }
        if let Some(tail) = block.tail() {
            self.line(depth + 1, "Tail");
            self.expr(depth + 2, tail);
        }
    }

    fn statement(&mut self, depth: usize, statement: Statement<'_>) {
        match statement {
            Statement::Let(binding) => self.binding(depth, binding, false),
            Statement::Var(binding) => self.binding(depth, binding, true),
            Statement::Const(decl) => self.item(depth, Item::Const(decl)),
            Statement::Expr(statement) => {
                self.line(depth, "ExprStatement");
                if let Some(expr) = statement.expr() {
                    self.expr(depth + 1, expr);
                }
            }
            Statement::Empty(_) => self.line(depth, "EmptyStatement"),
            Statement::Error(error) => {
                self.line(depth, &format!("Error «{}»", snippet(error.text())));
            }
        }
    }

    fn binding(&mut self, depth: usize, binding: crate::stmt::BindingStmt<'_>, is_var: bool) {
        let keyword = if is_var { "Var" } else { "Let" };
        let pattern = binding.pattern().map(Pattern::text).unwrap_or("?");
        let mut line = format!("{keyword} `{pattern}`");
        if let Some(ty) = binding.ty() {
            let _ = write!(line, " ty=`{}`", ty.text());
        }
        self.line(depth, &line);
        if let Some(init) = binding.initializer() {
            self.expr(depth + 1, init);
        }
    }

    #[expect(clippy::too_many_lines, reason = "one arm per expression form")]
    fn expr(&mut self, depth: usize, expr: Expr<'_>) {
        match expr {
            Expr::Literal(lit) => self.line(depth, &format!("Literal `{}`", lit.text())),
            Expr::Path(path) => {
                let segments: Vec<&str> = path.segments().collect();
                let mut line = format!("Path `{}`", segments.join("::"));
                if let Some(fish) = path.turbofish() {
                    let _ = write!(line, " turbofish=`{}`", fish.text());
                }
                self.line(depth, &line);
            }
            Expr::StructLiteral(lit) => {
                let path = lit.path().map(|p| p.text()).unwrap_or("?");
                self.line(depth, &format!("StructLiteral `{path}`"));
                for init in lit.inits() {
                    self.line(
                        depth + 1,
                        &format!("FieldInit {}", init.name().unwrap_or("?")),
                    );
                    if let Some(value) = init.value() {
                        self.expr(depth + 2, value);
                    }
                }
            }
            Expr::Unary(unary) => {
                self.line(depth, &format!("Unary `{}`", unary.op().unwrap_or("?")));
                if let Some(operand) = unary.operand() {
                    self.expr(depth + 1, operand);
                }
            }
            Expr::Binary(binary) => {
                self.line(depth, &format!("Binary `{}`", binary.op().unwrap_or("?")));
                self.opt_expr(depth + 1, binary.lhs());
                self.opt_expr(depth + 1, binary.rhs());
            }
            Expr::Range(range) => {
                self.line(depth, "Range");
                self.opt_expr(depth + 1, range.lhs());
                self.opt_expr(depth + 1, range.rhs());
            }
            Expr::Assign(assign) => {
                self.line(depth, "Assign");
                self.opt_expr(depth + 1, assign.lhs());
                self.opt_expr(depth + 1, assign.rhs());
            }
            Expr::Call(call) => {
                self.line(depth, "Call");
                self.opt_expr(depth + 1, call.callee());
                for arg in call.args() {
                    self.expr(depth + 1, arg);
                }
            }
            Expr::MethodCall(call) => {
                self.line(
                    depth,
                    &format!("MethodCall .{}", call.name().unwrap_or("?")),
                );
                self.opt_expr(depth + 1, call.receiver());
                for arg in call.args() {
                    self.expr(depth + 1, arg);
                }
            }
            Expr::Field(field) => {
                self.line(depth, &format!("Field .{}", field.name().unwrap_or("?")));
                self.opt_expr(depth + 1, field.receiver());
            }
            Expr::Index(index) => {
                self.line(depth, "Index");
                self.opt_expr(depth + 1, index.base());
                self.opt_expr(depth + 1, index.index());
            }
            Expr::Try(inner) => {
                self.line(depth, "Try");
                self.opt_expr(depth + 1, inner.inner());
            }
            Expr::Cast(cast) => {
                let ty = cast.ty().map(TypeRef::text).unwrap_or("?");
                self.line(depth, &format!("Cast ty=`{ty}`"));
                self.opt_expr(depth + 1, cast.inner());
            }
            Expr::Group(group) => {
                self.line(depth, "Group");
                self.opt_expr(depth + 1, group.inner());
            }
            Expr::If(if_expr) => {
                self.line(depth, "If");
                self.opt_expr(depth + 1, if_expr.condition());
                if let Some(then) = if_expr.then_block() {
                    self.block(depth + 1, then);
                }
                match if_expr.else_branch() {
                    Some(ElseBranch::If(nested)) => {
                        self.line(depth + 1, "Else");
                        self.expr(depth + 2, Expr::If(nested));
                    }
                    Some(ElseBranch::Block(block)) => {
                        self.line(depth + 1, "Else");
                        self.block(depth + 2, block);
                    }
                    None => {}
                }
            }
            Expr::Match(match_expr) => {
                self.line(depth, "Match");
                self.opt_expr(depth + 1, match_expr.scrutinee());
                for arm in match_expr.arms() {
                    let pattern = arm.pattern().map(Pattern::text).unwrap_or("?");
                    let guarded = if arm.guard().is_some() {
                        " guarded"
                    } else {
                        ""
                    };
                    self.line(depth + 1, &format!("Arm `{pattern}`{guarded}"));
                    if let Some(guard) = arm.guard() {
                        self.line(depth + 2, "Guard");
                        self.expr(depth + 3, guard);
                    }
                    self.opt_expr(depth + 2, arm.value());
                }
            }
            Expr::While(while_expr) => {
                self.line(depth, "While");
                self.opt_expr(depth + 1, while_expr.condition());
                if let Some(body) = while_expr.body() {
                    self.block(depth + 1, body);
                }
            }
            Expr::Loop(loop_expr) => {
                let label = loop_expr
                    .label()
                    .map(|label| format!(" label={label}"))
                    .unwrap_or_default();
                self.line(depth, &format!("Loop{label}"));
                if let Some(body) = loop_expr.body() {
                    self.block(depth + 1, body);
                }
            }
            Expr::For(for_expr) => {
                let pattern = for_expr.pattern().map(Pattern::text).unwrap_or("?");
                self.line(depth, &format!("For `{pattern}`"));
                self.opt_expr(depth + 1, for_expr.iterable());
                if let Some(body) = for_expr.body() {
                    self.block(depth + 1, body);
                }
            }
            Expr::Unsafe(unsafe_expr) => {
                self.line(depth, "Unsafe");
                if let Some(body) = unsafe_expr.body() {
                    self.block(depth + 1, body);
                }
            }
            Expr::Return(ret) => {
                self.line(depth, "Return");
                self.opt_expr(depth + 1, ret.value());
            }
            Expr::Break(brk) => {
                let label = brk
                    .label()
                    .map(|label| format!(" label={label}"))
                    .unwrap_or_default();
                self.line(depth, &format!("Break{label}"));
                self.opt_expr(depth + 1, brk.value());
            }
            Expr::Continue(cont) => {
                let label = cont
                    .label()
                    .map(|label| format!(" label={label}"))
                    .unwrap_or_default();
                self.line(depth, &format!("Continue{label}"));
            }
            Expr::Block(block) => self.block(depth, block),
        }
    }

    fn opt_expr(&mut self, depth: usize, expr: Option<Expr<'_>>) {
        if let Some(expr) = expr {
            self.expr(depth, expr);
        }
    }
}
