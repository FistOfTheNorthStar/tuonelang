// wire-decode — the equivalent-semantics Go peer for the tuonelang wire-decode
// workload, and the parity target ADR-0019 answers: Go's standard
// `encoding/binary` decodes and re-encodes the same big-endian fields with
// BigEndian.Uint32/PutUint32 over the same 256-byte buffer of 16
// length-prefixed frames, folding the identical checksum = 120, the exit byte
// (the last round's value, reassigned, not accumulated).
package main

import (
	"encoding/binary"
	"os"
)

const bufLen = 256

func fill() []byte {
	buf := make([]byte, 0, bufLen)
	for frame := 0; frame < 16; frame++ {
		buf = append(buf, 0, 0, 0, 16, 0, byte(64+frame))
		for i := 0; i < 10; i++ {
			buf = append(buf, byte((frame*7+i)&255))
		}
	}
	return buf
}

func decode(buf []byte) int {
	checksum := 0
	pos := 0
	var scratch [4]byte
	for pos+6 <= bufLen {
		length := binary.BigEndian.Uint32(buf[pos : pos+4])
		kind := binary.BigEndian.Uint16(buf[pos+4 : pos+6])

		// The round-trip: re-encoding the length must reproduce the bytes.
		binary.BigEndian.PutUint32(scratch[:], length)
		rebuilt := 0
		for j := 0; j < 4; j++ {
			if scratch[j] == buf[pos+j] {
				rebuilt++
			}
		}
		checksum = (checksum + rebuilt) & 255

		checksum = (checksum + int(length&255) + int(kind&255)) & 255
		for p := pos + 6; p < pos+int(length) && p < bufLen; p++ {
			checksum = (checksum ^ int(buf[p])) & 255
		}
		if length == 0 {
			return checksum
		}
		pos += int(length)
	}
	return checksum
}

func main() {
	buf := fill()
	checksum := 0
	for round := 0; round < 200; round++ {
		checksum = decode(buf)
	}
	os.Exit(checksum)
}
