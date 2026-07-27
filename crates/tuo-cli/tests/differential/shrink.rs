//! The minimized-reproduction workflow for differential failures.
//!
//! A random divergence is often large and unreadable. Before a failure is worth
//! a human's time it should be reduced to the smallest program that still
//! diverges — a *minimized reproduction*. This module implements that reduction
//! as a classic test-case shrinker: it repeatedly proposes smaller candidate
//! programs and keeps any that still (a) type-check and (b) make the two engines
//! disagree, until no proposed reduction helps. The reduced program plus a
//! report is written to disk as a saved repro the developer can run directly.
//!
//! The shrinker is oracle-driven and language-shape-aware but not parser-bound:
//! its reductions are textual (drop a helper, shorten the body, replace a
//! parenthesized subexpression with a literal), each re-validated by actually
//! re-running both engines. A reduction that breaks type-checking or removes the
//! divergence is simply rejected, so the shrinker can never turn a real finding
//! into a spurious one.

// Test-support module reached via `#[path]`; `pub` is the intended visibility.
#![allow(unreachable_pub)]

use std::path::Path;

use crate::harness::{self, Divergence, Outcome};

/// Reduce `divergence` to a minimal still-diverging program.
///
/// The oracle is the real one: a reduced candidate is kept only if it still
/// type-checks *and* the two engines still disagree on it. Returns the smallest
/// program the shrinker reached (at worst the original), so a caller can rely on
/// the result reproducing the failure.
#[must_use]
pub fn minimize(divergence: &Divergence, scratch: &Path) -> Divergence {
    minimize_by(divergence, |candidate| {
        if !harness::accepts(candidate) {
            return None;
        }
        harness::diff_program(candidate, scratch)
    })
}

/// Reduce a *synthetic* divergence whose oracle is "the interpreter still yields
/// `target`". Used to exercise the reduction machine and the saved-repro
/// artifact without a real backend disagreement (there is none to trigger).
#[must_use]
pub fn minimize_with_oracle(
    divergence: &Divergence,
    _scratch: &Path,
    target: &Outcome,
) -> Divergence {
    minimize_by(divergence, |candidate| {
        if !harness::accepts(candidate) {
            return None;
        }
        (harness::interpret(candidate) == *target).then(|| Divergence {
            source: candidate.to_owned(),
            interpreter: target.clone(),
            native: divergence.native.clone(),
        })
    })
}

/// The shared reduction loop: propose smaller candidates and keep any the
/// `oracle` accepts, iterating to a fixed point. `oracle(candidate)` returns the
/// divergence to adopt if the candidate is a valid, still-failing reduction, or
/// `None` to reject it.
fn minimize_by(
    divergence: &Divergence,
    mut oracle: impl FnMut(&str) -> Option<Divergence>,
) -> Divergence {
    let mut best = divergence.clone();

    // Iterate reduction passes to a fixed point: keep sweeping while any single
    // reduction shrank the program.
    let mut improved = true;
    let mut guard = 0;
    while improved && guard < 1000 {
        improved = false;
        guard += 1;

        for candidate in candidates(&best.source) {
            if candidate.len() >= best.source.len() {
                continue;
            }
            if let Some(smaller) = oracle(&candidate) {
                best = smaller;
                improved = true;
                break; // restart the pass from the new, smaller program
            }
        }
    }

    best
}

/// Propose reduced variants of `source`, smallest-impact first is not required —
/// the caller keeps only those that still diverge and are strictly smaller.
fn candidates(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    // 1. Drop each helper function line (every `fn` line except `main`). Removing
    //    a now-unused helper keeps the program well-typed and often shrinks it a
    //    lot; if `main` still calls it the reduction fails to type-check and is
    //    rejected by the oracle.
    for (index, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("fn ") && !line.contains("fn main(") {
            let mut kept: Vec<&str> = lines.clone();
            kept.remove(index);
            out.push(kept.join("\n"));
        }
    }

    // 2. Replace each parenthesized subexpression with a small literal. This
    //    collapses arithmetic, `if`, and call trees toward a constant while
    //    preserving Int typing.
    for replacement in ["0", "1"] {
        out.extend(replace_parenthesized(source, replacement));
    }

    // 3. Collapse an `if c { a } else { b }` to just one arm `{ a }` / `{ b }`.
    out.extend(collapse_if(source));

    out
}

