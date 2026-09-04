//! Typed views over items and declarations (grammar sections B, C, E, I).

use tuo_lexer::TokenKind;
use tuo_syntax::{SyntaxKind, SyntaxNode};

use crate::Ast;
use crate::context::{Name, ast_view, child_nodes, child_tokens, node_of_kind, nodes_of_kind};
use crate::expr::Expr;
use crate::stmt::{BindingStmt, Block};
use crate::ty::{TypeArgs, TypePath, TypeRef};

ast_view! {
    /// The root of a parsed file: an optional module declaration followed by
    /// items (and error islands where recovery occurred).
    SourceFile from SourceFile
}

impl<'a> SourceFile<'a> {
    /// The `module a::b;` declaration, if the file has one.
    #[must_use]
    pub fn module(self) -> Option<ModuleDecl<'a>> {
        node_of_kind(self.node, SyntaxKind::ModuleDecl)
            .and_then(|node| ModuleDecl::cast(self.ast, node))
    }

    /// The file's items, in source order — including [`Item::Error`]
    /// islands where the parser recovered.
    pub fn items(self) -> impl Iterator<Item = Item<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| Item::cast(ast, node))
    }
}

/// One top-level item.
#[derive(Clone, Copy, Debug)]
pub enum Item<'a> {
    /// An `import …;`.
    Import(ImportDecl<'a>),
    /// A function definition.
    Fn(FnDecl<'a>),
    /// A `struct` declaration.
    Struct(StructDecl<'a>),
    /// An `enum` declaration.
    Enum(EnumDecl<'a>),
    /// An `interface` declaration.
    Interface(InterfaceDecl<'a>),
    /// An `impl … for …` block.
    Impl(ImplDecl<'a>),
    /// A `const` item.
    Const(ConstDecl<'a>),
    /// A `spec` block.
    Spec(SpecDecl<'a>),
    /// Material skipped during error recovery.
    Error(ErrorNode<'a>),
}

impl<'a> Item<'a> {
    /// View `node` as an item, if it is one.
    #[must_use]
    pub fn cast(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<Self> {
        match node.kind {
            SyntaxKind::ImportItem => ImportDecl::cast(ast, node).map(Self::Import),
            SyntaxKind::FunctionItem | SyntaxKind::FunctionSignature => {
                FnDecl::cast(ast, node).map(Self::Fn)
            }
            SyntaxKind::StructItem => StructDecl::cast(ast, node).map(Self::Struct),
            SyntaxKind::EnumItem => EnumDecl::cast(ast, node).map(Self::Enum),
            SyntaxKind::InterfaceItem => InterfaceDecl::cast(ast, node).map(Self::Interface),
            SyntaxKind::ImplItem => ImplDecl::cast(ast, node).map(Self::Impl),
            SyntaxKind::ConstItem => ConstDecl::cast(ast, node).map(Self::Const),
            SyntaxKind::SpecItem => SpecDecl::cast(ast, node).map(Self::Spec),
            SyntaxKind::Error => ErrorNode::cast(ast, node).map(Self::Error),
            _ => None,
        }
    }
}

ast_view! {
    /// Material the parser skipped during error recovery, retained for
    /// tooling.
    ErrorNode from Error
}

ast_view! {
    /// The `module a::b;` declaration.
    ModuleDecl from ModuleDecl
}

impl<'a> ModuleDecl<'a> {
    /// The declared module path.
    #[must_use]
    pub fn path(self) -> Option<PathNode<'a>> {
        node_of_kind(self.node, SyntaxKind::Path).and_then(|node| PathNode::cast(self.ast, node))
    }
}

ast_view! {
    /// A `::`-separated identifier path (modules and imports).
    PathNode from Path
}

impl<'a> PathNode<'a> {
    /// The path segments, in source order.
    pub fn segments(self) -> impl Iterator<Item = &'a str> {
        self.segment_names().map(|name| name.text)
    }

    /// The path segments with their exact spans, in source order.
    pub fn segment_names(self) -> impl Iterator<Item = Name<'a>> {
        let ast = self.ast;
        child_tokens(self.node)
            .filter(move |&index| ast.token_kind(index) == TokenKind::Ident)
            .map(move |index| ast.token_name(index))
    }
}

ast_view! {
    /// An `import path (::{…})? (as name)? ;` item.
    ImportDecl from ImportItem
}

impl<'a> ImportDecl<'a> {
    /// The imported path.
    #[must_use]
    pub fn path(self) -> Option<PathNode<'a>> {
        node_of_kind(self.node, SyntaxKind::Path).and_then(|node| PathNode::cast(self.ast, node))
    }

    /// The grouped leaves of `import path::{a, b as c};`, in source order
    /// (empty when there is no group).
    pub fn leaves(self) -> impl Iterator<Item = ImportLeaf<'a>> {
        let ast = self.ast;
        node_of_kind(self.node, SyntaxKind::ImportGroup)
            .into_iter()
            .flat_map(move |group| {
                child_nodes(group).filter_map(move |node| ImportLeaf::cast(ast, node))
            })
    }

    /// The whole-import alias of `import path as name;`.
    #[must_use]
    pub fn alias(self) -> Option<&'a str> {
        self.alias_name().map(|name| name.text)
    }

    /// The whole-import alias with its exact span.
    #[must_use]
    pub fn alias_name(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }
}

ast_view! {
    /// One leaf of an import group: `name` or `name as alias`.
    ImportLeaf from ImportLeaf
}

impl<'a> ImportLeaf<'a> {
    /// The imported name.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.name_ref().map(|name| name.text)
    }

    /// The imported name with its exact span.
    #[must_use]
    pub fn name_ref(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }

    /// The alias, if renamed.
    #[must_use]
    pub fn alias(self) -> Option<&'a str> {
        self.alias_name().map(|name| name.text)
    }

    /// The alias with its exact span, if renamed.
    #[must_use]
    pub fn alias_name(self) -> Option<Name<'a>> {
        let ast = self.ast;
        child_tokens(self.node)
            .filter(|&index| ast.token_kind(index) == TokenKind::Ident)
            .map(|index| ast.token_name(index))
            .nth(1)
    }
}

