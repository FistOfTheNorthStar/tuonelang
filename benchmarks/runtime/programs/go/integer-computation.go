// integer-computation — the equivalent-semantics Go peer. Same tail-recursive
// integer reduction as the C peer and the tuonelang program: sum(1..=1000) by
// recursion = 500500; observable exit byte = 500500 & 0xff = 20.
package main

import "os"

func sum(n, acc int32) int32 {
	if n == 0 {
		return acc
	}
	return sum(n-1, acc+n)
}

func main() { os.Exit(int(sum(1000, 0))) }