/// For each top-level-balanced `(...)` group in `source`, yield a variant with
/// that group replaced by `replacement`. Only groups that actually contain an
/// operator or call (i.e. are worth collapsing) are targeted.
fn replace_parenthesized(source: &str, replacement: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    for start in 0..bytes.len() {
        if bytes[start] != b'(' {
            continue;
        }
        // Find the matching close paren.
        let mut depth = 0i32;
        let mut end = None;
        for (offset, &b) in bytes[start..].iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { continue };
        let inner = &source[start + 1..end];
        // Skip trivial groups (already a bare literal/name) — nothing to shrink.
        if !inner.contains(['+', '-', '*', '/', '%', '(']) {
            continue;
        }
        let mut reduced = String::with_capacity(source.len());
        reduced.push_str(&source[..start]);
        reduced.push_str(replacement);
        reduced.push_str(&source[end + 1..]);
        out.push(reduced);
    }
    out
}

/// For each `if COND { A } else { B }`, yield variants that keep only one arm.
fn collapse_if(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("if ") {
        let if_pos = search_from + rel;
        search_from = if_pos + 3;
        // Locate the `{ A }` then `else` then `{ B }` blocks by brace matching.
        let Some(then_open) = source[if_pos..].find('{').map(|p| if_pos + p) else {
            continue;
        };
        let Some(then_close) = matching_brace(source, then_open) else {
            continue;
        };
        let after = &source[then_close + 1..];
        let Some(else_rel) = after.find("else") else {
            continue;
        };
        let else_open_search = then_close + 1 + else_rel + "else".len();
        let Some(else_open) = source[else_open_search..]
            .find('{')
            .map(|p| else_open_search + p)
        else {
            continue;
        };
        let Some(else_close) = matching_brace(source, else_open) else {
            continue;
        };

        let then_arm = &source[then_open..=then_close]; // "{ A }"
        let else_arm = &source[else_open..=else_close]; // "{ B }"

        for arm in [then_arm, else_arm] {
            let mut reduced = String::with_capacity(source.len());
            reduced.push_str(&source[..if_pos]);
            reduced.push_str(arm);
            reduced.push_str(&source[else_close + 1..]);
            out.push(reduced);
        }
    }
    out
}

/// The index of the `}` matching the `{` at `open`, or `None` if unbalanced.
fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0i32;
    for (offset, &b) in bytes[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Write a minimized divergence to disk as a saved reproduction: the reduced
/// `.tuo` program next to a `.report` describing the disagreement and how to
/// re-run it. Returns the path of the saved program.
///
/// This is the artifact a developer opens after a differential failure: a
/// standalone program plus the exact commands to reproduce the divergence with
/// the two engines.
pub fn save_repro(divergence: &Divergence, dir: &Path, stem: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).expect("repro dir is creatable");
    let program_path = dir.join(format!("{stem}.tuo"));
    let report_path = dir.join(format!("{stem}.report"));

    std::fs::write(&program_path, &divergence.source).expect("repro program is writable");
    let report = format!(
        "{}\n\n# reproduce\n#   reference (interpreter): {}\n#   native backend:         {}\n# run natively and compare the exit status to the interpreter's value:\n#   tuo run {}\n",
        divergence.report(),
        divergence.interpreter.describe(),
        divergence.native.describe(),
        program_path.display(),
    );
    std::fs::write(&report_path, report).expect("repro report is writable");
    program_path
}
