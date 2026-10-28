"""Tests for the held-out scorer ``score_eval.py``. Run with
``python3 -m unittest discover -s training``.

The record here is made up and public on purpose: an eval task never goes in
a test. The ``score_one`` tests run the real compiler (``TUO_BIN``, or
``cargo run`` without it).
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from score_eval import extract_code, score_one  # noqa: E402

DOUBLE = """\
fn double(take x: Int) -> Int {
    x + x
}

spec double {
    then double(3) == 6;
}
"""

RECORD = {
    "reference_solution": DOUBLE,
    "heldout_specs": 'spec "double of zero" {\n    then double(0) == 0;\n}\n',
    "runnable": False,
    "run_exit": None,
}


class ExtractCode(unittest.TestCase):
    def test_takes_the_first_block_under_any_info_string(self):
        reply = "Here it is:\n```rust\nfn a() -> Int {\n    1\n}\n```\n```tuo\nfn b() -> Int {\n    2\n}\n```\n"
        self.assertEqual(extract_code(reply), "fn a() -> Int {\n    1\n}\n")

    def test_drops_reasoning_before_looking_for_a_block(self):
        reply = "<think>maybe ```tuo\nfn wrong() {}\n```</think>\n```tuo\nfn right() -> Int {\n    1\n}\n```"
        self.assertEqual(extract_code(reply), "fn right() -> Int {\n    1\n}\n")

    def test_unclosed_reasoning_leaves_nothing(self):
        self.assertEqual(extract_code("<think>still thinking when the cap hit"), "\n")

    def test_a_reply_without_a_fence_is_taken_whole(self):
        self.assertEqual(extract_code("  fn a() -> Int {\n    1\n}  "), "fn a() -> Int {\n    1\n}\n")


class ScoreOne(unittest.TestCase):
    def test_the_reference_passes_everything(self):
        self.assertEqual(
            score_one(DOUBLE, RECORD),
            {"compile": True, "spec": True, "test": True, "run": None},
        )

    def test_an_empty_completion_fails_everything(self):
        # `tuo check` accepts an empty file and its spec run reports 0 of 0.
        self.assertEqual(
            score_one("\n", RECORD),
            {"compile": False, "spec": False, "test": False, "run": None},
        )

    def test_a_completion_without_specs_fails_spec_pass(self):
        bare = "fn double(take x: Int) -> Int {\n    x + x\n}\n"
        self.assertEqual(
            score_one(bare, RECORD),
            {"compile": True, "spec": False, "test": True, "run": None},
        )

    def test_no_specs_is_fine_when_the_reference_has_none(self):
        bare = "fn double(take x: Int) -> Int {\n    x + x\n}\n"
        record = {**RECORD, "reference_solution": bare, "heldout_specs": ""}
        self.assertEqual(
            score_one(bare, record),
            {"compile": True, "spec": True, "test": None, "run": None},
        )

    def test_a_task_that_does_not_compile_fails_its_held_out_specs(self):
        self.assertEqual(
            score_one("fn double(take x: Int) -> Int {\n    let mut y = x;\n    y\n}\n", RECORD),
            {"compile": False, "spec": False, "test": False, "run": None},
        )


if __name__ == "__main__":
    unittest.main()