/// Implement the accessors shared by named declarations: doc comments,
/// `pub`, and the declared name.
macro_rules! decl_accessors {
    ($name:ident) => {
        impl<'a> $name<'a> {
            /// The declaration's doc comments (`///` lines), in source
            /// order.
            pub fn docs(self) -> impl Iterator<Item = &'a str> {
                let ast = self.ast;
                child_tokens(self.node)
                    .filter(move |&index| ast.token_kind(index) == TokenKind::DocComment)
                    .map(move |index| ast.token_text(index))
            }

            /// Is the declaration marked `pub`?
            #[must_use]
            pub fn is_pub(self) -> bool {
                self.ast.has_token(self.node, TokenKind::KwPub)
            }

            /// The attributes written before this declaration, with their
            /// spans, in source order (ADR-0020 Stage C).
            ///
            /// The name is returned verbatim: deciding whether an attribute
            /// exists, and whether it is allowed here, is the checker's job,
            /// so an unknown one reaches a diagnostic rather than being
            /// silently dropped.
            pub fn attributes(self) -> impl Iterator<Item = Name<'a>> {
                let ast = self.ast;
                crate::item::child_nodes(self.node)
                    .filter(|node| node.kind == SyntaxKind::Attribute)
                    .filter_map(move |node| ast.direct_token_name(node, TokenKind::Ident))
            }

            /// The declared name.
            #[must_use]
            pub fn name(self) -> Option<&'a str> {
                self.ast.direct_token_text(self.node, TokenKind::Ident)
            }

            /// The declared name with its exact span.
            #[must_use]
            pub fn name_ref(self) -> Option<Name<'a>> {
                self.ast.direct_token_name(self.node, TokenKind::Ident)
            }
        }
    };
}

ast_view! {
    /// A function definition (with body) or signature (interface member
    /// ending in `;`).
    FnDecl from FunctionItem | FunctionSignature
}
decl_accessors!(FnDecl);

impl<'a> FnDecl<'a> {
    /// Is this a body-less signature (interface member)?
    #[must_use]
    pub fn is_signature(self) -> bool {
        self.node.kind == SyntaxKind::FunctionSignature
    }

    /// The generic parameter list, if any.
    #[must_use]
    pub fn generics(self) -> Option<GenericParams<'a>> {
        node_of_kind(self.node, SyntaxKind::GenericParams)
            .and_then(|node| GenericParams::cast(self.ast, node))
    }

    /// The parameters (receiver included), in source order.
    pub fn params(self) -> impl Iterator<Item = ParamDecl<'a>> {
        let ast = self.ast;
        node_of_kind(self.node, SyntaxKind::ParamList)
            .into_iter()
            .flat_map(move |list| {
                child_nodes(list).filter_map(move |node| ParamDecl::cast(ast, node))
            })
    }

    /// The declared return type, if any.
    #[must_use]
    pub fn return_type(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        node_of_kind(self.node, SyntaxKind::ReturnType)
            .and_then(|ret| child_nodes(ret).find_map(|node| TypeRef::cast(ast, node)))
    }

    /// The `where` clause, if any.
    #[must_use]
    pub fn where_clause(self) -> Option<WhereClause<'a>> {
        node_of_kind(self.node, SyntaxKind::WhereClause)
            .and_then(|node| WhereClause::cast(self.ast, node))
    }

    /// The function body (`None` for signatures).
    #[must_use]
    pub fn body(self) -> Option<Block<'a>> {
        node_of_kind(self.node, SyntaxKind::Block).and_then(|node| Block::cast(self.ast, node))
    }
}

