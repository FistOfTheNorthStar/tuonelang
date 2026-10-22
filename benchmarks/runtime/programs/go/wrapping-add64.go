// wrapping-add64 — the equivalent-semantics Go peer for the tuonelang
// wrapping-add64 workload (ADR-0031). The identical loop: 50,000,000 rounds
// of a modular 64-bit add and a rotate right by 29 on uint64, through the
// standard math/bits.RotateLeft64 (a negative count rotates right), starting
// from the same seeds; the exit byte is the low byte of x ^ y = 231.
package main

import (
	"math/bits"
	"os"
)

func main() {
	var x uint64 = 1
	var y uint64 = 11400714819323198485
	for i := 0; i < 50000000; i++ {
		x = x + y
		y = bits.RotateLeft64(y^x, -29)
	}
	os.Exit(int((x ^ y) & 255))
}
