# Fixed parser benchmark corpus

The frozen inputs behind the parser architecture decision gate
(`specification/adr/ADR-parser-strategy.md`). They are consumed by
`crates/tuo-parser/benches/parser_compare.rs` (Criterion throughput),
`crates/tuo-parser/examples/parse_memory.rs` (dhat heap profile), and the
`oracle_parity` differential tests.

| File | Contents |
|------|----------|
| `clean.tuo` | 64 repetitions of a varied, well-formed unit: generic struct + enum + interface + impl + const + function + spec. Zero diagnostics expected. |
| `expr_heavy.tuo` | 96 functions of dense expression code: method chains, turbofish, casts, matches, struct literals, ranges, assignment. Zero diagnostics expected. |
| `error_heavy.tuo` | 72 units mixing valid items with 4 deliberate error sites each (broken statement, broken item, broken tail), exercising recovery. 288 diagnostics and 216 intact functions expected. |

**These files are fixed.** Benchmark results are only comparable against the
same bytes, so do not edit them casually. They were generated
deterministically; if the grammar ever changes incompatibly, regenerate a
*new* corpus (documenting the generator), re-run the gate benchmarks, and
note the reset in the ADR.

Run the measurements with:

```bash
cargo bench -p tuo-parser --bench parser_compare
cargo run -p tuo-parser --release --example parse_memory
```
