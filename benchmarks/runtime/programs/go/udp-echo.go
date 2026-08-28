// The udp-echo workload's Go peer (ADR-0017): the identical sequence the
// tuonelang program performs, using Go's own net.ListenPacket /
// WriteTo / ReadFrom — the parity target for datagram I/O. Per round, bind
// two ephemeral loopback UDP sockets, then for each of 8 datagrams send 16
// bytes, read them back on the server, echo a reply to the sender's address,
// and receive it on the client. Same counts, same exit byte (128).
package main

import (
	"net"
	"os"
)

func bindEphemeral() (net.PacketConn, error) {
	return net.ListenPacket("udp4", "127.0.0.1:0")
}

func roundOnce(datagrams int) int {
	server, err := bindEphemeral()
	if err != nil {
		return -1
	}
	defer server.Close()
	client, err := bindEphemeral()
	if err != nil {
		return -3
	}
	defer client.Close()

	payload := []byte("0123456789abcdef")
	buf := make([]byte, 2048)
	count := 0
	for i := 0; i < datagrams; i++ {
		if n, err := client.WriteTo(payload, server.LocalAddr()); err != nil || n != 16 {
			return -4
		}
		n, from, err := server.ReadFrom(buf)
		if err != nil || n != 16 {
			return -5
		}
		for b := 0; b < 16; b++ {
			if buf[b] == 0 {
				return -6
			}
			count++
		}
		if n, err := server.WriteTo([]byte("ok"), from); err != nil || n != 2 {
			return -8
		}
		if n, _, err := client.ReadFrom(buf); err != nil || n != 2 {
			return -9
		}
	}
	return count
}

func main() {
	result := 0
	for r := 0; r < 100; r++ {
		result = roundOnce(8)
	}
	os.Exit(result & 0xff)
}
