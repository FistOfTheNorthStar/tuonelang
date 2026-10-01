# ADR-0029: A native, correctly-rounded square root — `std::float::sqrt`

- **Status:** accepted
- **Date:** 2026-10-02

## Context

The Computer Language Benchmarks Game workloads added to the performance lab
measured `nbody` at **6.3x** the time of its C, Rust, and Go peers, while the
other supported programs ran within 1.4x. The difference was one function.
v0 had no square root in the language; `std::math::sqrt` was written in
tuonelang as twenty Newton–Raphson iterations starting from `x`:

```tuo
var guess = x;
while i < 20 { guess = (guess + x / guess) / 2.0; ... }
```

Every peer computes the root with the hardware's single square-root
instruction. `nbody` takes one root per body pair per step, so tuonelang paid
twenty divisions where each peer paid one instruction.

Measuring it also showed the function was **wrong**, not just slow. Newton's
iteration started at `x` first halves its guess on every step until it nears
the root, and twenty steps are not enough for an input far from 1:

| input | old `std::math::sqrt` | correct |
|---|---|---|
| `2.0` | `1.414213562373095` | `1.4142135623730951` (off by one ulp) |
| `1e10` | `100000.00015603233` | `100000.0` |
| `1e300` | `9.5367431640625e293` | `1e150` |
| `1e-310` | `9.5367431640625e-7` | `9.999999999999986e-156` |

The old spec bounded the error only at `sqrt(2.0)` and checked four perfect
squares near 1, so none of this was visible.

## Decision

Add one pure builtin, **`std::float::sqrt(take x: Float) -> Float`**: IEEE 754's
square root, **correctly rounded**. It follows IEEE 754 exactly at the edges:
`sqrt(-1.0)` is NaN, `sqrt(-0.0)` is `-0.0`, `sqrt(+inf)` is `+inf`. It never
traps.

It is lowered as a new MIR unary operator, `UnOp::Sqrt`:

- **interpreter:** Rust's `f64::sqrt` (an `F32` operand: `f32::sqrt`), which
  the Rust standard library guarantees is correctly rounded;
- **Cranelift:** the `sqrt` instruction;
- **LLVM:** the `llvm.sqrt` intrinsic, which without fast-math flags may not be
  approximated.

All three are therefore the same correctly-rounded function, and the engines
agree bit for bit, pinned three-way by
`tests/codegen/fixtures/flt_sqrt.tuo` (exact roots, an irrational root, a
huge and a subnormal input, `+inf`, NaN, and the sign of `-0.0`).

**`std::math::sqrt` keeps its documented contract** (0.0 for a non-positive
input, so a caller need not guard against NaN) and now delegates to the
builtin for positive inputs. Its spec pins exact values, including the large
and subnormal cases the Newton loop got wrong. `std::math::sqrt` remains the
one obvious square root for a program; `std::float::sqrt` is the primitive
beneath it, as `std::rt::write` is beneath `std::io::print`, and is the
spelling for code that wants IEEE's NaN and `-0.0` behaviour.

### Why a builtin and not a better library function

A library function cannot reach the instruction: tuonelang has no inline
assembly, no FFI, and no intrinsics. A better software algorithm would still
cost tens of instructions against one, and would have to be proven correctly
rounded, which the hardware already is.

### Why a new module, `std::float`

`std::math` is a catalog module written in tuonelang. A builtin installed at
`std::math` would collide with the catalog's own `sqrt`. The builtin modules
are named for the type they operate on (`std::str`, `std::string`,
`std::array`, `std::map`); `std::float` follows that rule and is the home for
any later float primitive with a hardware instruction behind it.

## Consequences

- `nbody` went from 6.3x to 1.17x C in the speed table
  (`benchmarks_game_speed_table`); `spectral-norm` takes one root, so its time
  is unchanged.
- Every program that called `std::math::sqrt` with an input far from 1 now
  gets the right answer. The change is observable for such inputs, and the
  old results were wrong.
- Adding the builtin installs two symbols (the `std::float` module and
  `sqrt`), which renumbers later symbol IDs in the MIR optimization goldens;
  the re-blessed goldens differ only in those IDs.
- No ABI change: no runtime symbol is added, since both backends emit the
  instruction inline.
