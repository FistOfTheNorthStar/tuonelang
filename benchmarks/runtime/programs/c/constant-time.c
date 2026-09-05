/* constant-time — the equivalent-semantics C peer for the tuonelang
 * constant-time workload (ADR-0020). The identical computation: the same
 * 32-byte tags, the same branchless and early-returning comparisons over the
 * same alternating best-case/worst-case inputs, the same 1000 rounds.
 *
 * The masking idioms are written the same way as the tuonelang side rather
 * than the way C would conventionally write them. In particular the mask uses
 * the sign-smearing shift `(bit << 63) >> 63` instead of the conventional
 * `0 - bit`: tuonelang's `-` traps on i64::MIN, so its `std::ct` cannot use
 * the conventional spelling, and a peer that used the cheaper idiom would be
 * measuring a different program. Equivalent semantics means the same
 * algorithm, not merely the same answer.
 *
 * Signed shifts are used deliberately so the arithmetic right shift smears the
 * sign bit, matching tuonelang's Int (a signed 64-bit type). 500,000
 * agreements reduced mod 256 gives the exit byte 32. */
#include <stdint.h>

static int64_t ct_or_reduce(int64_t x) {
    int64_t a = x | (x >> 32);
    int64_t b = a | (a >> 16);
    int64_t c = b | (b >> 8);
    int64_t d = c | (c >> 4);
    int64_t e = d | (d >> 2);
    int64_t f = e | (e >> 1);
    return f & 1;
}

static int64_t ct_is_zero(int64_t x) { return ct_or_reduce(x) ^ 1; }

/* The branchless comparison: constant work for a given length. */
static int64_t ct_bytes_eq(const int64_t *a, const int64_t *b, int64_t n) {
    int64_t diff = 0;
    for (int64_t i = 0; i < n; i++) {
        diff |= a[i] ^ b[i];
    }
    return ct_is_zero(diff);
}

/* The early-returning comparison: the vulnerability, for comparison. */
static int64_t naive_bytes_eq(const int64_t *a, const int64_t *b, int64_t n) {
    for (int64_t i = 0; i < n; i++) {
        if (a[i] != b[i]) {
            return 0;
        }
    }
    return 1;
}

static void make_tag(int64_t seed, int64_t *out) {
    for (int64_t i = 0; i < 32; i++) {
        out[i] = (seed + i * 7) & 255;
    }
}

int main(void) {
    int64_t reference[32], early_mismatch[32], equal[32];
    make_tag(11, reference);
    make_tag(12, early_mismatch);
    make_tag(11, equal);

    int64_t agreements = 0;
    for (int64_t round = 0; round < 500000; round++) {
        int64_t same_ct, same_naive;
        if ((round & 1) == 0) {
            same_ct = ct_bytes_eq(reference, early_mismatch, 32);
            same_naive = naive_bytes_eq(reference, early_mismatch, 32);
        } else {
            same_ct = ct_bytes_eq(reference, equal, 32);
            same_naive = naive_bytes_eq(reference, equal, 32);
        }
        if (same_ct == same_naive) {
            agreements++;
        }
    }
    return (int)(agreements % 256);
}
