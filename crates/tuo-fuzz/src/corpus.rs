//! A deterministic corpus of source inputs, shared by every stage's robustness
//! sweep.
//!
//! Fuzzing the compiler well means feeding it three qualitatively different
//! kinds of input: raw byte soup (does arbitrary UTF-8 crash the front end?),
//! token-fragment soup (do plausible-but-broken token sequences confuse the
//! parser?), and near-valid programs (do the deep stages — resolve, types,
//! ownership, MIR, the interpreter — survive input that is *shaped* like a
//! program but need not be well-formed?). This module generates all three from
//! one fixed-seed PRNG, so every stage's sweep draws from the same corpus and a
//! failure at any stage reproduces exactly from its seed.
//!
//! # Determinism
//!
//! The generator is a pure function of its `u64` seed — a self-contained
//! SplitMix64, never the `rand` crate or any wall-clock/entropy source. Same
//! seed, same input, on every host and every run, so the CI sweep is stable and
//! a failing seed is a permanent, replayable reproduction. This mirrors the
//! differential suite's generator (`tuo-cli/tests/differential/generator.rs`),
//! which the [`super::stages`] checkers complement: that generator emits only
//! *accepted* programs to compare engines; this one deliberately emits malformed
//! input to prove the pipeline never crashes on it.

