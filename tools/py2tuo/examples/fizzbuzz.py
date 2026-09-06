"""Classification and counting -- the runnable core, without strings."""


def classify(n: int) -> int:
    """0 = plain, 1 = fizz, 2 = buzz, 3 = fizzbuzz."""
    fizz = n % 3 == 0
    buzz = n % 5 == 0
    if fizz and buzz:
        return 3
    elif fizz:
        return 1
    elif buzz:
        return 2
    else:
        return 0


def count_kind(limit: int, kind: int) -> int:
    """How many of 1..limit classify as `kind`."""
    seen = 0
    for n in range(1, limit):
        if classify(n) == kind:
            seen += 1
    return seen


def main() -> int:
    return count_kind(100, 3)
