//! Interfaces and impls (ADR-0032 Stage B): declaration rules, conformance,
//! placement and coherence, and the non-generic qualified call `I::m(x)`.
//!
//! An interface member's `Self` is represented while checking as
//! [`Ty::Param`] of the interface's own symbol, so the ordinary
//! [`substitute`] replaces it with an impl's target. Every member's first
//! parameter is the receiver (D3), so the first argument of a qualified call
//! selects the impl, at check time, by its concrete type; the selected member
//! is published in [`crate::TypeckResult::impl_call`] for the ownership
//! checker and MIR lowering, which never re-derive it.

use std::collections::HashMap;

use tuo_ast::{Ast, Expr, FnDecl, ImplDecl, InterfaceDecl, Name};
use tuo_diagnostics::{Diagnostic, StructuredValue};
use tuo_resolve::{ModuleId, SymbolId, SymbolKind};
use tuo_source::{Span, TextRange};

use super::{Checker, FnSig, code, substitute, value_span};
use crate::infer::VarClass;
use crate::ty::Ty;

/// One member of an interface, as declared.
#[derive(Clone)]
pub(super) struct MemberDecl {
    /// The member's name.
    pub(super) name: String,
    /// The span of the member's name.
    pub(super) span: Span,
    /// The signature, `Self` as [`Ty::Param`] of the interface.
    pub(super) sig: FnSig,
    /// Whether the first parameter is the receiver (`T0029` otherwise).
    pub(super) has_receiver: bool,
}

/// An `impl` block as collected, before its checks run.
pub(super) struct ImplDraft {
    /// What the interface path resolved to, if anything.
    interface: Option<SymbolId>,
    /// The interface path's span.
    interface_span: Span,
    /// The number of type arguments written on the interface.
    interface_args: usize,
    /// The span of `impl[…]`'s parameter list, when it has one.
    generics: Option<Span>,
    /// From `impl` to the end of the target type.
    header: Span,
    /// The target type.
    target: Ty,
    /// The target type's span.
    target_span: Span,
    /// The module the block is written in.
    module: Option<ModuleId>,
    /// The members' symbols, in source order.
    members: Vec<SymbolId>,
}

/// An accepted `impl I for T`: what qualified calls select from.
pub(super) struct ImplEntry {
    interface: SymbolId,
    target: Ty,
    header: Span,
    members: HashMap<String, SymbolId>,
}

