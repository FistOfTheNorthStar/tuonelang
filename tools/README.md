# tuonelang developer tools

Auxiliary developer tooling that supports building tuonelang but is not part of
the shipped compiler.

Tools:

- **[`py2tuo`](py2tuo/)** — a compiler from a **typed subset of Python** to
  tuonelang source. Translates the overlap between the two languages exactly and
  refuses everything else with a positioned diagnostic, rather than emitting
  tuonelang that means something different. Its output is verified by the real
  `tuo` binary, and its tests compare the translated program's runtime answer
  against CPython's. Python-only; nothing in the workspace depends on it.

- **[`tokenizer-lab`](tokenizer-lab/)** — a data-driven harness that measures how
  candidate tuonelang syntax tokenizes across multiple tokenizers, so syntax is
  not designed around a single tokenizer's quirks. See its README for usage and
  for how to add tokenizer adapters without changing the core.
