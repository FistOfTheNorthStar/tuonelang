// collections — the equivalent-semantics Go peer for the tuonelang
// `collections` workload (ADR-0004 Stage 2's fixed-size array `[Int; 8]`). The
// closest Go peer to a fixed inline array is a value-type `[8]int64` (not a
// heap slice): the same bulk scan and indexed probes, the same 200 rounds. Each
// round scans (3+1+4+1+5+9+2+6 = 31) and probes indices 0,3,7 (3+1+6 = 10), so
// 200 × 41 = 8200; observable exit byte = 8200 & 0xff = 8.
package main

import "os"

func lookup(table [8]int64, i int64) int64 { return table[i] }

func scan(table [8]int64) int64 {
	var total int64 = 0
	for k := 0; k < 8; k++ {
		total += table[k]
	}
	return total
}

func probe(table [8]int64) int64 {
	return lookup(table, 0) + lookup(table, 3) + lookup(table, 7)
}

func main() {
	table := [8]int64{3, 1, 4, 1, 5, 9, 2, 6}
	var total int64 = 0
	for round := 0; round < 200; round++ {
		total += scan(table) + probe(table)
	}
	os.Exit(int(total))
}
