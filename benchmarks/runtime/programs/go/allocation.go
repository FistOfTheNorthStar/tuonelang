// allocation — the equivalent-semantics Go peer for the tuonelang allocation
// workload (ADR-0009's owned `String` and growable `Array[Int]`). Same
// computation the same way: a growable int64 vector filled 0..count by push
// with **explicit doubling growth** (mirroring the tuonelang program's and the C
// peer's realloc doubling, rather than Go's built-in append heuristic, so the
// three grow identically), summed; and a growable byte buffer filled to `count`
// bytes the same way, measured by length. Go's GC reclaims each round's buffers
// (the peer to tuonelang's drop-glue free and C's explicit free); the round is
// repeated so the workload really exercises heap allocation/growth throughput.
// The observable result is one round's contribution (reassigned, not
// accumulated): for count = 16 the vector sums to 120 and the buffer is 16
// bytes, so main returns 136.
package main

import "os"

func arraySum(count int) int64 {
	var xs []int64
	var length int64 = 0
	var cap64 int64 = 0
	for i := 0; i < count; i++ {
		if length == cap64 {
			if cap64 == 0 {
				cap64 = 1
			} else {
				cap64 *= 2
			}
			grown := make([]int64, cap64)
			copy(grown, xs[:length])
			xs = grown
		}
		xs[length] = int64(i)
		length++
	}
	var total int64 = 0
	for j := int64(0); j < length; j++ {
		total += xs[j]
	}
	return total
}

func stringLen(count int) int64 {
	var s []byte
	var length int64 = 0
	var cap64 int64 = 0
	for i := 0; i < count; i++ {
		if length == cap64 {
			if cap64 == 0 {
				cap64 = 1
			} else {
				cap64 *= 2
			}
			grown := make([]byte, cap64)
			copy(grown, s[:length])
			s = grown
		}
		s[length] = 65
		length++
	}
	return length
}

func roundOnce(count int) int64 {
	return arraySum(count) + stringLen(count)
}

func main() {
	var result int64 = 0
	for r := 0; r < 2000; r++ {
		result = roundOnce(16)
	}
	os.Exit(int(result))
}
