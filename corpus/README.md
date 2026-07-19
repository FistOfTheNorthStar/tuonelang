# tuonelang corpus

This directory will hold tuonelang source programs used to exercise and validate the
compiler.

**No source code is considered part of the trusted tuonelang corpus unless it is
eventually compiler-validated.** A file living here confers no guarantee: until
the compiler can lex, parse, check, and (where applicable) execute a program,
its presence is provisional. Corpus entries graduate to "trusted" only once the
compiler validates them, and the tooling that performs that validation does not
yet exist.

The corpus is intentionally empty at this stage.

All corpus programs must be canonically formatted (`tuo fmt --check` clean);
the formatter defines tuonelang's single canonical source representation.