ast_view! {
    /// One parameter: `mode name: Type`, or the receiver `mode self`.
    ParamDecl from Param | Receiver
}

impl<'a> ParamDecl<'a> {
    /// Is this the `self` receiver?
    #[must_use]
    pub fn is_receiver(self) -> bool {
        self.node.kind == SyntaxKind::Receiver
    }

    /// The parameter mode as written (`in`, `mut`, or `take`).
    #[must_use]
    pub fn mode(self) -> Option<&'a str> {
        child_tokens(self.node)
            .next()
            .map(|index| self.ast.token_text(index))
    }

    /// The parameter name (`self` for the receiver).
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.name_ref().map(|name| name.text)
    }

    /// The parameter name with its exact span.
    #[must_use]
    pub fn name_ref(self) -> Option<Name<'a>> {
        if self.is_receiver() {
            self.ast
                .direct_token_name(self.node, TokenKind::KwSelfValue)
        } else {
            self.ast.direct_token_name(self.node, TokenKind::Ident)
        }
    }

    /// The declared type (`None` for the receiver).
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }
}

ast_view! {
    /// A bracketed generic-parameter list: `[T: Ord + Clone, U]`.
    GenericParams from GenericParams
}

impl<'a> GenericParams<'a> {
    /// The declared parameters, in source order.
    pub fn params(self) -> impl Iterator<Item = GenericParam<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| GenericParam::cast(ast, node))
    }
}

ast_view! {
    /// One generic parameter with optional bounds: `T: Ord + Clone`.
    GenericParam from GenericParam
}

impl<'a> GenericParam<'a> {
    /// The parameter name.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.ast.direct_token_text(self.node, TokenKind::Ident)
    }

    /// The parameter name with its exact span.
    #[must_use]
    pub fn name_ref(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }

    /// The bound paths, in source order.
    pub fn bounds(self) -> impl Iterator<Item = TypePath<'a>> {
        let ast = self.ast;
        node_of_kind(self.node, SyntaxKind::BoundList)
            .into_iter()
            .flat_map(move |bounds| {
                nodes_of_kind(bounds, SyntaxKind::TypePath)
                    .filter_map(move |node| TypePath::cast(ast, node))
            })
    }
}

ast_view! {
    /// A `where` clause.
    WhereClause from WhereClause
}

impl<'a> WhereClause<'a> {
    /// The predicates, in source order.
    pub fn preds(self) -> impl Iterator<Item = WherePred<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| WherePred::cast(ast, node))
    }
}

ast_view! {
    /// One `Type: Bound + Bound` predicate of a `where` clause.
    WherePred from WherePred
}

impl<'a> WherePred<'a> {
    /// The constrained type.
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }

    /// The bound paths, in source order.
    pub fn bounds(self) -> impl Iterator<Item = TypePath<'a>> {
        let ast = self.ast;
        node_of_kind(self.node, SyntaxKind::BoundList)
            .into_iter()
            .flat_map(move |bounds| {
                nodes_of_kind(bounds, SyntaxKind::TypePath)
                    .filter_map(move |node| TypePath::cast(ast, node))
            })
    }
}

ast_view! {
    /// A `struct` declaration.
    StructDecl from StructItem
}
decl_accessors!(StructDecl);

impl<'a> StructDecl<'a> {
    /// The generic parameter list, if any.
    #[must_use]
    pub fn generics(self) -> Option<GenericParams<'a>> {
        node_of_kind(self.node, SyntaxKind::GenericParams)
            .and_then(|node| GenericParams::cast(self.ast, node))
    }

    /// The `where` clause, if any.
    #[must_use]
    pub fn where_clause(self) -> Option<WhereClause<'a>> {
        node_of_kind(self.node, SyntaxKind::WhereClause)
            .and_then(|node| WhereClause::cast(self.ast, node))
    }

    /// The fields, in source order.
    pub fn fields(self) -> impl Iterator<Item = FieldDecl<'a>> {
        field_block_fields(self.ast, self.node)
    }
}

