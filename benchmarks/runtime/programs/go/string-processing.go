// string-processing — the equivalent-semantics Go peer for the tuonelang
// `string-processing` workload (ADR-0006's borrowed `Str`). Byte-level scanning
// over a fixed request-log line, done the same way: count spaces (32) and
// slashes (47), count ASCII digits, and compare two fixed slices against "GET"
// and "HTTP/1.1". Go's []byte view of a string is the peer to tuonelang's byte
// slicing over `Str`. Per round: 4 spaces + 3 slashes + 11 digits + 1 + 1 = 20;
// 200 rounds × 20 = 4000; observable exit byte = 4000 & 0xff = 160.
package main

import "os"

var lineText = []byte("GET /users/42 HTTP/1.1 200 1532")

func countByte(text []byte, target byte) int {
	found := 0
	for i := 0; i < len(text); i++ {
		if text[i] == target {
			found++
		}
	}
	return found
}

func countDigits(text []byte) int {
	found := 0
	for i := 0; i < len(text); i++ {
		b := text[i]
		if b >= 48 && b <= 57 {
			found++
		}
	}
	return found
}

func sliceEquals(text []byte, start, end int, expect string) int {
	if end-start != len(expect) {
		return 0
	}
	for i := start; i < end; i++ {
		if text[i] != expect[i-start] {
			return 0
		}
	}
	return 1
}

func score(text []byte) int {
	return countByte(text, 32) + countByte(text, 47) +
		countDigits(text) + sliceEquals(text, 0, 3, "GET") +
		sliceEquals(text, 14, 22, "HTTP/1.1")
}

func main() {
	total := 0
	for round := 0; round < 200; round++ {
		total += score(lineText)
	}
	os.Exit(total)
}