/// A deterministic SplitMix64 PRNG — pure, seedable, host-independent.
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator. Any `u64` is a valid, distinct seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 pseudo-random bits (SplitMix64).
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` (`bound` must be non-zero).
    pub fn below(&mut self, bound: u32) -> u32 {
        (self.next_u64() % u64::from(bound)) as u32
    }

    /// Pick a random element of a non-empty slice.
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u32) as usize]
    }
}

/// The three input flavors the corpus mixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Arbitrary bytes, lossily decoded to a `str` (the compiler only lexes
    /// `str`). Exercises the raw robustness of the lexer and parser.
    ByteSoup,
    /// Concatenated token-shaped fragments — plausible but usually ill-formed
    /// programs. Exercises parser recovery and the shallow semantic stages.
    TokenSoup,
    /// Program-shaped input built from item/statement skeletons: mostly
    /// parseable, often failing later stages. Exercises resolve/types/ownership
    /// /MIR on realistic-looking-but-broken code.
    NearProgram,
}

impl Flavor {
    /// The full set, for a sweep that wants to cover every flavor.
    #[must_use]
    pub fn all() -> [Flavor; 3] {
        [Flavor::ByteSoup, Flavor::TokenSoup, Flavor::NearProgram]
    }
}

/// Generate one input string of the given flavor from `seed`.
///
/// Pure in `(flavor, seed)`: the same pair always yields the same string.
#[must_use]
pub fn input(flavor: Flavor, seed: u64) -> String {
    let mut rng = Rng::new(seed);
    match flavor {
        Flavor::ByteSoup => byte_soup(&mut rng),
        Flavor::TokenSoup => token_soup(&mut rng),
        Flavor::NearProgram => near_program(&mut rng),
    }
}

/// Arbitrary bytes, lossily decoded. Short by design — the interesting crashes
/// hide in small, dense inputs, and a lossy short string still covers control
/// bytes, stray high bytes, and truncated multi-byte sequences.
fn byte_soup(rng: &mut Rng) -> String {
    let len = (rng.below(64)) as usize;
    let bytes: Vec<u8> = (0..len).map(|_| (rng.below(256)) as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Lexer-relevant fragments: keywords, punctuation, operators, partial
/// literals, comment starts, and a few non-ASCII graphemes. Concatenating a
/// handful produces plausible-but-broken token streams.
const TOKEN_FRAGMENTS: &[&str] = &[
    "fn ", "f", "()", "{", "}", "(", ")", "[", "]", "let ", "x", "=", "==", "!=", ";", ",", ":",
    "::", "->", "..", "|", "&", "+", "-", "*", "/", "%", "<", ">", "spec ", "then ", "assert ",
    "struct ", "enum ", "match ", "=>", "if ", "else ", "loop ", "while ", "for ", "in ",
    "return ", "break", "take ", "borrow ", "Int", "Bool", "0", "1", "0x", "0b1", "1.5", "1_000",
    "\"", "\"s\"", "'a'", "'l:", "//c\n", "///d\n", "\n", " ", "\t", "🦀", "ä", "_", "true",
    "false", "mut ",
];

fn token_soup(rng: &mut Rng) -> String {
    let picks = rng.below(20) as usize;
    (0..picks).map(|_| *rng.pick(TOKEN_FRAGMENTS)).collect()
}

/// Program-shaped skeletons. Each is nearly parseable and, when it parses,
/// stresses a deeper stage: an undefined name (resolve), a type clash (types), a
/// double move (ownership), a call chain (MIR + interpreter). Filling a `{HOLE}`
/// with another skeleton or a token fragment keeps the result program-shaped
/// while varying it.
const PROGRAM_SKELETONS: &[&str] = &[
    "fn main() -> Int { {HOLE} }",
    "fn f(take x: Int) -> Int { x + {HOLE} }",
    "fn main() -> Int { let y = {HOLE}; y }",
    "fn main() -> Int { if {HOLE} { 1 } else { 0 } }",
    "fn g() -> Int { undefined_name({HOLE}) }",
    "fn main() -> Bool { {HOLE} == 1 }",
    "fn main() -> Int { let b = mkbox(); consume(b); consume({HOLE}) }",
    "fn main() -> Int { loop { break {HOLE}; } }",
    "struct S { x: Int } fn main() -> Int { {HOLE} }",
    "fn main() -> Int { let xs = [{HOLE}, 2, 3]; xs[0] }",
    "fn main() -> Int { let xs = [{HOLE}; 4]; xs[3] }",
    "fn main() -> Int { let xs: [Int; 2] = {HOLE}; xs[1] }",
    "fn main() -> Int { var t = 0; for x in [{HOLE}, 1] { t = t + x; } t }",
    "enum E { A, B } fn main() -> Int { {HOLE} }",
    "fn rec(take n: Int) -> Int { if n == 0 { 0 } else { rec(n - {HOLE}) } }",
    "spec main { then main() == {HOLE}; }",
];

const HOLE_LEAVES: &[&str] = &[
    "0",
    "1",
    "-1",
    "x",
    "y",
    "42",
    "true",
    "false",
    "n",
    "b",
    "\"s\"",
    "1.5",
    "()",
    "S { x: 1 }",
    "E::A",
    "undefined",
    "1 + 1",
    "f(2)",
    "[1, 2]",
    "[0; 3]",
    "[[1]; 2]",
    "[]",
    "[1; -1]",
    "[1, 2,]",
];

fn near_program(rng: &mut Rng) -> String {
    let skeleton = rng.pick(PROGRAM_SKELETONS);
    // Depth-bounded hole filling so the result stays small and terminating.
    fill(skeleton, rng, 3)
}

/// Replace the first `{HOLE}` in `template` with a randomly chosen filler,
/// recursing up to `depth` times so a skeleton can be nested inside a skeleton.
fn fill(template: &str, rng: &mut Rng, depth: u32) -> String {
    let Some(pos) = template.find("{HOLE}") else {
        return template.to_string();
    };
    let filler = if depth == 0 || rng.below(3) == 0 {
        (*rng.pick(HOLE_LEAVES)).to_string()
    } else {
        match rng.below(3) {
            0 => token_soup(rng),
            1 => fill(rng.pick(PROGRAM_SKELETONS), rng, depth - 1),
            _ => (*rng.pick(HOLE_LEAVES)).to_string(),
        }
    };
    let mut out = String::with_capacity(template.len() + filler.len());
    out.push_str(&template[..pos]);
    out.push_str(&filler);
    out.push_str(&template[pos + "{HOLE}".len()..]);
    // Fill any remaining holes with leaves so nothing unparseable leaks through.
    while let Some(next) = out.find("{HOLE}") {
        let leaf = *rng.pick(HOLE_LEAVES);
        out.replace_range(next..next + "{HOLE}".len(), leaf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_in_seed_and_flavor() {
        for flavor in Flavor::all() {
            for seed in 0..50 {
                assert_eq!(
                    input(flavor, seed),
                    input(flavor, seed),
                    "flavor {flavor:?} seed {seed} must reproduce"
                );
            }
        }
    }

    #[test]
    fn distinct_seeds_mostly_produce_distinct_inputs() {
        // Not a hard guarantee (collisions are possible), but a healthy
        // generator varies its output; assert the population is not degenerate.
        for flavor in Flavor::all() {
            let mut seen = std::collections::BTreeSet::new();
            for seed in 0..200 {
                seen.insert(input(flavor, seed));
            }
            assert!(
                seen.len() > 20,
                "flavor {flavor:?} produced only {} distinct inputs",
                seen.len()
            );
        }
    }

    #[test]
    fn near_programs_have_no_unfilled_holes() {
        for seed in 0..500 {
            let text = input(Flavor::NearProgram, seed);
            assert!(
                !text.contains("{HOLE}"),
                "unfilled hole at seed {seed}: {text:?}"
            );
        }
    }
}
