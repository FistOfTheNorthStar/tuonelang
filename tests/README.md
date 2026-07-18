# tuonelang compiler test corpus

This directory holds end-to-end and stage-level tests that exercise the
compiler through its public surfaces. It is organized by compiler stage so that
each phase can be validated in isolation as it is implemented.

`lexer/` holds the tokenizer's fixture + snapshot corpus (see its README).
The other categories contain no tests yet — the corresponding compiler stages
do not exist. The directories and this document define where those tests will
live and what each category is responsible for.

| Directory | Purpose |
|-----------|---------|
| `lexer/` | Tokenization: token kinds, spans, and lexical error recovery. |
| `parser/` | Parsing: syntax/AST shape and parse-error diagnostics. |
| `diagnostics/` | Diagnostic quality: codes, messages, labels, and suggestions. |
| `types/` | Type checking and inference behavior. |
| `ownership/` | Ownership and memory-safety enforcement. |
| `mir/` | MIR construction and its verified invariants. |
| `specs/` | Colocated executable specifications run via the MIR interpreter. |
| `codegen/` | Native code generation (Cranelift and LLVM backends). |
| `differential/` | Cross-backend differential tests: interpreter vs. native backends must agree. |
