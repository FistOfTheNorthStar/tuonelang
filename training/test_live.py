"""Tests for live model attempts: how ``seeds.py`` attaches them and how
``generate.py`` turns one into compiler feedback. Run with
``python3 -m unittest discover -s training``.

The ``feedback`` tests run the real compiler (``TUO_BIN``, or ``cargo run``
without it).
"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from generate import feedback  # noqa: E402
from seeds import Seed, SeedError, attach_attempts  # noqa: E402

SEED = Seed(concept="loops", task="Write `double(x)`.", solution="", name="double")


class AttachAttempts(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())

    def write(self, relative: str, text: str = "fn f() {}\n"):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_attempts_attach_to_their_seed_in_run_order(self):
        for run in ("b-run", "a-run"):
            self.write(f"{run}/run.json", "{}")
            self.write(f"{run}/loops/double.tuo", f"// {run}\n")
        (seed,) = attach_attempts([SEED], self.root)
        self.assertEqual(seed.attempts, (("a-run", "// a-run\n"), ("b-run", "// b-run\n")))

    def test_no_live_directory_leaves_the_seeds_alone(self):
        self.assertEqual(attach_attempts([SEED], self.root / "missing"), [SEED])

    def test_an_attempt_at_no_seeds_task_is_refused(self):
        self.write("run/run.json", "{}")
        self.write("run/loops/renamed.tuo")
        with self.assertRaisesRegex(SeedError, "loops/renamed"):
            attach_attempts([SEED], self.root)

    def test_a_run_without_its_record_is_refused(self):
        self.write("run/loops/double.tuo")
        with self.assertRaisesRegex(SeedError, "run.json"):
            attach_attempts([SEED], self.root)

    def test_a_stray_file_in_a_run_is_refused(self):
        self.write("run/run.json", "{}")
        self.write("run/loops/double.txt")
        with self.assertRaisesRegex(SeedError, "not attempts"):
            attach_attempts([SEED], self.root)


class Feedback(unittest.TestCase):
    def test_a_front_end_error_is_its_first_diagnostic(self):
        report, command, kind = feedback("fn double(take x: Int) -> Int {\n    let mut y = x;\n    y + y\n}\n")
        self.assertEqual((command, kind), ("check", "parse"))
        self.assertIn("not `let mut`", report)

    def test_a_failing_spec_is_its_report(self):
        wrong = "fn double(take x: Int) -> Int {\n    x + 1\n}\n\nspec double {\n    then double(3) == 6;\n}\n"
        report, command, kind = feedback(wrong)
        self.assertEqual((command, kind), ("spec", "spec"))
        self.assertIn("FAILED", report)

    def test_a_valid_program_has_no_feedback(self):
        right = "fn double(take x: Int) -> Int {\n    x + x\n}\n\nspec double {\n    then double(3) == 6;\n}\n"
        self.assertIsNone(feedback(right))


if __name__ == "__main__":
    unittest.main()
