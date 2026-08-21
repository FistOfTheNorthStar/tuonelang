// indirect-calls — the equivalent-semantics Go peer for the tuonelang
// `indirect-calls` workload (ADR-0008 Tier 1). It runs the identical loop,
// calling through a **function value** (Go's first-class func, the closest peer
// to tuonelang's non-capturing function value and to C's function pointer): the
// same arithmetic (`acc = step(acc, 1)`), the same 2,000,000 iterations, and the
// same observable exit (2_000_000 & 0xff = 128). The indirect-call sibling of
// function-calls.go, which calls f/g directly.
package main

import "os"

func add(a, b int32) int32 { return a + b }

func fold(reps int32, step func(int32, int32) int32) int32 {
	var acc int32 = 0
	for i := int32(0); i < reps; i++ {
		acc = step(acc, 1)
	}
	return acc
}

func main() {
	// 2_000_000 indirect calls through the `add` function value; acc = 2_000_000,
	// exit byte 2_000_000 & 0xff = 128.
	os.Exit(int(fold(2000000, add)))
}
