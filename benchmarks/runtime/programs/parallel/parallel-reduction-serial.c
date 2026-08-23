// parallel-reduction (serial baseline) — the equivalent-semantics C peer for
// the tuonelang serial reduction. Same computation the same way: four chunks
// of 60000 seeds through the same range-bounded `busy` mixing, chunk sums
// folded on one thread. Exit byte: 64.

static long long busy(long long seed) {
    long long x = seed;
    for (long long k = 0; k < 40; k++) {
        x = (x * 31 + seed + k) % 1048576;
    }
    return x;
}

static long long chunk_sum(long long w) {
    long long total = 0;
    long long end = (w + 1) * 60000;
    for (long long i = w * 60000; i < end; i++) {
        total = (total + busy(i)) % 1048576;
    }
    return total;
}

int main(void) {
    long long total = 0;
    for (long long w = 0; w < 4; w++) {
        total += chunk_sum(w);
    }
    return (int)total; /* the process exit byte is total & 0xff = 64 */
}
