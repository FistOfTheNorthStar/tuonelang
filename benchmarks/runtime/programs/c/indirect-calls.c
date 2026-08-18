// indirect-calls — the equivalent-semantics C peer for the tuonelang
// `indirect-calls` workload (ADR-0008 Tier 1). It runs the identical loop,
// calling through a **function pointer** rather than tuonelang's function value:
// the same arithmetic (`acc += add(acc, 1)`-style step), the same 2,000,000
// iterations, and the same observable exit (2_000_000 & 0xff = 128). This is the
// indirect-call sibling of function-calls.c, which calls f/g directly.
static int add(int a, int b) { return a + b; }

static int fold(int reps, int (*step)(int, int)) {
    int acc = 0;
    for (int i = 0; i < reps; i++) {
        acc = step(acc, 1);
    }
    return acc;
}

int main(void) {
    // 2_000_000 indirect calls through the `add` function pointer; acc = 2_000_000,
    // exit byte 2_000_000 & 0xff = 128.
    return fold(2000000, add);
}
