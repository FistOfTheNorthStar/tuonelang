// function-calls — the equivalent-semantics Go peer. Same non-recursive call
// tree as the C peer and the tuonelang program: g(3)+g(4)+g(5) where
// g(n) = f(n)+f(n) and f(n) = n+1, giving 30.
package main

import "os"

func f(n int32) int32 { return n + 1 }
func g(n int32) int32 { return f(n) + f(n) }

func main() { os.Exit(int(g(3) + g(4) + g(5))) }
