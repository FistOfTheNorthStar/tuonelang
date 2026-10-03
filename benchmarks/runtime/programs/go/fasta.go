// fasta — the equivalent-semantics Go peer for the tuonelang fasta workload
// (the Benchmarks Game's fasta): the same three generated sequences, the same
// linear congruential generator, 60 bases per line, every byte folded into
// sum = (sum * 31 + byte) % 1000003. Exit byte: sum % 256.
package main

import "os"

const n = 1000000
const line = 60
const alu = "GGCCGGGCGCGGTGGCTCACGCCTGTAATCCCAGCACTTTGGGAGGCCGAGGCGGGCGGATCACCTGAGGTCAGGAGTTCGAGACCAGCCTGGCCAACATGGTGAAACCCCGTCTCTACTAAAAATACAAAAATTAGCCGGGCGTGGTGGCGCGCGCCTGTAATCCCAGCTACTCGGGAGGCTGAGGCAGGAGAATCGCTTGAACCCGGGAGGCGGAGGTTGCAGTGAGCCGAGATCGCGCCACTGCACTCCAGCCTGGGCGACAGAGCGAGACTCCGTCTCAAAAA"

var sum int64
var last int64 = 42

func emit(b int64) { sum = (sum*31 + b) % 1000003 }

func randomUnit() float64 {
	last = (last*3877 + 29573) % 139968
	return float64(last) / 139968.0
}

func repeat(count int) {
	at, col := 0, 0
	for i := 0; i < count; i++ {
		emit(int64(alu[at]))
		at++
		if at == len(alu) {
			at = 0
		}
		col++
		if col == line {
			emit(10)
			col = 0
		}
	}
	if col > 0 {
		emit(10)
	}
}

func randomSeq(codes string, cumulative []float64, count int) {
	col := 0
	for i := 0; i < count; i++ {
		r := randomUnit()
		k := 0
		for k < len(cumulative)-1 && r >= cumulative[k] {
			k++
		}
		emit(int64(codes[k]))
		col++
		if col == line {
			emit(10)
			col = 0
		}
	}
	if col > 0 {
		emit(10)
	}
}

func cumulate(p []float64) []float64 {
	out := make([]float64, len(p))
	acc := 0.0
	for i, x := range p {
		acc = acc + x
		out[i] = acc
	}
	return out
}

func main() {
	iub := cumulate([]float64{0.27, 0.12, 0.12, 0.27, 0.02, 0.02, 0.02, 0.02,
		0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02})
	homo := cumulate([]float64{0.3029549426680, 0.1979883004921, 0.1975473066391, 0.3015094502008})
	repeat(2 * n)
	randomSeq("acgtBDHKMNRSVWY", iub, 3*n)
	randomSeq("acgt", homo, 5*n)
	os.Exit(int(sum % 256))
}
