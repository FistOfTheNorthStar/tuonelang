/* fannkuch-redux — the equivalent-semantics C peer for the tuonelang
 * fannkuch-redux workload (the Benchmarks Game's fannkuch-redux): for every
 * permutation of 0..N, count the prefix reversals ("pancake flips") until 0
 * reaches the front; report the maximum and an alternating-sign checksum.
 * The identical computation: the same permutation order (the Benchmarks
 * Game's rotation scheme), the same flip loop. For N = 10 the checksum is
 * 73196 and the maximum 38; the exit byte is (checksum + max) % 256. */
#define N 10

int main(void) {
    long long perm[N], perm1[N], count[N];
    long long max_flips = 0, checksum = 0, perm_count = 0;
    long long r = N;
    for (long long i = 0; i < N; i++) perm1[i] = i;
    for (;;) {
        while (r != 1) {
            count[r - 1] = r;
            r = r - 1;
        }
        for (long long i = 0; i < N; i++) perm[i] = perm1[i];
        long long flips = 0;
        long long k = perm[0];
        while (k != 0) {
            long long lo = 0, hi = k;
            while (lo < hi) {
                long long t = perm[lo];
                perm[lo] = perm[hi];
                perm[hi] = t;
                lo = lo + 1;
                hi = hi - 1;
            }
            flips = flips + 1;
            k = perm[0];
        }
        if (flips > max_flips) max_flips = flips;
        if (perm_count % 2 == 0) checksum = checksum + flips;
        else checksum = checksum - flips;
        for (;;) {
            if (r == N) return (int)((checksum + max_flips) % 256);
            long long perm0 = perm1[0];
            for (long long i = 0; i < r; i++) perm1[i] = perm1[i + 1];
            perm1[r] = perm0;
            count[r] = count[r] - 1;
            if (count[r] > 0) break;
            r = r + 1;
        }
        perm_count = perm_count + 1;
    }
}
