/* channels — the equivalent-semantics C peer for the tuonelang channels
 * workload (ADR-0015). The identical sequence through the same kind of
 * structure the tuonelang runtime uses: a mutex-and-condvar FIFO of
 * malloc'd nodes. Per round, send 500 values through the queue (lock,
 * append, signal, unlock) and receive all 500 back (lock, wait-while-empty,
 * pop, unlock) — the identical locked crossings, single-threaded. The
 * observable result is one round's receive count (reassigned, not
 * accumulated): 500, exit byte 500 & 0xff = 244. */
#include <pthread.h>
#include <stdlib.h>

typedef struct node {
    long long value;
    struct node *next;
} node;

static pthread_mutex_t q_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t q_ready = PTHREAD_COND_INITIALIZER;
static node *q_head = 0;
static node *q_tail = 0;

static long long q_send(long long v) {
    node *n = (node *)malloc(sizeof(node));
    if (!n) return -1;
    n->value = v;
    n->next = 0;
    pthread_mutex_lock(&q_lock);
    if (q_tail) q_tail->next = n; else q_head = n;
    q_tail = n;
    pthread_cond_signal(&q_ready);
    pthread_mutex_unlock(&q_lock);
    return 0;
}

static long long q_recv(void) {
    node *n;
    long long v;
    pthread_mutex_lock(&q_lock);
    while (!q_head) pthread_cond_wait(&q_ready, &q_lock);
    n = q_head;
    q_head = n->next;
    if (!q_head) q_tail = 0;
    pthread_mutex_unlock(&q_lock);
    v = n->value;
    free(n);
    return v;
}

static long long round_trip(long long n) {
    long long i, count;
    for (i = 0; i < n; i++) {
        if (q_send(i) != 0) return -1;
    }
    count = 0;
    while (count < n) {
        if (q_recv() < 0) return -2;
        count++;
    }
    return count;
}

int main(void) {
    long long result = 0;
    long long r;
    for (r = 0; r < 200; r++) {
        result = round_trip(500);
    }
    return (int)(result & 0xff);
}
