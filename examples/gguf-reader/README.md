# gguf-reader — a little-endian, 64-bit container format

The GGUF header, metadata table, and tensor descriptors, parsed end to end in
tuonelang: the whole structural walk a model loader performs *before* it touches
a single weight.

```bash
tuo check examples/gguf-reader/src/main.tuo examples/gguf-reader/src/std_str.tuo
tuo test  --manifest examples/gguf-reader              # 31 passed, 0 failed
tuo run   examples/gguf-reader/src/main.tuo examples/gguf-reader/src/std_str.tuo ; echo $?   # 2
tuo run --release examples/gguf-reader/src/main.tuo examples/gguf-reader/src/std_str.tuo ; echo $?  # 2
```

## Why this example exists

It was written to answer a question **empirically**, not to exercise a feature.

Every other wire-format example in this tree is **big-endian**: PostgreSQL's
frame length, the SHA-256 block, the `wire-decode` benchmark workload. That is
not a coincidence — ADR-0019 Stage A was opened to serve a network protocol, and
network byte order is big-endian by convention. So the operator surface it added
(`&`, `|`, `^`, `~`, `<<`, `>>`) and the `std::bits` helpers built on it
(`be32`, `be16`, `byte_of_be32`) had only ever been exercised in one direction.

GGUF is the opposite case in every respect: **little-endian**, **64-bit**, and a
**memory-mappable file format** rather than a stream of framed messages — its
fields are sized to be read straight into registers, not to be delimited on a
wire. If ADR-0019's surface had been quietly fitted to PostgreSQL, this is where
it would show.

**It generalizes.** `le16_at`, `le32_at`, and `le64_at` are written in the
shipped operators with no new language feature, no new builtin, and no compiler
change. That is the finding, and it is why this example produced **no ADR** —
per the project's rule, an ADR follows a gap a real program actually hit, and
this program did not hit one.

## What it covers

* **The header** — the magic `"GGUF"`, a little-endian `u32` version, and two
  little-endian `u64` counts (tensors, metadata pairs). 24 fixed bytes, the only
  fixed-size region in the format.
* **The metadata table** — `n_kv` entries, each a length-prefixed UTF-8 key, a
  `u32` type tag, and a value whose encoding depends on that tag. A model's
  architecture, context length, and quantization all live here.
* **The tensor descriptors** — `n_tensors` entries, each a name, a dimension
  count, that many `u64` extents, a type tag, and a `u64` blob-relative offset.
  Descriptors are variable-width, so reaching descriptor *n* means walking the
  *n* before it exactly right.
* **The alignment rule** — the data blob begins at the descriptor table's end
  rounded up to `general.alignment` (default 32), and each tensor's offset is
  relative to *that*, not to the file. Both halves are easy to get right alone
  and easy to get wrong together, so `tensor_data_offset` composes them and is
  spec'd directly.

The specs assert against a fixture built **byte by byte from the format's own
encoding rules**, not against the parser's own reasoning — a bug that survived
would have to be made twice, in opposite directions.

That is still two implementations by one author, so `src/cross_check.tuo` adds a
**third**: the byte offsets it asserts come from an independent Python `struct`
implementation written from the GGUF v3 spec alone, which reports the descriptor
table ending at 209, the blob beginning at 224 (209 rounded up to the declared
alignment of 32), tensor 0 at 224, tensor 1 at 736, and tensor 0 holding 16x8 =
128 elements. It exits 100 only when the parser agrees with all of them, pinning
the alignment rule and the blob-relative-to-file-absolute composition — the
format's two classic bug sources — against an outside authority.

```bash
tuo run examples/gguf-reader/src/cross_check.tuo \
        examples/gguf-reader/src/main.tuo \
        examples/gguf-reader/src/std_str.tuo ; echo $?    # 100
```

## The one boundary it found: `u64` vs signed `Int`

GGUF's counts, string lengths, dimensions, and offsets are all **unsigned
64-bit**. tuonelang's `Int` is signed `I64`. Values below 2^63 round-trip
exactly — which is every real model file by an enormous margin, 2^63 bytes being
9.2 exabytes — but a value with **bit 63 set has no representation at all**.

This is a genuine language limit, not an oversight in the parser, and the
compiler is consistent about it: `std::bits` stops at 32-bit assembly, and
`std::bits::test_bit` documents that bit 63 is excluded because `1 << 63`
overflows a signed `Int`. (`1 << 63` does type-check, and evaluates to
`i64::MIN` — a *negative* number, useless as a length or an offset.)

The boundary is **exact, not approximate**, and the spec pins both edges:
`2^63 - 1` (`i64::MAX`) decodes correctly and `2^63` is the first value refused.
An off-by-one here would either reject a legal offset or admit an
unrepresentable one.

`le64_at` therefore **refuses** such a value with `-1`, the same sentinel a
truncated buffer gets, rather than wrapping it into a negative length that a
caller would go on to use as an array bound. Its high word is combined with
`* 4294967296` rather than `<< 32` deliberately: multiplication **traps** on
overflow, so the refusal is checked before it can be observed. A corrupt file
gets a diagnosable `-1`; it never gets a silently wrong offset.

**Why this is a documented limit and not an ADR.** No real GGUF file reaches
2^63, so no program is blocked — and the project's rule is that a language
change follows a demonstrated need, never a hypothetical one. The honest
response is a refusal at the boundary plus a spec that pins it, which is what
`spec "sixty_four_bit_values"` does. If a dogfooding target ever genuinely needs
the full `u64` range, *that* is the program that earns the ADR, and it would
arrive with a name and a benchmark rather than as a speculative widening.

## What it deliberately does not cover

**No tensor data is read, and nothing is dequantized.** That is the honest
boundary. A descriptor table is pure structure over bytes and is exactly what v0
expresses; reading the blob means `mmap` and SIMD over `f16`, and v0 has
neither. The parser is pure and spec-checked, so the program is hermetic — its
exit byte is decided by the specs, not by any model file on disk.

The metadata walk also **reports rather than guesses** on array-typed entries
(tag 9): stepping over one requires its element type and count, and v0 has no
sum type over mixed element types to decode it *into*. `u32_metadata` returns
`-1` there rather than desynchronizing the table.

## Compiler feedback while writing it

Four diagnostics, all actionable, none requiring a look at compiler source:

* `P0002` on `push_le(mut out, …)` — `mut` is a parameter mode, not written at
  the call site.
* `R0002: no `eq` in `std::str`` with `= actual: eq` — `Str` compares with `==`.
* `R0002: cannot find `header` in this scope` carrying the fix directly:
  *"an identifier-named spec targets a module-level function; use a string name
  for a free-standing spec"*.
* `M0005: bb18 reads _21 after it was moved out` — an owned `String` returned by
  `tensor_name` and moved into `std::string::len` inside a loop; binding it
  first fixes it. The ownership checker caught this before any backend saw it.
