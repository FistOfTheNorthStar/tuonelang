// map-lookup — the equivalent-semantics C peer for the tuonelang map-lookup
// workload (ADR-0011's `Map[Int, Int]`). Same computation the same way: an
// open-addressing hash table (splitmix64-finalizer hash, linear probing,
// doubling growth) receives `count` inserts (key i → 3·i), every key is
// looked back up summing values modulo 1000, and the first half is removed
// (tombstone deletion — the idiomatic C shape; the observable results agree
// since the workload never iterates). One round contributes
// 3·999·1000/2 % 1000 + 500 = 1000 for count = 1000; the round repeats and
// the result is reassigned, not accumulated, so main returns 1000 & 0xff =
// 232.

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int64_t *keys;
    int64_t *values;
    uint8_t *state; /* 0 empty, 1 live, 2 tombstone */
    uint64_t cap;
    uint64_t len;
} table;

static uint64_t mix(int64_t key) {
    uint64_t x = (uint64_t)key;
    x ^= x >> 30; x *= 0xbf58476d1ce4e5b9ULL;
    x ^= x >> 27; x *= 0x94d049bb133111ebULL;
    x ^= x >> 31;
    return x;
}

static void grow(table *t, uint64_t new_cap) {
    int64_t *keys = malloc(new_cap * sizeof(int64_t));
    int64_t *values = malloc(new_cap * sizeof(int64_t));
    uint8_t *state = calloc(new_cap, 1);
    for (uint64_t i = 0; i < t->cap; i++) {
        if (t->state[i] != 1) continue;
        uint64_t slot = mix(t->keys[i]) & (new_cap - 1);
        while (state[slot] == 1) slot = (slot + 1) & (new_cap - 1);
        keys[slot] = t->keys[i];
        values[slot] = t->values[i];
        state[slot] = 1;
    }
    free(t->keys);
    free(t->values);
    free(t->state);
    t->keys = keys;
    t->values = values;
    t->state = state;
    t->cap = new_cap;
}

static void insert(table *t, int64_t k, int64_t v) {
    if (t->cap == 0 || 2 * (t->len + 1) > t->cap) grow(t, t->cap ? t->cap * 2 : 16);
    uint64_t slot = mix(k) & (t->cap - 1);
    while (t->state[slot] == 1) {
        if (t->keys[slot] == k) {
            t->values[slot] = v;
            return;
        }
        slot = (slot + 1) & (t->cap - 1);
    }
    t->keys[slot] = k;
    t->values[slot] = v;
    t->state[slot] = 1;
    t->len += 1;
}

static int64_t get(const table *t, int64_t k, int64_t missing) {
    if (t->cap == 0) return missing;
    uint64_t slot = mix(k) & (t->cap - 1);
    while (t->state[slot] != 0) {
        if (t->state[slot] == 1 && t->keys[slot] == k) return t->values[slot];
        slot = (slot + 1) & (t->cap - 1);
    }
    return missing;
}

static void del(table *t, int64_t k) {
    if (t->cap == 0) return;
    uint64_t slot = mix(k) & (t->cap - 1);
    while (t->state[slot] != 0) {
        if (t->state[slot] == 1 && t->keys[slot] == k) {
            t->state[slot] = 2;
            t->len -= 1;
            return;
        }
        slot = (slot + 1) & (t->cap - 1);
    }
}

static long long round_once(long long count) {
    table t = {0, 0, 0, 0, 0};
    for (long long i = 0; i < count; i++) insert(&t, i, i * 3);
    long long hits = 0;
    for (long long j = 0; j < count; j++) hits = (hits + get(&t, j, 0)) % 1000;
    for (long long k = 0; k < count / 2; k++) del(&t, k);
    long long result = hits + (long long)t.len;
    free(t.keys);
    free(t.values);
    free(t.state);
    return result;
}

int main(void) {
    long long result = 0;
    for (int r = 0; r < 50; r++) {
        result = round_once(1000);
    }
    return (int)result; /* 1000 & 0xff = 232 via the process exit byte */
}
