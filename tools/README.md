# tuonelang developer tools

Auxiliary developer tooling that supports building tuonelang but is not part of
the shipped compiler.

Tools:

- **[`tokenizer-lab`](tokenizer-lab/)** — a data-driven harness that measures how
  candidate tuonelang syntax tokenizes across multiple tokenizers, so syntax is
  not designed around a single tokenizer's quirks. See its README for usage and
  for how to add tokenizer adapters without changing the core.
