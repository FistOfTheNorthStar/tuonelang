/* The udp-echo workload's C peer (ADR-0017): the identical sequence the
   tuonelang program performs — per round, bind two ephemeral loopback UDP
   sockets, then for each of 8 datagrams send 16 bytes, recvfrom them on the
   server, read all 16 bytes back out of the staging buffer, echo a reply to
   the sender's port, and receive it on the client. Equivalent semantics:
   same syscalls, same counts, same exit byte (128). */
#include <arpa/inet.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int bind_ephemeral(void) {
    struct sockaddr_in addr;
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) return -1;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = 0;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int port_of(int fd) {
    struct sockaddr_in addr;
    socklen_t alen = sizeof(addr);
    if (getsockname(fd, (struct sockaddr *)&addr, &alen) != 0) return -1;
    return (int)ntohs(addr.sin_port);
}

static int round_once(int datagrams) {
    struct sockaddr_in to;
    unsigned char buf[2048];
    int count = 0, i;
    int server = bind_ephemeral();
    int sport, client;
    if (server < 0) return -1;
    sport = port_of(server);
    if (sport <= 0) return -2;
    client = bind_ephemeral();
    if (client < 0) return -3;
    memset(&to, 0, sizeof(to));
    to.sin_family = AF_INET;
    to.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    for (i = 0; i < datagrams; i++) {
        struct sockaddr_in from;
        socklen_t flen = sizeof(from);
        ssize_t n;
        int b;
        to.sin_port = htons((unsigned short)sport);
        if (sendto(client, "0123456789abcdef", 16, 0,
                   (struct sockaddr *)&to, sizeof(to)) != 16)
            return -4;
        n = recvfrom(server, buf, sizeof(buf), 0,
                     (struct sockaddr *)&from, &flen);
        if (n != 16) return -5;
        /* Read every byte back out of the buffer, mirroring the
           tuonelang program's udp_byte_at loop. */
        for (b = 0; b < 16; b++) {
            if (buf[b] == 0) return -6;
            count++;
        }
        if (sendto(server, "ok", 2, 0, (struct sockaddr *)&from,
                   sizeof(from)) != 2)
            return -8;
        n = recvfrom(client, buf, sizeof(buf), 0, 0, 0);
        if (n != 2) return -9;
    }
    close(client);
    close(server);
    return count;
}

int main(void) {
    int result = 0, r;
    for (r = 0; r < 100; r++) result = round_once(8);
    return result & 0xff;
}
