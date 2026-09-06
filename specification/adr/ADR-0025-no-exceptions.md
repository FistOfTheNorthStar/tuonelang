# ADR-0025: No exceptions — `Result[T, E]` and the trap are the two failure modes

- **Status:** accepted
- **Date:** 2026-09-06

## Context

tuonelang has no exceptions, and never has. That is a decision rather than an
omission, but until now it has been recorded only implicitly — as the absence of
a `throw`/`try` production in the grammar, and as scattered notes in the cheat
sheet's anti-pattern table. This ADR states it explicitly so the reasoning is
durable and reviewable, per the ADR process.

The prompt for writing it down came from dogfooding: `tools/py2tuo`, the
Python-subset transcoder, must refuse `try` / `except` / `raise`, and the
refusal needs a citable rationale rather than "the compiler does not implement
it". A user reading that diagnostic should be able to find out *why*.

## Decision

tuonelang has exactly **two** failure mechanisms, and adds no third.

### 1. `Result[T, E]` — expected, recoverable failure

A function that can fail returns `Result[T, E]`, and the caller must `match` it.
The failure is in the type, so it cannot be ignored silently, and control flow
stays visible in the source: there is no path through a function that the reader
cannot see. `std::core` carries the combinators (`is_ok`/`is_err`/`ok_or`/`ok`)
and the error-chain model (`error_new`/`error_wrap`/`error_is`/`error_root`).

### 2. The trap — programmer error, not a recoverable condition

Integer overflow, division by zero, and out-of-bounds indexing **abort** with a
deterministic `TrapCode`, a stderr message, and a fixed exit status
(`tuo-runtime`). These are not catchable and are not intended to be: they mark a
program that has already left its own specification. `x + 1` on `i64::MAX`
prints `tuo: trap: integer overflow` and aborts.

**Neither is an exception**, and the distinction is the point: `Result` is
visible in the signature and handled by the caller; a trap is invisible in the
signature because it is not part of the contract.

## Why not exceptions

**They defeat the language's central promise.** tuonelang's stated invariant is
that a program that type-checks means what it says. An exception is an invisible
control-flow edge out of *every* call site — a signature stops describing what a
function does, and the reader cannot see the paths. That is the same class of
implicitness the language rejects elsewhere (no inferred signatures, no default
arguments, no overloading, explicit parameter modes).

**They interact badly with the ownership model.** ADR-0003's model is call-scoped:
a borrow lives for a call, and drop placement is determined by the checker.
Unwinding introduces an exit path from the middle of any expression, so every
partially-initialized place needs drop glue on a second, invisible edge. This is
where unwinding costs real complexity in languages that have it, and it would
land directly on the machinery ADR-0009 and ADR-0012 built.

**They are hostile to the LLM-generation goal.** tuonelang exists to be easy for
a coding agent to write *correctly*. Exceptions are the archetypal construct a
model gets subtly wrong: catching too broadly, catching too late, or silently
swallowing. `Result` forces the decision to the one place a reader and a
compiler can both see it.

**They are unnecessary.** The two mechanisms above cover the space: recoverable
failure is a value, and unrecoverable failure is an abort. The middle ground
exceptions occupy — recoverable-but-invisible — is the part deliberately
omitted.

## Consequences

*Easier:* every failure path is visible in a type. A reviewer can enumerate how
a function can fail by reading its signature. There is no unwinding machinery in
the ABI, no invisible drop edge, and no `panic = "unwind"` axis in the backends.

*Harder:* error propagation is manual. tuonelang has no `?` operator, so a
call chain that can fail is a chain of `match`. This is real friction and is
accepted deliberately — `std::core`'s error chains exist to make it tolerable.
**A `?`-style propagation operator is not precluded by this ADR** and would be a
reasonable later increment; it is sugar over `Result`, not an exception.

*For translation from exception-based languages* (`py2tuo` and any successor):
`try`/`except`/`raise` are refused with a diagnostic naming `Result[T, E]`. This
is a genuine expressiveness gap and the refusal says so — a Python function that
raises must be restructured, not mechanically rewritten, because the translation
requires choosing an error type, which is a design decision the transcoder is
not entitled to make.

*Revisit if:* a `?`-style operator proves insufficient for real error-handling
ergonomics in programs larger than the current examples. That would be an
argument for better `Result` ergonomics, not for exceptions.
