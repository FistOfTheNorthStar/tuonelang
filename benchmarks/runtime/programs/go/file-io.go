// file-io — the equivalent-semantics Go peer for the tuonelang file-io
// workload (ADR-0013's OS effect boundary). The identical sequence: per
// round, open/create/truncate a scratch file, write 15 sixteen-byte chunks
// (240 bytes), close it, reopen it for read, read it back **one byte per
// Read call** (mirroring the tuonelang program's byte-at-a-time read_byte
// crossings and the C peer's one-byte read(2) loop), close it, and remove
// it. The observable result is one round's byte count (reassigned, not
// accumulated): 240, the exit byte.
package main

import "os"

func roundTrip(chunks int64) int64 {
	path := "file-io-bench.tmp"
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0644)
	if err != nil {
		return -1
	}
	chunk := []byte("0123456789abcdef")
	for i := int64(0); i < chunks; i++ {
		f.Write(chunk)
	}
	f.Close()
	r, err := os.Open(path)
	if err != nil {
		return -2
	}
	var count int64 = 0
	b := make([]byte, 1)
	for {
		n, _ := r.Read(b)
		if n != 1 {
			break
		}
		count++
	}
	r.Close()
	os.Remove(path)
	return count
}

func main() {
	var result int64 = 0
	for r := int64(0); r < 200; r++ {
		result = roundTrip(15)
	}
	os.Exit(int(result & 0xff))
}
