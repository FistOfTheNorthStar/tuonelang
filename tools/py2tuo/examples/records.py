"""Record types -- what @dataclass translation is for.

Every program built from records has one natural Python spelling: a class.
This one is a small work queue, exercising construction, field reads and
writes, a struct as a parameter and a return type, and a list of structs.
"""

from dataclasses import dataclass


@dataclass
class Task:
    """One unit of work."""

    id: int
    retries: int
    done: bool


def make(id: int) -> Task:
    """A fresh, unattempted task."""
    return Task(id=id, retries=0, done=False)


def attempt(t: Task) -> int:
    """Record one attempt; succeed once three have been made."""
    t.retries = t.retries + 1
    if t.retries >= 3:
        t.done = True
    return t.retries


def pending(ts: list[Task]) -> int:
    """How many tasks are not yet done."""
    left = 0
    i = 0
    while i < len(ts):
        if not ts[i].done:
            left += 1
        i += 1
    return left


def main() -> int:
    first = make(1)
    a = attempt(first)
    b = attempt(first)
    c = attempt(first)

    batch = [make(2), make(3)]
    return c * 100 + pending(batch) * 10 + first.retries + a + b
