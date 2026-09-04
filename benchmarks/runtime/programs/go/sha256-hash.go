// sha256-hash — the equivalent-semantics Go peer for the tuonelang sha256-hash
// workload, and the parity target ADR-0019 answers: Go's standard
// `crypto/sha256` digests the same fixed 64-byte message, and the exit byte is
// the digest's first byte (0x96 = 150), the last round's value (reassigned,
// not accumulated) over 200 rounds.
package main

import (
	"crypto/sha256"
	"os"
)

func message() []byte {
	m := make([]byte, 64)
	for i := range m {
		m[i] = byte(48 + (i % 10))
	}
	return m
}

func main() {
	m := message()
	first := 0
	for round := 0; round < 200; round++ {
		sum := sha256.Sum256(m)
		first = int(sum[0])
	}
	os.Exit(first)
}