/// The `FieldDecl` views inside a node's `FieldBlock` child.
fn field_block_fields<'a>(
    ast: Ast<'a>,
    node: &'a SyntaxNode,
) -> impl Iterator<Item = FieldDecl<'a>> {
    node_of_kind(node, SyntaxKind::FieldBlock)
        .into_iter()
        .flat_map(move |block| {
            child_nodes(block).filter_map(move |field| FieldDecl::cast(ast, field))
        })
}

ast_view! {
    /// One `name: Type` field of a struct or enum variant.
    FieldDecl from FieldDecl
}
decl_accessors!(FieldDecl);

impl<'a> FieldDecl<'a> {
    /// The field's type.
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }
}

ast_view! {
    /// An `enum` declaration.
    EnumDecl from EnumItem
}
decl_accessors!(EnumDecl);

impl<'a> EnumDecl<'a> {
    /// The generic parameter list, if any.
    #[must_use]
    pub fn generics(self) -> Option<GenericParams<'a>> {
        node_of_kind(self.node, SyntaxKind::GenericParams)
            .and_then(|node| GenericParams::cast(self.ast, node))
    }

    /// The variants, in source order.
    pub fn variants(self) -> impl Iterator<Item = VariantDecl<'a>> {
        let ast = self.ast;
        nodes_of_kind(self.node, SyntaxKind::VariantDecl)
            .filter_map(move |node| VariantDecl::cast(ast, node))
    }
}

ast_view! {
    /// One enum variant, with an optional payload field block.
    VariantDecl from VariantDecl
}
decl_accessors!(VariantDecl);

impl<'a> VariantDecl<'a> {
    /// The payload fields (empty for a bare variant).
    pub fn fields(self) -> impl Iterator<Item = FieldDecl<'a>> {
        field_block_fields(self.ast, self.node)
    }
}

ast_view! {
    /// An `interface` declaration.
    InterfaceDecl from InterfaceItem
}
decl_accessors!(InterfaceDecl);

impl<'a> InterfaceDecl<'a> {
    /// The generic parameter list, if any.
    #[must_use]
    pub fn generics(self) -> Option<GenericParams<'a>> {
        node_of_kind(self.node, SyntaxKind::GenericParams)
            .and_then(|node| GenericParams::cast(self.ast, node))
    }

    /// The members — signatures and default-bodied functions — in source
    /// order.
    pub fn members(self) -> impl Iterator<Item = FnDecl<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| FnDecl::cast(ast, node))
    }
}

ast_view! {
    /// An `impl Interface for Type { … }` block.
    ImplDecl from ImplItem
}

impl<'a> ImplDecl<'a> {
    /// The generic parameter list, if any.
    #[must_use]
    pub fn generics(self) -> Option<GenericParams<'a>> {
        node_of_kind(self.node, SyntaxKind::GenericParams)
            .and_then(|node| GenericParams::cast(self.ast, node))
    }

    /// The implemented interface's path.
    #[must_use]
    pub fn interface(self) -> Option<TypePath<'a>> {
        node_of_kind(self.node, SyntaxKind::TypePath)
            .and_then(|node| TypePath::cast(self.ast, node))
    }

    /// The implemented interface's type arguments, if any.
    #[must_use]
    pub fn interface_args(self) -> Option<TypeArgs<'a>> {
        node_of_kind(self.node, SyntaxKind::TypeArguments)
            .and_then(|node| TypeArgs::cast(self.ast, node))
    }

    /// The `where` clause, if any.
    #[must_use]
    pub fn where_clause(self) -> Option<WhereClause<'a>> {
        node_of_kind(self.node, SyntaxKind::WhereClause)
            .and_then(|node| WhereClause::cast(self.ast, node))
    }

    /// The implementing type (after `for`).
    #[must_use]
    pub fn target(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }

    /// The implemented functions, in source order.
    pub fn functions(self) -> impl Iterator<Item = FnDecl<'a>> {
        let ast = self.ast;
        nodes_of_kind(self.node, SyntaxKind::FunctionItem)
            .filter_map(move |node| FnDecl::cast(ast, node))
    }
}

ast_view! {
    /// A `const NAME: Type = value;` item.
    ConstDecl from ConstItem
}
decl_accessors!(ConstDecl);

impl<'a> ConstDecl<'a> {
    /// The declared type.
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }

    /// The value expression.
    #[must_use]
    pub fn value(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node)
            .find(|node| node.kind.is_expression())
            .and_then(|node| Expr::cast(ast, node))
    }
}

