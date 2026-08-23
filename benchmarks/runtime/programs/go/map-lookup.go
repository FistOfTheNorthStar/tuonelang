// map-lookup — the equivalent-semantics Go peer for the tuonelang map-lookup
// workload (ADR-0011's `Map[Int, Int]`). Same computation the same way,
// through Go's built-in map: `count` inserts (key i → 3·i), every key looked
// back up summing values modulo 1000, then the first half deleted. The
// workload never iterates the map, so Go's randomized iteration order is
// irrelevant and the observable result agrees with the tuonelang and C
// peers: one round contributes 3·999·1000/2 % 1000 + 500 = 1000 for
// count = 1000; the round repeats and the result is reassigned, not
// accumulated, so main exits 1000 & 0xff = 232.
package main

import "os"

func round(count int64) int64 {
	m := make(map[int64]int64)
	for i := int64(0); i < count; i++ {
		m[i] = i * 3
	}
	var hits int64
	for j := int64(0); j < count; j++ {
		hits = (hits + m[j]) % 1000
	}
	for k := int64(0); k < count/2; k++ {
		delete(m, k)
	}
	return hits + int64(len(m))
}

func main() {
	var result int64
	for r := 0; r < 50; r++ {
		result = round(1000)
	}
	os.Exit(int(result & 0xff))
}
