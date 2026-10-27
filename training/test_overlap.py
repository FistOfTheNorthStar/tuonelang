"""Tests for the near-duplicate detection in ``similarity.py`` and
``overlap.py``. Run with ``python3 -m unittest discover -s training``.

The examples here are public material on purpose: an eval task never goes
in a test.
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from harvest import Harvested  # noqa: E402
from overlap import (  # noqa: E402
    BENCH_PROGRAM,
    EVAL_PROGRAM,
    TRAIN_PROGRAM,
    benchmark_overlap,
    eval_contamination,
    harvest_duplicates,
    seed_duplicates,
)
from seeds import Seed  # noqa: E402
from similarity import Item, program_similarity, shape  # noqa: E402

GCD = """\
fn gcd(take a: Int, take b: Int) -> Int {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

spec gcd {
    then gcd(12, 8) == 4;
}
"""

# The same program renamed, re-commented, re-specced, in a module.
EUCLID = """\
module solution;

// Euclid's algorithm.
pub fn euclid(take x: Int, take y: Int) -> Int {
    if y == 0 {
        x
    } else {
        euclid(y, x % y)
    }
}

spec "euclid of coprimes" {
    then euclid(17, 5) == 1;
    then euclid(9, 0) == 9;
}
"""

SUM = """\
fn total(in xs: [Int; 4]) -> Int {
    var sum = 0;
    for x in xs {
        sum = sum + x;
    }
    sum
}
"""

# A different program over the same kind of input.
LARGEST = """\
fn largest(in xs: [Int; 4]) -> Int {
    var best = xs[0];
    for x in xs {
        if x > best {
            best = x;
        }
    }
    best
}
"""

DOUBLE = "fn double(take x: Int) -> Int {\n    x + x\n}\n"
TWICE = "fn twice(take n: Int) -> Int {\n    n + n\n}\n"


def item(program: str, task: str = "", label: str = "x") -> Item:
    return Item(label, task, program)


def seed(name: str, program: str, task: str = "", concept: str = "c") -> Seed:
    return Seed(concept=concept, task=task or f"Write `{name}`.", solution=program, name=name)


class SimilarityTest(unittest.TestCase):
    def test_a_renamed_copy_is_identical(self):
        self.assertEqual(program_similarity(item(GCD), item(EUCLID)), 1.0)

    def test_specs_comments_and_literals_do_not_count(self):
        self.assertEqual(
            shape("fn f() -> Int { 1 } // one\nspec f { then f() == 1; }"),
            shape('fn g() -> Int { 2 }\nspec "g" { then g() == 2; }'),
        )
        self.assertEqual(shape('fn f() -> Str { "a" }'), shape('fn h() -> Str { "b" }'))

    def test_paths_and_type_names_are_kept(self):
        self.assertNotEqual(shape("fn f() { std::core::min(1, 2) }"), shape("fn f() { std::core::max(1, 2) }"))
        self.assertNotEqual(shape("fn f(in p: Point) {}"), shape("fn f(in p: Rect) {}"))

    def test_different_programs_are_below_every_threshold(self):
        score = program_similarity(item(SUM), item(LARGEST))
        self.assertLess(score, min(EVAL_PROGRAM, BENCH_PROGRAM, TRAIN_PROGRAM))

    def test_a_short_program_matches_only_an_identical_shape(self):
        self.assertEqual(program_similarity(item(DOUBLE), item(TWICE)), 1.0)
        triple = "fn triple(take x: Int) -> Int {\n    x + x + x\n}\n"
        self.assertEqual(program_similarity(item(DOUBLE), item(triple)), 0.0)

    def test_a_trivial_shape_never_matches(self):
        self.assertEqual(program_similarity(item("fn f() {}"), item("fn g() {}")), 0.0)
        self.assertEqual(program_similarity(item(""), item("")), 0.0)


class OverlapTest(unittest.TestCase):
    def test_a_seed_that_copies_a_benchmark_reference_is_refused(self):
        # `spec-first-gcd`'s reference solution is GCD, renamed `gcd`.
        found = benchmark_overlap([seed("euclid", EUCLID)], [])
        self.assertIn("benchmark:spec-first-gcd", [m.b.label for m in found])

    def test_two_seeds_that_are_the_same_program_are_refused(self):
        found = seed_duplicates([seed("gcd", GCD), seed("euclid", EUCLID, concept="d")])
        self.assertEqual([(m.a.label, m.b.label) for m in found], [("seed:c/gcd", "seed:d/euclid")])
        self.assertEqual(seed_duplicates([seed("total", SUM), seed("largest", LARGEST)]), [])

    def test_a_harvested_copy_of_a_seed_is_dropped(self):
        harvested = [
            Harvested(origin="corpus", name="twice", task="t", solution=TWICE),
            Harvested(origin="corpus", name="largest", task="t", solution=LARGEST),
            Harvested(origin="corpus", name="largest2", task="t", solution=LARGEST),
        ]
        self.assertEqual(harvest_duplicates([seed("double", DOUBLE)], harvested), [0, 2])

    def test_an_eval_task_close_to_training_material_is_refused(self):
        found = eval_contamination([seed("euclid", EUCLID)], [seed("gcd", GCD)], [])
        self.assertTrue(any("eval:c/euclid ~ seed:c/gcd" in f for f in found), found)

    def test_an_eval_task_with_a_paraphrased_task_is_refused(self):
        task = "Write `scale(n)` returning n multiplied by the configured factor, with specs."
        found = eval_contamination(
            [seed("scale", SUM, task=task)],
            [seed("grow", LARGEST, task=task.replace("with specs", "with a spec"))],
            [],
        )
        self.assertTrue(any("task similarity" in f for f in found), found)

    def test_an_eval_task_named_after_a_benchmark_function_is_refused(self):
        found = eval_contamination([seed("gcd", SUM)], [], [])
        self.assertTrue(any("spec-first-gcd" in f for f in found), found)

    def test_distinct_material_passes(self):
        self.assertEqual(eval_contamination([seed("largest", LARGEST)], [seed("total", SUM)], []), [])


if __name__ == "__main__":
    unittest.main()
