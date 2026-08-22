// parallel-reduction (parallel) — the equivalent-semantics C peer for the
// tuonelang parallel reduction: the identical four chunk sums computed on
// four POSIX threads and folded, mirroring `std::rt::par_map`'s structured
// fork-join with the same thread count. Same `busy`, same chunks, same
// arithmetic, same exit byte (64) as the serial baseline — only the thread
// count differs.

#include <pthread.h>

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

typedef struct {
    long long worker;
    long long result;
} chunk_ctx;

static void *chunk_worker(void *arg) {
    chunk_ctx *ctx = (chunk_ctx *)arg;
    ctx->result = chunk_sum(ctx->worker);
    return 0;
}

int main(void) {
    pthread_t threads[4];
    chunk_ctx ctx[4];
    for (long long w = 0; w < 4; w++) {
        ctx[w].worker = w;
        ctx[w].result = 0;
        pthread_create(&threads[w], 0, chunk_worker, &ctx[w]);
    }
    long long total = 0;
    for (long long w = 0; w < 4; w++) {
        pthread_join(threads[w], 0);
        total += ctx[w].result;
    }
    return (int)total; /* the process exit byte is total & 0xff = 64 */
}
