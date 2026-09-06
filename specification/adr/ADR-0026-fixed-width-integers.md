# ADR-0026: `Int` is `I64` and traps — fixed-width integers, and why `std::bignum` is not the same thing

- **Status:** accepted
- **Date:** 2026-09-06

## Context

tuonelang's `Int` is an alias for `I64`: a fixed-width, 64-bit signed integer
whose overflow **traps** rather than wrapping. `std::bignum` provides
arbitrary-precision non-negative integers as a library, over 28-bit limbs.

Both facts are documented, but their *relationship* is not, and it is the
relationship that matters. This ADR records why the fixed-width default is
right, and — more usefully — why having `std::bignum` in the library does not
close the gap with a language whose integers are unbounded.

The prompt came from dogfooding. `tools/py2tuo` refuses Python integer literals
wider than 64 bits, and the obvious objection is "but `std::bignum` exists".
That objection is wrong, for a reason worth writing down.

## Decision

`Int` is `I64`. Arithmetic that leaves the range **traps**; it does not wrap and
does not promote. `std::bignum` remains a library type that a program opts into
explicitly, and **no implicit promotion from `Int` to a bignum exists or will**.

### Why fixed width

**It is what the machine has.** tuonelang compiles to native code with a
matching integer model, which is what makes the C peer an apt comparison in
every performance-lab workload. An unbounded default would put an allocation
behind `+`.

**It is predictable.** A `Map[Int, Int]` entry is a known size; a struct's layout
is computable by `abi::layout_of` from the types alone. Unbounded integers make
every containing type dynamically sized.

### Why trapping rather than wrapping

Wraparound is silent and almost never intended. A trap converts a class of
silent wrong answers into a loud, deterministic abort with a fixed exit status —
consistent with the language's refusal to mis-compile. Where modular arithmetic
*is* intended, it is written explicitly: `std::bits`'s `add32`/`mul32` exist
precisely because a checksum needs wrapping and plain `+` correctly refuses to
provide it (ADR-0019).

### Why `std::bignum` does not close the gap

This is the load-bearing part.

`std::bignum` is a **type you choose**. Python's `int` is a type you *cannot
avoid*: every integer literal, every `+`, every loop counter is unbounded, and a
computation silently grows past 64 bits without the program mentioning it. The
gap is not "tuonelang lacks big integers" — it has them, and they verifiably
exceed `i64` range (`mul` of two `i64::MAX` values reports 126 bits). The gap is
that **Python's unboundedness is implicit and tuonelang's is opt-in**.

No library can close that gap, because closing it would mean changing what `+`
on `Int` *means* — implicit promotion. That is rejected here for three reasons:

1. **It would make arithmetic allocate.** `a + b` could heap-allocate depending
   on runtime values, so no arithmetic expression would be allocation-free, in a
   language whose allocator boundary is deliberately explicit (ADR-0009).
2. **It would break layout.** `Int` would no longer have a size, so every
   struct, array, and map containing one would become dynamically sized.
3. **It would destroy the trap's meaning.** Overflow would stop being an error
   and become a representation change, removing the signal that a computation
   left its intended range.

`std::bignum` also carries a documented **non-goal**: it is deliberately *not*
constant time (ADR-0022), so it is safe for public values and unsafe for secrets.
That is a further reason it is an explicit opt-in rather than a silent
substrate.

## Consequences

*Easier:* predictable layout and cost. A native integer operation is a native
integer operation. The C peer comparison in the performance lab stays honest,
and `abi::layout_of` stays a pure function of the type.

*Harder:* a computation that legitimately exceeds 64 bits must say so, by
switching to `std::bignum` explicitly and accepting its API (free functions over
`Array[Int]` limbs) and its cost.

*For translation from unbounded-integer languages:* a Python integer literal
wider than 64 bits is refused with a diagnostic that names the mismatch and
points at `std::bignum`. `py2tuo` deliberately does **not** silently translate
Python `int` arithmetic into bignum calls: doing so would make every translated
program pay allocation for arithmetic, and would misrepresent the source
program's cost. The refusal is a faithful report that the two languages disagree
about what an integer is.

*Revisit if:* a checked-promotion type (an `Int` that widens on overflow rather
than trapping, opted into per binding) proves valuable. That would be a *new
type* with explicit semantics, not a change to `Int`, and it would need its own
ADR and benchmark plan.
