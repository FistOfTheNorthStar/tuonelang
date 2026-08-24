/* networking — the equivalent-semantics C peer for the tuonelang networking
 * workload (ADR-0014's socket effects). The identical sequence: per round,
 * create an IPv4 TCP socket listening on an ephemeral loopback port, learn
 * the port with getsockname, connect to it (the backlog completes the
 * handshake before accept), accept, write 8 sixteen-byte chunks (128 bytes)
 * from the client, read them back on the server **one byte per read(2)
 * call** (mirroring the tuonelang program's byte-at-a-time read_byte
 * crossings), and close all three descriptors. The observable result is one
 * round's byte count (reassigned, not accumulated): 128, the exit byte. */
#include <arpa/inet.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static long long round_trip(long long chunks) {
    struct sockaddr_in addr;
    socklen_t alen = sizeof(addr);
    int one = 1;
    int listener, client, server;
    long long i, count, want;
    listener = socket(AF_INET, SOCK_STREAM, 0);
    if (listener < 0) return -1;
    setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(0);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(listener, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||
        listen(listener, 16) != 0 ||
        getsockname(listener, (struct sockaddr *)&addr, &alen) != 0) {
        close(listener);
        return -2;
    }
    client = socket(AF_INET, SOCK_STREAM, 0);
    if (client < 0) {
        close(listener);
        return -3;
    }
    if (connect(client, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        close(client);
        close(listener);
        return -3;
    }
    server = accept(listener, 0, 0);
    if (server < 0) {
        close(client);
        close(listener);
        return -4;
    }
    for (i = 0; i < chunks; i++) {
        write(client, "0123456789abcdef", 16);
    }
    count = 0;
    want = chunks * 16;
    while (count < want) {
        unsigned char b;
        if (read(server, &b, 1) != 1) return -5;
        count++;
    }
    close(client);
    close(server);
    close(listener);
    return count;
}

int main(void) {
    long long result = 0;
    long long r;
    for (r = 0; r < 100; r++) {
        result = round_trip(8);
    }
    return (int)(result & 0xff);
}
