//! Extracting the *facts* a corpus entry records: which language features a
//! validated program uses, plus the declaration and spec counts that feed its
//! complexity measure and its native-entry detection.
//!
//! Feature detection has two honest sources. Declaration kinds
//! (function/struct/enum/interface/const/spec, and whether more than one module
//! is declared) come from the resolved **symbol table**, which is exact. The
//! remaining constructs — imports, control flow, mutability, arithmetic,
//! ownership-mode markers — are not distinct symbols, so they are detected by
//! **lexing** the source and looking for their keywords/operators. Lexical
//! detection is conservative: a feature is reported only when its token is
//! present, and only for programs the front end already accepted (so the tokens
//! are known to be meaningful, not fragments of a malformed file).

use tuo_compiler::lexer::{self, TokenKind};
use tuo_resolve::{Resolution, SymbolKind};
use tuo_source::{SourceId, SourceMap};

use crate::metadata::{Complexity, Feature, FeatureSet};

/// The name of the native entry function (the v0 entry ABI).
const ENTRY: &str = "main";

/// The facts extracted from a validated program.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProgramFacts {
    /// The language features the program exercises.
    pub features: FeatureSet,
    /// Distinct module-level declarations (functions, structs, enums,
    /// interfaces, consts, and specs).
    pub item_count: usize,
    /// Colocated specs.
    pub spec_count: usize,
    /// Whether the program declares a function named `main` (a runnable native
    /// entry). The backend still decides whether it is a *valid* entry; this is
    /// only whether native execution is worth attempting.
    pub has_native_entry: bool,
}

impl ProgramFacts {
    /// The coarse complexity measure for this program over `map`/`sources`.
    #[must_use]
    pub fn complexity(&self, map: &SourceMap, sources: &[SourceId]) -> Complexity {
        let mut bytes = 0;
        let mut lines = 0;
        for &id in sources {
            let text = map.source(id).text();
            bytes += text.len();
            lines += text.lines().filter(|line| !line.trim().is_empty()).count();
        }
        Complexity {
            bytes,
            lines,
            items: self.item_count,
            specs: self.spec_count,
            features: self.features.len(),
        }
    }
}

/// Extract the facts of the program formed by `sources` (drawn from `map`),
/// using its `resolution` for exact declaration information.
#[must_use]
pub fn extract_facts(
    map: &SourceMap,
    sources: &[SourceId],
    resolution: &Resolution,
) -> ProgramFacts {
    let mut features = FeatureSet::new();
    let mut item_count = 0;
    let mut spec_count = 0;
    let mut module_count = 0;
    let mut has_native_entry = false;

    for (_, symbol) in resolution.symbols() {
        // Only count *user-written* declarations. Prelude symbols (the
        // canonical `Option`/`Result` enums and their variants) and implicit
        // prefix modules carry no declaration span, so they are present in
        // every program and would falsely report features the source never
        // wrote. Skipping them keeps feature detection honest.
        if symbol.declaration.is_none() {
            continue;
        }
        match symbol.kind {
            SymbolKind::Function => {
                features.insert(Feature::Function);
                item_count += 1;
                if symbol.name == ENTRY {
                    has_native_entry = true;
                }
            }
            SymbolKind::Struct => {
                features.insert(Feature::Struct);
                item_count += 1;
            }
            SymbolKind::Enum => {
                features.insert(Feature::Enum);
                item_count += 1;
            }
            SymbolKind::Interface => {
                features.insert(Feature::Interface);
                item_count += 1;
            }
            SymbolKind::Const => {
                features.insert(Feature::Const);
                item_count += 1;
            }
            SymbolKind::Spec => {
                features.insert(Feature::Spec);
                spec_count += 1;
                item_count += 1;
            }
            SymbolKind::Module => module_count += 1,
            // Variants, params, locals, and type params are not top-level items.
            SymbolKind::Variant | SymbolKind::Param | SymbolKind::Local | SymbolKind::TypeParam => {
            }
        }
    }

    if module_count > 1 {
        features.insert(Feature::MultiModule);
    }

    // Lexical features: constructs the symbol table does not surface. Lex each
    // source and note the keywords/operators present.
    for &id in sources {
        detect_lexical_features(map.source(id).text(), &mut features);
    }

    ProgramFacts {
        features,
        item_count,
        spec_count,
        has_native_entry,
    }
}

/// Add every lexically-detected feature present in `text` to `features`.
fn detect_lexical_features(text: &str, features: &mut FeatureSet) {
    // Build a throwaway source to lex; the corpus already validated this text,
    // so lexing is only reading back keywords, never diagnosing.
    let mut map = SourceMap::new();
    let file = map.intern_file("<facts>");
    let Ok(source) = map.add_source(file, text.to_string()) else {
        return;
    };
    for token in lexer::lex(map.source(source)).tokens {
        match token.kind {
            TokenKind::KwImport => features.insert(Feature::Import),
            TokenKind::KwIf | TokenKind::KwElse => features.insert(Feature::Conditional),
            TokenKind::KwMatch => features.insert(Feature::Match),
            TokenKind::KwWhile | TokenKind::KwLoop | TokenKind::KwFor => {
                features.insert(Feature::Loop);
            }
            TokenKind::KwVar => features.insert(Feature::Mutability),
            TokenKind::KwTake | TokenKind::KwMut | TokenKind::KwMove => {
                features.insert(Feature::OwnershipModes);
            }
            TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash => {
                features.insert(Feature::Arithmetic);
            }
            _ => {}
        }
    }
}
