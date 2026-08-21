// recursion — the equivalent-semantics Go peer. Same naive tree recursion as
// the C peer and the tuonelang program: fib(20) = 6765; observable exit byte =
// 6765 & 0xff = 109.
package main

import "os"

func fib(n int32) int32 {
	if n < 2 {
		return n
	}
	return fib(n-1) + fib(n-2)
}

func main() { os.Exit(int(fib(20))) }
