// startup — the equivalent-semantics Go peer for the tuonelang `startup`
// workload. A trivial program that starts and exits 0. The Go runtime's
// scheduler/GC startup is part of what is measured — that is exactly the point
// of an AOT-native peer that ships a runtime: the observable is the same (exit
// 0), the startup cost is the language's own.
package main

import "os"

func main() { os.Exit(0) }
