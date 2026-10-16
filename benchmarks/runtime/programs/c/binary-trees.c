/* binary-trees — the equivalent-semantics C peer for the tuonelang
 * binary-trees workload (the Benchmarks Game's binary-trees): a stretch tree
 * of depth MAX + 1, a long-lived tree of depth MAX, and 2^(MAX - d + 4) trees
 * of each depth d = 4, 6, ..., MAX. Every node is its own malloc; a walk counts
 * a tree's nodes and frees each one as it is counted, exactly as the tuonelang
 * walk consumes its boxes. Exit byte: the summed node counts % 256. */
#include <stdlib.h>

#define MIN_DEPTH 4
#define MAX_DEPTH 16

typedef struct Node {
    struct Node *left;
    struct Node *right;
} Node;

static Node *make(long long depth) {
    Node *node = malloc(sizeof(Node));
    if (depth == 0) {
        node->left = NULL;
        node->right = NULL;
    } else {
        node->left = make(depth - 1);
        node->right = make(depth - 1);
    }
    return node;
}

static long long check(Node *node) {
    long long left = node->left ? check(node->left) : 0;
    long long right = node->right ? check(node->right) : 0;
    free(node);
    return 1 + left + right;
}

int main(void) {
    long long total = check(make(MAX_DEPTH + 1));
    Node *long_lived = make(MAX_DEPTH);
    for (long long depth = MIN_DEPTH; depth <= MAX_DEPTH; depth += 2) {
        long long iterations = 1LL << (MAX_DEPTH - depth + MIN_DEPTH);
        for (long long i = 0; i < iterations; i++) total += check(make(depth));
    }
    total += check(long_lived);
    return (int)(total % 256);
}
