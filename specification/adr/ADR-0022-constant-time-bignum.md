# ADR-0022: A constant-time bignum — why `std::bignum` cannot simply adopt `std::ct`

- **Status:** proposed (2026-09-05)
- **Date:** 2026-09-05
- **Context:** ADR-0020 shipped `std::ct` and the compiler-verified
  `#[constant_time]` attribute; its adoption note then recorded that
  `std::crypto` took the constant-time comparison up but **`std::bignum` did
  not**, keeping its "deliberately not constant time" caveat unchanged.

  That looks like unfinished adoption work and is not. This ADR exists to
  record why, because the difference decides how large the work is: adopting
  `std::ct` in `std::crypto` was one function (`verify`, delegating to
  `bytes_eq`), whereas in `std::bignum` there is nothing to delegate to. The
  variable-time behaviour is in the **algorithms**, not in the spelling of
  individual operations.

  Concretely, in the shipped module:

  | Site | What leaks |
  |---|---|
  | `mul`'s `if ai != 0` zero-limb skip | how many zero limbs the operands have |
  | `normalized` trimming trailing zero limbs | the *length* of every result, and so every later loop bound |
  | `div_small`, `cmp`, the decimal conversions | short-circuit on the values given |

  Replacing each `if` with `std::ct::select` would make the module **slower and
  still variable-time**, because the loop bounds themselves are data. A masked
  select cannot fix a loop that runs a data-dependent number of times.

  Worse, it would be actively harmful: the module would then *look* like it had
  adopted the constant-time subset while keeping the leak, which is the failure
  mode ADR-0020 exists to prevent. An honest caveat beats a misleading
  adoption.

- **Decision:** *Not yet taken.* A constant-time bignum is a **different
  implementation**, not a modification of this one, and it should be written
  only when there is a program that needs it.

## What a constant-time bignum requires

Four changes, none of them local:

1. **Fixed-width limb arrays with no normalization.** Every value carries the
   same number of limbs regardless of magnitude, so no length is
   value-dependent. This is the change that forces all the others.
2. **Schoolbook multiplication with no zero-skip.** Every limb pair is
   multiplied, including the ones that are zero.
3. **Montgomery reduction instead of division.** Modular reduction is the
   operation public-key cryptography actually needs, and long division is
   irreducibly data-dependent; Montgomery form replaces it with multiplication
   and shifts.
4. **Constant-time comparison and conditional subtraction**, built on
   `std::ct::select` — the one place the existing subset applies directly.

Only (4) is adoption. (1)–(3) are new algorithms.

## Why this is not being written now

**Nothing in the workspace needs it.** The dogfooding chain that produced
ADR-0019 and ADR-0020 was driven by a PostgreSQL connector, and that connector
authenticates with SCRAM-SHA-256 — SHA-256, HMAC, and PBKDF2, all of which are
fixed-width and already constant time in `std::crypto`. `std::bignum` exists
for parsing, formatting, and protocol arithmetic on **public** values, which is
what it is documented for and what it is used for.

The programs that would need a constant-time bignum are RSA, Diffie-Hellman,
and elliptic-curve operations — and those need modular exponentiation, modular
inversion, and division by a bignum, none of which `std::bignum` has either.
The constant-time requirement arrives *with* those algorithms rather than
before them, so this ADR is naturally a companion to whichever ADR adds them,
not a prerequisite.

This also keeps ADR-0019's TLS exclusion intact and for the same reason: TLS
needs X.509, a certificate store, and AEAD ciphers on top of all of the above.

## Consequences (if implemented)

- **The two implementations must not be confusable.** Whatever ships should be
  a separate module (`std::bignum_ct` or similar) rather than a flag on this
  one, so a caller cannot believe they have the constant-time version because a
  parameter was set. The type should differ, so the compiler enforces the
  distinction.
- **A benchmark plan is required**, per the project rule, and the shape is
  known from ADR-0020's `constant-time` workload: it measures a cost
  deliberately paid. A constant-time bignum is substantially slower than this
  one — no zero-skip, no early exit, fixed width — and the benchmark must
  report that honestly rather than being tuned to hide it.