impl Checker<'_> {
    /// The module a file's items belong to.
    pub(super) fn file_module(&self, ast: Ast<'_>) -> Option<ModuleId> {
        let path: Vec<String> = ast
            .file()
            .module()
            .and_then(|module| module.path())
            .map(|path| path.segments().map(str::to_owned).collect())
            .unwrap_or_default();
        self.resolution
            .modules()
            .iter()
            .position(|module| module.path == path)
            .and_then(|index| u32::try_from(index).ok())
            .map(ModuleId::from_raw)
    }

    /// Collect an interface's member signatures, reporting the forms Stage B
    /// refuses: generic interfaces and members (`T0031`), default bodies
    /// (`T0030`), and members without a receiver first (`T0029`).
    pub(super) fn collect_interface(&mut self, decl: InterfaceDecl<'_>) {
        let Some(id) = self.declared_at(decl.name_ref()) else {
            return;
        };
        let name = self.resolution.symbol(id).name.clone();
        if let Some(generics) = decl.generics() {
            let span = self.at(generics.span());
            self.push(
                Diagnostic::error(
                    code(31),
                    format!("interface `{name}` has type parameters, which are not supported"),
                    span,
                )
                .with_primary_label("generic interfaces are not supported")
                .with_help("declare the interface without type parameters")
                .with_actual(StructuredValue::Name(name.clone())),
            );
        }
        let outer = self.self_ty.replace(Ty::Param(id));
        let mut members: Vec<MemberDecl> = Vec::new();
        for member in decl.members() {
            let Some(member_name) = member.name_ref() else {
                continue;
            };
            // A repeated name is the resolver's `R0001`.
            if members.iter().any(|known| known.name == member_name.text) {
                continue;
            }
            self.check_member_form(&name, member, member_name);
            let sig = self.lower_fn_sig(member);
            members.push(MemberDecl {
                name: member_name.text.to_owned(),
                span: member_name.span,
                sig,
                has_receiver: member.params().next().is_some_and(|p| p.is_receiver()),
            });
        }
        self.self_ty = outer;
        self.interfaces.insert(id, members);
    }

    /// `T0029`–`T0031` on one interface member.
    fn check_member_form(&mut self, interface: &str, member: FnDecl<'_>, name: Name<'_>) {
        let member_name = name.text;
        if let Some(generics) = member.generics() {
            let span = self.at(generics.span());
            self.push(
                Diagnostic::error(
                    code(31),
                    format!(
                        "member `{member_name}` of interface `{interface}` has type parameters, \
                         which are not supported"
                    ),
                    span,
                )
                .with_primary_label("generic interface members are not supported")
                .with_help("declare the member without type parameters")
                .with_actual(StructuredValue::Name(member_name.to_owned())),
            );
        }
        if member.body().is_some() {
            self.push(
                Diagnostic::error(
                    code(30),
                    format!(
                        "member `{member_name}` of interface `{interface}` has a default body, \
                         and default member bodies are not supported"
                    ),
                    name.span,
                )
                .with_primary_label("default member bodies are not supported")
                .with_help(format!(
                    "end the signature with `;` and write the body in each `impl {interface} \
                     for …`"
                ))
                .with_actual(StructuredValue::Name(member_name.to_owned())),
            );
        }
        if !member.params().next().is_some_and(|p| p.is_receiver()) {
            self.push(
                Diagnostic::error(
                    code(29),
                    format!(
                        "member `{member_name}` of interface `{interface}` has no receiver as its \
                         first parameter"
                    ),
                    name.span,
                )
                .with_primary_label(
                    "the first parameter must be `in self`, `mut self` or `take self`",
                )
                .with_help(
                    "members without a receiver (such as `fn zero() -> Self`) are deferred; \
                     write a free function instead",
                )
                .with_actual(StructuredValue::Name(member_name.to_owned())),
            );
        }
    }

    /// Collect an impl block: its target type and its members' signatures
    /// (`Self` and the receiver meaning the target). The block's checks run
    /// in [`Self::check_impls`], once every interface is known.
    pub(super) fn collect_impl(&mut self, decl: ImplDecl<'_>, module: Option<ModuleId>) {
        let target_ref = decl.target();
        let target = target_ref.map_or(Ty::Error, |ty| self.lower_type(ty));
        let block = self.at(decl.span());
        let target_span = self.at(target_ref.and_then(|ty| ty.span()));
        if let Some(span) = decl.span() {
            self.impl_targets.insert(span, target.clone());
        }
        let outer = self.self_ty.replace(target.clone());
        let mut members = Vec::new();
        for function in decl.functions() {
            let Some(id) = self.declared_at(function.name_ref()) else {
                continue;
            };
            let sig = self.lower_fn_sig(function);
            self.symbol_types
                .insert(id, Ty::Fn(Box::new(sig.as_fn_ty())));
            self.fns.insert(id, sig);
            self.impl_fns.insert(id);
            members.push(id);
        }
        self.self_ty = outer;
        let interface_path = decl.interface();
        let interface = interface_path
            .and_then(|path| path.segment_names().last())
            .and_then(|name| self.refs.get(&name.span).copied());
        let header = if block.source() == target_span.source()
            && block.range().start() <= target_span.range().end()
        {
            TextRange::new(block.range().start(), target_span.range().end())
                .map_or(block, |range| Span::new(block.source(), range))
        } else {
            block
        };
        self.impl_drafts.push(ImplDraft {
            interface,
            interface_span: self.at(interface_path.and_then(|path| path.span())),
            interface_args: decl.interface_args().map_or(0, |args| args.types().count()),
            generics: decl.generics().map(|generics| self.at(generics.span())),
            header,
            target,
            target_span,
            module,
            members,
        });
    }

    /// Check every collected impl: that it names an interface (`T0041`), is
    /// not generic (`T0035`), targets a concrete type (`T0036`), is placed
    /// with its interface or its type (`T0037`), is the only impl of its
    /// pair (`T0038`), and conforms to its interface (`T0032`–`T0034`).
    pub(super) fn check_impls(&mut self) {
        for draft in std::mem::take(&mut self.impl_drafts) {
            let Some(interface) = draft.interface else {
                // An unresolved name is already the resolver's `R0002`.
                continue;
            };
            let interface_name = self.resolution.symbol(interface).name.clone();
            let kind = self.resolution.symbol(interface).kind;
            if kind != SymbolKind::Interface {
                self.push(
                    Diagnostic::error(
                        code(41),
                        format!("`{interface_name}` is a {}, not an interface", kind.noun()),
                        draft.interface_span,
                    )
                    .with_primary_label("`impl … for` must name an interface")
                    .with_help("inherent impls are not supported; write free functions")
                    .with_actual(StructuredValue::Name(interface_name)),
                );
                continue;
            }
            if draft.interface_args > 0 {
                self.check_type_arity(
                    &interface_name,
                    0,
                    draft.interface_args,
                    draft.interface_span,
                );
            }
            if let Some(span) = draft.generics {
                self.push(
                    Diagnostic::error(
                        code(35),
                        format!("an `impl` of `{interface_name}` has type parameters"),
                        span,
                    )
                    .with_primary_label("blanket and generic impls are not supported")
                    .with_help("write one `impl` per concrete type")
                    .with_actual(StructuredValue::Name(interface_name)),
                );
                continue;
            }
            if matches!(draft.target, Ty::Error) {
                continue;
            }
            let target_text = self.render(&draft.target);
            if !self.is_impl_target(&draft.target) {
                self.push(
                    Diagnostic::error(
                        code(36),
                        format!("`{target_text}` cannot implement an interface"),
                        draft.target_span,
                    )
                    .with_primary_label(
                        "an impl target is a non-generic struct or enum, or a builtin scalar type",
                    )
                    .with_help(
                        "type applications, tuples, arrays and function types cannot be impl \
                         targets; wrap the value in a struct and implement the interface for it",
                    )
                    .with_actual(StructuredValue::Type(target_text)),
                );
                continue;
            }
            self.check_placement(interface, &draft, &interface_name, &target_text);
            if let Some(first) = self
                .impls
                .iter()
                .find(|entry| entry.interface == interface && entry.target == draft.target)
            {
                let first = first.header;
                self.push(
                    Diagnostic::error(
                        code(38),
                        format!("`{target_text}` already implements `{interface_name}`"),
                        draft.header,
                    )
                    .with_primary_label("a second impl of the same interface for the same type")
                    .with_secondary_label(first, "first impl here")
                    .with_help("remove one of the two impls")
                    .with_actual(StructuredValue::Type(target_text)),
                );
                continue;
            }
            self.check_conformance(interface, &draft, &interface_name, &target_text);
            let mut members = HashMap::new();
            for &member in &draft.members {
                members.insert(self.resolution.symbol(member).name.clone(), member);
                self.impl_members.insert(
                    member,
                    crate::ImplMember {
                        interface,
                        target: draft.target.clone(),
                    },
                );
            }
            self.impls.push(ImplEntry {
                interface,
                target: draft.target,
                header: draft.header,
                members,
            });
        }
    }

    /// May `ty` be an impl target (D4)? A non-generic user struct or enum,
    /// or a builtin scalar type (`Str` included).
    fn is_impl_target(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(symbol, args) => {
                args.is_empty()
                    && self
                        .structs
                        .get(symbol)
                        .is_some_and(|def| def.type_params.is_empty())
            }
            Ty::Enum(symbol, args) => {
                args.is_empty()
                    && self
                        .enums
                        .get(symbol)
                        .is_some_and(|def| def.type_params.is_empty())
            }
            Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Str => true,
            _ => false,
        }
    }

    /// `T0037`: `impl I for T` lives in the module declaring `I` or the one
    /// declaring `T` (with `I` only, for a builtin `T`).
    fn check_placement(
        &mut self,
        interface: SymbolId,
        draft: &ImplDraft,
        interface_name: &str,
        target_text: &str,
    ) {
        let Some(module) = draft.module else {
            return;
        };
        let interface_module = self.resolution.symbol(interface).module;
        let target_module = match &draft.target {
            Ty::Struct(symbol, _) | Ty::Enum(symbol, _) => {
                Some(self.resolution.symbol(*symbol).module)
            }
            _ => None,
        };
        if module == interface_module || Some(module) == target_module {
            return;
        }
        let interface_place = self.module_text(interface_module);
        let allowed = match target_module {
            Some(target_module) => format!(
                "{interface_place}, which declares `{interface_name}`, or {}, which declares \
                 `{target_text}`",
                self.module_text(target_module)
            ),
            None => format!(
                "{interface_place}, which declares `{interface_name}` (`{target_text}` is a \
                 builtin type)"
            ),
        };
        self.push(
            Diagnostic::error(
                code(37),
                format!(
                    "`impl {interface_name} for {target_text}` is in {}, but it must be in \
                     {allowed}",
                    self.module_text(module)
                ),
                draft.header,
            )
            .with_primary_label("an impl lives with its interface or its type")
            .with_help(format!("move this impl to {allowed}"))
            .with_actual(StructuredValue::Text(self.module_text(module))),
        );
    }

    /// A module's name for a diagnostic.
    fn module_text(&self, module: ModuleId) -> String {
        let path = &self.resolution.module(module).path;
        if path.is_empty() {
            "the root module".to_owned()
        } else {
            format!("module `{}`", path.join("::"))
        }
    }

    /// `T0032`–`T0034`: the impl supplies exactly the interface's members,
    /// each with the interface's signature, `Self` replaced by the target.
    fn check_conformance(
        &mut self,
        interface: SymbolId,
        draft: &ImplDraft,
        interface_name: &str,
        target_text: &str,
    ) {
        let declared = self.interfaces.get(&interface).cloned().unwrap_or_default();
        let map: HashMap<SymbolId, Ty> = HashMap::from([(interface, draft.target.clone())]);
        for member in &declared {
            if draft
                .members
                .iter()
                .any(|&symbol| self.resolution.symbol(symbol).name == member.name)
            {
                continue;
            }
            let expected = self.signature_text(&member.name, &member.sig, &map);
            self.push(
                Diagnostic::error(
                    code(32),
                    format!(
                        "`impl {interface_name} for {target_text}` is missing member `{}`",
                        member.name
                    ),
                    draft.header,
                )
                .with_primary_label(format!("`{}` is not implemented", member.name))
                .with_secondary_label(member.span, "member declared here")
                .with_help(format!("add `{expected} {{ … }}` to this impl"))
                .with_expected(StructuredValue::Name(member.name.clone())),
            );
        }
        for &symbol in &draft.members {
            let name = self.resolution.symbol(symbol).name.clone();
            let span = self.at(self.resolution.symbol(symbol).declaration);
            let Some(member) = declared.iter().find(|member| member.name == name) else {
                let listed: Vec<String> = declared
                    .iter()
                    .map(|member| format!("`{}`", member.name))
                    .collect();
                let help = if listed.is_empty() {
                    format!("`{interface_name}` declares no members; remove this function")
                } else {
                    format!(
                        "`{interface_name}` declares {}; remove this function or make it a free \
                         function",
                        listed.join(", ")
                    )
                };
                self.push(
                    Diagnostic::error(
                        code(33),
                        format!("`{name}` is not a member of interface `{interface_name}`"),
                        span,
                    )
                    .with_primary_label("not declared by the interface")
                    .with_help(help)
                    .with_actual(StructuredValue::Name(name)),
                );
                continue;
            };
            let Some(actual) = self.fns.get(&symbol).cloned() else {
                continue;
            };
            self.compare_member(interface_name, &name, span, member, &actual, &map);
        }
    }

    /// `T0034`: every way an impl member's signature differs from its
    /// interface's (count, each parameter's mode and type, the return type).
    fn compare_member(
        &mut self,
        interface_name: &str,
        name: &str,
        span: Span,
        member: &MemberDecl,
        actual: &FnSig,
        map: &HashMap<SymbolId, Ty>,
    ) {
        let declared_here = |diagnostic: Diagnostic| {
            diagnostic.with_secondary_label(member.span, "interface member declared here")
        };
        let expected_params: Vec<Ty> = member
            .sig
            .params
            .iter()
            .map(|ty| substitute(ty, map))
            .collect();
        if expected_params.len() != actual.params.len() {
            self.push(declared_here(
                Diagnostic::error(
                    code(34),
                    format!(
                        "`{name}` takes {} parameter{} but `{interface_name}::{name}` declares {}",
                        actual.params.len(),
                        if actual.params.len() == 1 { "" } else { "s" },
                        expected_params.len()
                    ),
                    span,
                )
                .with_primary_label("the impl member's signature differs from the interface's")
                .with_expected(StructuredValue::Count(expected_params.len() as u64))
                .with_actual(StructuredValue::Count(actual.params.len() as u64)),
            ));
        }
        for (index, ((expected, expected_mode), (found, found_mode))) in expected_params
            .iter()
            .zip(&member.sig.modes)
            .zip(actual.params.iter().zip(&actual.modes))
            .enumerate()
        {
            let which = if index == 0 && member.has_receiver {
                "the receiver".to_owned()
            } else {
                format!("parameter {}", index + 1)
            };
            if expected_mode != found_mode {
                self.push(declared_here(
                    Diagnostic::error(
                        code(34),
                        format!(
                            "{which} of `{name}` is `{}` but `{interface_name}::{name}` declares \
                             `{}`",
                            found_mode.keyword(),
                            expected_mode.keyword()
                        ),
                        span,
                    )
                    .with_primary_label("the impl member's signature differs from the interface's")
                    .with_help(format!("write `{}` here", expected_mode.keyword()))
                    .with_expected(StructuredValue::Token(expected_mode.keyword().to_owned()))
                    .with_actual(StructuredValue::Token(found_mode.keyword().to_owned())),
                ));
            }
            if !has_error(expected) && !has_error(found) && expected != found {
                let (expected, found) = (self.render(expected), self.render(found));
                self.push(declared_here(
                    Diagnostic::error(
                        code(34),
                        format!(
                            "{which} of `{name}` has type `{found}` but `{interface_name}::{name}` \
                             declares `{expected}`"
                        ),
                        span,
                    )
                    .with_primary_label("the impl member's signature differs from the interface's")
                    .with_expected(StructuredValue::Type(expected))
                    .with_actual(StructuredValue::Type(found)),
                ));
            }
        }
        let expected_ret = substitute(&member.sig.ret, map);
        if !has_error(&expected_ret) && !has_error(&actual.ret) && expected_ret != actual.ret {
            let (expected, found) = (self.render(&expected_ret), self.render(&actual.ret));
            self.push(declared_here(
                Diagnostic::error(
                    code(34),
                    format!(
                        "`{name}` returns `{found}` but `{interface_name}::{name}` declares \
                         `{expected}`"
                    ),
                    span,
                )
                .with_primary_label("the impl member's signature differs from the interface's")
                .with_expected(StructuredValue::Type(expected))
                .with_actual(StructuredValue::Type(found)),
            ));
        }
    }

    /// `fn name(mode self, mode T, …) -> R`, `Self` replaced per `map`.
    fn signature_text(&self, name: &str, sig: &FnSig, map: &HashMap<SymbolId, Ty>) -> String {
        let params: Vec<String> = sig
            .params
            .iter()
            .zip(&sig.modes)
            .enumerate()
            .map(|(index, (ty, mode))| {
                if index == 0 {
                    format!("{} self", mode.keyword())
                } else {
                    format!(
                        "{} p{index}: {}",
                        mode.keyword(),
                        substitute(ty, map).render(self.resolution)
                    )
                }
            })
            .collect();
        format!(
            "fn {name}({}) -> {}",
            params.join(", "),
            substitute(&sig.ret, map).render(self.resolution)
        )
    }

    /// A qualified interface-member call `I::m(args)` (ADR-0032 Stage B):
    /// select the impl by the first argument's concrete type, check the
    /// rest against the member's signature, and publish the selection.
    pub(super) fn interface_call(
        &mut self,
        interface: SymbolId,
        tail: &[Name<'_>],
        args: &[Expr<'_>],
        span: Span,
    ) -> Ty {
        let interface_name = self.resolution.symbol(interface).name.clone();
        let declared = self.interfaces.get(&interface).cloned().unwrap_or_default();
        let member = match tail {
            [name] => declared
                .iter()
                .find(|member| member.name == name.text)
                .cloned(),
            _ => None,
        };
        let Some(member) = member else {
            for arg in args {
                self.expr(*arg);
            }
            let written: Vec<&str> = tail.iter().map(|name| name.text).collect();
            let written = written.join("::");
            let at = tail.first().map_or(span, |name| name.span);
            let listed: Vec<String> = declared
                .iter()
                .map(|member| format!("`{interface_name}::{}`", member.name))
                .collect();
            let help = if listed.is_empty() {
                format!("`{interface_name}` declares no members")
            } else {
                format!("`{interface_name}` declares {}", listed.join(", "))
            };
            self.push(
                Diagnostic::error(
                    code(39),
                    format!("`{written}` is not a member of interface `{interface_name}`"),
                    at,
                )
                .with_primary_label(format!("no member `{written}` in `{interface_name}`"))
                .with_help(help)
                .with_actual(StructuredValue::Name(written)),
            );
            return Ty::Error;
        };
        let written = format!("{interface_name}::{}", member.name);
        // A member without a receiver is already `T0029` at its declaration.
        let (true, Some(first)) = (member.has_receiver, args.first()) else {
            if member.has_receiver {
                self.arity_mismatch(member.sig.params.len(), 0, span);
            }
            for arg in args {
                self.expr(*arg);
            }
            return Ty::Error;
        };
        let first_span = self.at(value_span(first));
        let first_ty = self.expr(*first);
        let receiver = match self.icx.literal_class(&first_ty) {
            Some(VarClass::Integer) => Ty::int(),
            Some(VarClass::Float) => Ty::float(),
            _ => self.icx.view(&first_ty),
        };
        if receiver != first_ty {
            self.expect_ty(&receiver, &first_ty, first_span);
        }
        let selected = match &receiver {
            Ty::Error | Ty::Never => None,
            Ty::Var(_) => {
                self.push(
                    Diagnostic::error(code(11), "type annotation needed", first_span)
                        .with_primary_label("cannot infer the type here")
                        .with_help(format!(
                            "the first argument's type selects the impl of `{interface_name}`; \
                             annotate it"
                        )),
                );
                None
            }
            Ty::Param(param) => {
                let param = self.resolution.symbol(*param).name.clone();
                self.push(
                    Diagnostic::error(
                        code(25),
                        format!(
                            "`{written}` on a value of generic type `{param}` is not supported \
                             yet"
                        ),
                        span,
                    )
                    .with_primary_label("interface calls in generic bodies are ADR-0032 Stage C")
                    .with_help(
                        "call the member on a value of a concrete type, or write a free function \
                         per type",
                    )
                    .with_actual(StructuredValue::Name(written.clone())),
                );
                None
            }
            concrete => {
                let entry = self
                    .impls
                    .iter()
                    .find(|entry| entry.interface == interface && &entry.target == concrete);
                match entry {
                    Some(entry) => Some(entry.members.get(&member.name).copied()),
                    None => {
                        self.no_impl(interface, &interface_name, &written, concrete, first_span);
                        None
                    }
                }
            }
        };
        let Some(selected) = selected else {
            for arg in &args[1..] {
                self.expr(*arg);
            }
            return Ty::Error;
        };
        let map: HashMap<SymbolId, Ty> = HashMap::from([(interface, receiver)]);
        let params: Vec<Ty> = member
            .sig
            .params
            .iter()
            .map(|ty| substitute(ty, &map))
            .collect();
        if params.len() != args.len() {
            self.arity_mismatch(params.len(), args.len(), span);
        }
        for (param, arg) in params.iter().zip(args).skip(1) {
            let arg_span = self.at(value_span(arg));
            let actual = self.expr(*arg);
            self.expect_ty(param, &actual, arg_span);
        }
        for arg in args.iter().skip(params.len().max(1)) {
            self.expr(*arg);
        }
        // `None` only when the impl is missing the member, already `T0032`.
        if let Some(symbol) = selected {
            self.record_call_edge(symbol);
            self.impl_calls.insert(span, symbol);
        }
        substitute(&member.sig.ret, &map)
    }

    /// `T0040`: the first argument's type has no impl of the interface.
    fn no_impl(
        &mut self,
        interface: SymbolId,
        interface_name: &str,
        written: &str,
        found: &Ty,
        span: Span,
    ) {
        let found = self.render(found);
        let implementors: Vec<String> = self
            .impls
            .iter()
            .filter(|entry| entry.interface == interface)
            .map(|entry| format!("`{}`", entry.target.render(self.resolution)))
            .collect();
        let help = if implementors.is_empty() {
            format!(
                "no type implements `{interface_name}`; add `impl {interface_name} for {found}`"
            )
        } else {
            format!(
                "`{interface_name}` is implemented for {}; pass one of those, or add `impl \
                 {interface_name} for {found}`",
                implementors.join(", ")
            )
        };
        self.push(
            Diagnostic::error(
                code(40),
                format!("`{found}` does not implement `{interface_name}`"),
                span,
            )
            .with_primary_label(format!(
                "the first argument of `{written}` selects the impl, and there is no `impl \
                 {interface_name} for {found}`"
            ))
            .with_help(help)
            .with_expected(StructuredValue::Name(interface_name.to_owned()))
            .with_actual(StructuredValue::Type(found)),
        );
    }
}

/// Does `ty` contain the poison type anywhere?
fn has_error(ty: &Ty) -> bool {
    match ty {
        Ty::Error => true,
        Ty::Tuple(items) => items.iter().any(has_error),
        Ty::Array(item)
        | Ty::FixedArray(item, _)
        | Ty::Range(item)
        | Ty::Option(item)
        | Ty::Wrapper(_, item) => has_error(item),
        Ty::Map(key, value) | Ty::Result(key, value) => has_error(key) || has_error(value),
        Ty::Fn(fn_ty) => {
            fn_ty.params.iter().any(|param| has_error(&param.ty)) || has_error(&fn_ty.ret)
        }
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(has_error),
        _ => false,
    }
}
