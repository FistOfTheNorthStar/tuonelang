/* wrapping-add64 — the equivalent-semantics C peer for the tuonelang
 * wrapping-add64 workload (ADR-0031). The identical loop: 50,000,000 rounds
 * of a modular 64-bit add and a rotate right by 29 on uint64_t, starting from
 * the same seeds; the exit byte is the low byte of x ^ y = 231. */
#include <stdint.h>

int main(void) {
    uint64_t x = 1, y = 11400714819323198485ull;
    for (long i = 0; i < 50000000; i++) {
        x = x + y;
        y = y ^ x;
        y = (y >> 29) | (y << 35);
    }
    return (int)((x ^ y) & 255);
}
