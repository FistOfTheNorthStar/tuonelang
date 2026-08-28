/* The connect-timeout workload's C peer (ADR-0017): the identical sequence
   the tuonelang program performs, using the standard bounded-connect idiom —
   non-blocking connect + poll(POLLOUT) with the same deadline, consulting
   SO_ERROR. Same round count, same bounded-outcome accounting, same exit
   byte (200). */
#include <errno.h>
#include <fcntl.h>
#include <arpa/inet.h>
#include <netinet/in.h>
#include <poll.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int dead_port(void) {
    struct sockaddr_in addr;
    socklen_t alen = sizeof(addr);
    int port, fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = 0;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||
        listen(fd, 16) != 0 ||
        getsockname(fd, (struct sockaddr *)&addr, &alen) != 0) {
        close(fd);
        return -1;
    }
    port = (int)ntohs(addr.sin_port);
    close(fd);
    return port;
}

/* Returns 1 when the attempt came back bounded (refused or timed out). */
static int round_once(int port, int ms) {
    struct sockaddr_in addr;
    struct pollfd pfd;
    int fd, flags, err = 0;
    socklen_t errlen = sizeof(err);
    fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return 0;
    flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {
        close(fd);
        return 0;
    }
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((unsigned short)port);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0) {
        close(fd); /* someone accepted: not the bounded-failure path */
        return 0;
    }
    if (errno != EINPROGRESS) {
        close(fd); /* refused immediately — a bounded outcome */
        return 1;
    }
    pfd.fd = fd;
    pfd.events = POLLOUT;
    pfd.revents = 0;
    if (poll(&pfd, 1, ms) == 0) {
        close(fd); /* timed out — a bounded outcome */
        return 1;
    }
    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &errlen) != 0 || err != 0) {
        close(fd);
        return 1;
    }
    close(fd);
    return 0;
}

int main(void) {
    int port = dead_port(), count = 0, r;
    if (port <= 0) return 1;
    for (r = 0; r < 200; r++) count += round_once(port, 50);
    return count & 0xff;
}