ast_view! {
    /// A `spec name { … }` block.
    SpecDecl from SpecItem
}

impl<'a> SpecDecl<'a> {
    /// The spec's name (an identifier or a string literal, as written).
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        let ast = self.ast;
        child_tokens(self.node)
            .find(|&index| {
                matches!(
                    ast.token_kind(index),
                    TokenKind::Ident | TokenKind::StringLiteral
                )
            })
            .map(|index| ast.token_text(index))
    }

    /// The spec's name as a spanned identifier, when the identifier form is
    /// used. Identifier-named specs *target* the declaration of that name;
    /// string-named specs are free-standing descriptions.
    #[must_use]
    pub fn target_name(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }

    /// The spec's statements and clauses, in source order.
    pub fn statements(self) -> impl Iterator<Item = SpecStatement<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| SpecStatement::cast(ast, node))
    }
}

/// One statement inside a `spec` body.
#[derive(Clone, Copy, Debug)]
pub enum SpecStatement<'a> {
    /// A `given name: Type = value, …;` clause.
    Given(GivenClause<'a>),
    /// A `when …;` clause.
    When(WhenClause<'a>),
    /// A `then expr;` clause.
    Then(SimpleClause<'a>),
    /// An `assert expr;` clause.
    Assert(SimpleClause<'a>),
    /// A plain `let` binding.
    Let(BindingStmt<'a>),
    /// A plain `var` binding.
    Var(BindingStmt<'a>),
    /// A plain expression statement.
    Expr(crate::stmt::ExprStmt<'a>),
    /// Material skipped during error recovery.
    Error(ErrorNode<'a>),
}

impl<'a> SpecStatement<'a> {
    /// View `node` as a spec statement, if it is one.
    #[must_use]
    pub fn cast(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<Self> {
        match node.kind {
            SyntaxKind::GivenClause => GivenClause::cast(ast, node).map(Self::Given),
            SyntaxKind::WhenClause => WhenClause::cast(ast, node).map(Self::When),
            SyntaxKind::ThenClause => SimpleClause::cast(ast, node).map(Self::Then),
            SyntaxKind::AssertClause => SimpleClause::cast(ast, node).map(Self::Assert),
            SyntaxKind::LetStatement => BindingStmt::cast(ast, node).map(Self::Let),
            SyntaxKind::VarStatement => BindingStmt::cast(ast, node).map(Self::Var),
            SyntaxKind::ExpressionStatement => {
                crate::stmt::ExprStmt::cast(ast, node).map(Self::Expr)
            }
            SyntaxKind::Error => ErrorNode::cast(ast, node).map(Self::Error),
            _ => None,
        }
    }
}

ast_view! {
    /// A `given` clause: one or more typed bindings.
    GivenClause from GivenClause
}

impl<'a> GivenClause<'a> {
    /// The bindings, in source order.
    pub fn bindings(self) -> impl Iterator<Item = GivenBinding<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| GivenBinding::cast(ast, node))
    }
}

ast_view! {
    /// One `name: Type (= value)?` binding of a `given` clause.
    GivenBinding from GivenBinding
}

impl<'a> GivenBinding<'a> {
    /// The bound name.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.ast.direct_token_text(self.node, TokenKind::Ident)
    }

    /// The bound name with its exact span.
    #[must_use]
    pub fn name_ref(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }

    /// The declared type.
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }

    /// The initializer, if present.
    #[must_use]
    pub fn initializer(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node)
            .find(|node| node.kind.is_expression())
            .and_then(|node| Expr::cast(ast, node))
    }
}

ast_view! {
    /// A `when …;` clause: either an inner binding or an expression.
    WhenClause from WhenClause
}

impl<'a> WhenClause<'a> {
    /// The inner `let`/`var` binding, if that form is used.
    #[must_use]
    pub fn binding(self) -> Option<BindingStmt<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| BindingStmt::cast(ast, node))
    }

    /// The inner expression, if that form is used.
    #[must_use]
    pub fn expr(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node)
            .find(|node| node.kind.is_expression())
            .and_then(|node| Expr::cast(ast, node))
    }
}

ast_view! {
    /// A `then expr;` or `assert expr;` clause.
    SimpleClause from ThenClause | AssertClause
}

impl<'a> SimpleClause<'a> {
    /// The clause's expression.
    #[must_use]
    pub fn expr(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node)
            .find(|node| node.kind.is_expression())
            .and_then(|node| Expr::cast(ast, node))
    }
}
