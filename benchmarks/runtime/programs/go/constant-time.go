// constant-time — the equivalent-semantics Go peer for the tuonelang
// constant-time workload (ADR-0020), and the parity target it answers.
//
// Go's answer to this problem is `crypto/subtle.ConstantTimeCompare`, so that
// is what this peer uses — the same convention every other workload here
// follows, where the Go side is the standard library's own solution rather
// than a hand-transliteration of the tuonelang source (sha256-hash uses
// crypto/sha256, json-parse uses encoding/json). The comparison being drawn is
// "what does this cost in tuonelang versus what it costs in Go", and for a
// language with a standard-library answer that means using it.
//
// The naive early-returning comparison is written out, because Go's standard
// library deliberately does not provide the vulnerable form.
//
// Same 32-byte tags, same alternating best-case/worst-case inputs, same 1000
// rounds, same exit byte 32.
package main

import (
	"crypto/subtle"
	"os"
)

// The early-returning comparison: the vulnerability, for comparison.
func naiveBytesEq(a, b []byte) int {
	for i := range a {
		if a[i] != b[i] {
			return 0
		}
	}
	return 1
}

func makeTag(seed int) []byte {
	out := make([]byte, 32)
	for i := range out {
		out[i] = byte((seed + i*7) & 255)
	}
	return out
}

func main() {
	reference := makeTag(11)
	earlyMismatch := makeTag(12)
	equal := makeTag(11)

	agreements := 0
	for round := 0; round < 500000; round++ {
		var sameCT, sameNaive int
		if round&1 == 0 {
			sameCT = subtle.ConstantTimeCompare(reference, earlyMismatch)
			sameNaive = naiveBytesEq(reference, earlyMismatch)
		} else {
			sameCT = subtle.ConstantTimeCompare(reference, equal)
			sameNaive = naiveBytesEq(reference, equal)
		}
		if sameCT == sameNaive {
			agreements++
		}
	}
	os.Exit(agreements % 256)
}
