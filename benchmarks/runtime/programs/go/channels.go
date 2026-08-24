// channels — the equivalent-semantics Go peer for the tuonelang channels
// workload (ADR-0015), and the direct Go-parity comparison the ADR invites:
// where tuonelang crosses its runtime's locked FIFO and C a hand-rolled
// mutex-and-condvar queue, Go uses its **native channel** (buffered to the
// round's size, since the protocol sends all 500 before receiving and an
// unbuffered channel would block a single goroutine). Per round, send 500
// values and receive all 500 back. The observable result is one round's
// receive count (reassigned, not accumulated): 500, exit byte
// 500 & 0xff = 244.
package main

import "os"

func roundTrip(ch chan int64, n int64) int64 {
	for i := int64(0); i < n; i++ {
		ch <- i
	}
	var count int64 = 0
	for count < n {
		if <-ch < 0 {
			return -2
		}
		count++
	}
	return count
}

func main() {
	ch := make(chan int64, 500)
	var result int64 = 0
	for r := int64(0); r < 200; r++ {
		result = roundTrip(ch, 500)
	}
	os.Exit(int(result & 0xff))
}