- **The 28-bit limb size is a correctness property and carries over.**
  Schoolbook multiplication accumulates `limb + limb*limb + carry`, which is 56
  bits at 28-bit limbs and would overflow — that is, *trap* — at 32. Any new
  implementation inherits this constraint.
- **`#[constant_time]` will not accept these functions as written**, since the
  checker refuses loops and indexing. Whether it can be extended to accept
  *public-length* loops is the same question ADR-0021 raises as its question
  (2), and the two ADRs should be resolved together.

## Amendment (2026-09-28) — the need has arrived, and the trap tension with it

Two statements above were true when written and are no longer.

**"Nothing in the workspace needs it" is superseded.** The 2026-09-15 TLS
round added `std::x25519`, `std::ed25519`, and `std::tls`, and all three
declare `std::bignum` as a dependency (`DECLARED_DEPENDENCIES`,
`tuo-cli/tests/stdlib.rs`). X25519's field arithmetic is `std::bignum`'s
`add`/`sub`/`mul`/`compare` behind a curve-specific reduction, and the scalar
it multiplies by is a private key. Each module says so in its header, and
`std::tls` names the limitation as the one that matters most. This ADR
predicted that the requirement would arrive *with* the elliptic-curve
algorithms. It did. The prediction that this ADR would then be "a companion
to whichever ADR adds them" did not hold, because no ADR added them; they
landed as stdlib modules. This amendment is the record that the programs
which need a constant-time bignum now exist in the catalog.

**"This also keeps ADR-0019's TLS exclusion intact" is superseded** for the
same reason. The exclusion was overturned; see DOGFOODING.md's 2026-09-15
update. What survives of it is this ADR's subject: the stack demonstrates that
the protocol is expressible and is not deployable, and the distance between
those two is a constant-time bignum.

**The trap tension ADR-0020 recorded belongs here.** ADR-0020 noted that
trapping arithmetic and constant-time code are in conflict, sidestepped the
conflict by building its primitives from shifts alone, and said a future ADR
"may need to widen" the wrapping-arithmetic escape hatch. That ADR is this
one, and the point becomes requirement (5):

5. **Limb arithmetic that cannot trap.** A trap is a data-dependent branch to
   an abort, so fixed-width limb code must be written such that no `+` or `*`
   can overflow for *any* input, and the checker must be able to see that.
   The 28-bit limb already guarantees it for schoolbook multiplication (the
   56-bit bound above). What is missing is the means to state it: today
   `#[constant_time]` refuses trapping arithmetic outright (`T0019`), which is
   sound and rejects every bignum.

The existing workarounds show the shape of the problem. `std::bits::add32` and
`mul32` compute modular results by masking, and `std::sha512::add64` adds
through 32-bit halves because two words with their high bits set would
otherwise abort the process. The 32-bit pair is referenced on 49 lines across
the catalog. All of them are correct, and none is verified: each is an
argument in a comment that an intermediate stays in range.

Two ways to discharge (5) are open, and choosing between them is part of
accepting this ADR rather than something this amendment settles:

- **Wrapping operators as builtins** (`std::bits::wrapping_add` and peers),
  lowered to the machine's native modular arithmetic. Simple, and it makes
  `add32`/`add64` one instruction instead of several. It also adds a second
  integer arithmetic to a language whose first is deliberately the trapping
  one (ADR-0026), so its use would need to stay visibly exceptional.
- **Range reasoning in the checker**, so that masked operands are known to be
  in range and the `+` between them is provably non-trapping. No new
  arithmetic, and the existing workarounds become verified rather than
  replaced. It is a substantially larger piece of work, and ADR-0021's
  question (2) on public-length loops is the same kind of reasoning.

Either way the benchmark plan above gains a second measurement: `sha256-hash`
and the forthcoming bignum workload before and after, since the first option
changes the emitted code of every hash in the catalog.

## What this ADR does not propose

Changing `std::bignum`. Its caveat is a correct description of what it does,
now stated precisely enough that a reader can see the leak is algorithmic
rather than a missing call to `std::ct`. It remains safe for public values and
unsafe for secrets, which is what it says.
