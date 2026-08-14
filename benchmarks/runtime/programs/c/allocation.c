// allocation — the equivalent-semantics C peer for the tuonelang allocation
// workload. Same computation the same way: a growable long-long vector filled
// 0..count by push (doubling growth via realloc), summed; and a growable byte
// buffer filled to `count` bytes the same way, measured by length. Each round's
// buffers are freed at scope end; the round is repeated so the workload really
// exercises malloc/realloc/free throughput. The observable result is one
// round's contribution (reassigned, not accumulated), matching the tuonelang
// program: for count = 16 the vector sums to 120 and the buffer is 16 bytes, so
// main returns 136.

#include <stdlib.h>
#include <string.h>

static long long array_sum(int count) {
    long long *xs = NULL;
    long long len = 0;
    long long cap = 0;
    for (int i = 0; i < count; i++) {
        if (len == cap) {
            cap = cap == 0 ? 1 : cap * 2;
            xs = (long long *)realloc(xs, (size_t)cap * sizeof(long long));
        }
        xs[len++] = i;
    }
    long long total = 0;
    for (long long j = 0; j < len; j++) {
        total += xs[j];
    }
    free(xs);
    return total;
}

static long long string_len(int count) {
    unsigned char *s = NULL;
    long long len = 0;
    long long cap = 0;
    for (int i = 0; i < count; i++) {
        if (len == cap) {
            cap = cap == 0 ? 1 : cap * 2;
            s = (unsigned char *)realloc(s, (size_t)cap);
        }
        s[len++] = 65;
    }
    free(s);
    return len;
}

static long long round_once(int count) {
    return array_sum(count) + string_len(count);
}

int main(void) {
    long long result = 0;
    for (int r = 0; r < 2000; r++) {
        result = round_once(16);
    }
    return (int)result;
}
