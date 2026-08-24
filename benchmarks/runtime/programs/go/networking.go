// networking — the equivalent-semantics Go peer for the tuonelang networking
// workload (ADR-0014's socket effects). The identical sequence: per round,
// listen on an ephemeral loopback port, dial it (the backlog completes the
// handshake before Accept), accept, write 8 sixteen-byte chunks (128 bytes)
// from the client, read them back on the server **one byte per Read call**
// (mirroring the tuonelang program's byte-at-a-time read_byte crossings and
// the C peer's one-byte read(2) loop), and close all three. The observable
// result is one round's byte count (reassigned, not accumulated): 128, the
// exit byte.
package main

import (
	"net"
	"os"
)

func roundTrip(chunks int64) int64 {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return -1
	}
	client, err := net.Dial("tcp", listener.Addr().String())
	if err != nil {
		listener.Close()
		return -3
	}
	server, err := listener.Accept()
	if err != nil {
		client.Close()
		listener.Close()
		return -4
	}
	chunk := []byte("0123456789abcdef")
	for i := int64(0); i < chunks; i++ {
		client.Write(chunk)
	}
	var count int64 = 0
	want := chunks * 16
	b := make([]byte, 1)
	for count < want {
		n, _ := server.Read(b)
		if n != 1 {
			return -5
		}
		count++
	}
	client.Close()
	server.Close()
	listener.Close()
	return count
}

func main() {
	var result int64 = 0
	for r := int64(0); r < 100; r++ {
		result = roundTrip(8)
	}
	os.Exit(int(result & 0xff))
}
