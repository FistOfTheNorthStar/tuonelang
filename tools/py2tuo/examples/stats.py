"""Summary statistics over a list of integers."""


def total(xs: list[int]) -> int:
    """Sum every value."""
    acc = 0
    for x in xs:
        acc = acc + x
    return acc


def largest(xs: list[int], fallback: int) -> int:
    """The largest value, or the fallback when empty."""
    best = fallback
    for x in xs:
        if x > best:
            best = x
    return best


def mean_floor(xs: list[int]) -> int:
    """The arithmetic mean, rounded toward zero."""
    n = len(xs)
    if n == 0:
        return 0
    return total(xs) // n


def main() -> int:
    xs = [1, 2, 3]
    return total(xs)
