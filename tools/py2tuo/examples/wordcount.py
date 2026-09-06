"""Keyed aggregation -- the dict shape tuonelang actually has."""


def tally(keys: list[int]) -> int:
    """Count occurrences, then report the most-seen key's count."""
    counts: dict[int, int] = {}
    i = 0
    while i < len(keys):
        k = keys[i]
        counts[k] = counts.get(k, 0) + 1
        i += 1

    best = 0
    ks = list(counts.keys())
    j = 0
    while j < len(ks):
        seen = counts.get(ks[j], 0)
        if seen > best:
            best = seen
        j += 1
    return best


def main() -> int:
    xs = [4, 7, 4]
    return tally(xs) * 10 + len(xs)
