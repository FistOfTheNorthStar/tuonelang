// The connect-timeout workload's Go peer (ADR-0017): the identical sequence
// the tuonelang program performs, using Go's own net.DialTimeout — the
// parity target for a bounded connect. Same round count, same
// bounded-outcome accounting, same exit byte (200).
package main

import (
	"net"
	"os"
	"strconv"
	"time"
)

func deadPort() int {
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		return -1
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	return port
}

// Returns 1 when the attempt came back bounded (refused or timed out).
func roundOnce(port int, d time.Duration) int {
	addr := net.JoinHostPort("127.0.0.1", strconv.Itoa(port))
	conn, err := net.DialTimeout("tcp4", addr, d)
	if err != nil {
		return 1
	}
	conn.Close()
	return 0
}

func main() {
	port := deadPort()
	if port <= 0 {
		os.Exit(1)
	}
	count := 0
	for r := 0; r < 200; r++ {
		count += roundOnce(port, 50*time.Millisecond)
	}
	os.Exit(count & 0xff)
}
