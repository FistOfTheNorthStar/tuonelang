/* The C peer of file-io.tuo: the identical open/write/read/close/unlink
 * sequence — 200 rounds of writing 15 sixteen-byte chunks and reading the
 * 240 bytes back one byte per read(2) call, matching the tuonelang program's
 * byte-at-a-time effect crossings. Exit byte: 240. */
#include <fcntl.h>
#include <unistd.h>

static long long round_trip(long long chunks) {
    const char *path = "file-io-bench.tmp";
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        return -1;
    }
    for (long long i = 0; i < chunks; i++) {
        write(fd, "0123456789abcdef", 16);
    }
    close(fd);
    int rfd = open(path, O_RDONLY);
    if (rfd < 0) {
        return -2;
    }
    long long count = 0;
    unsigned char byte;
    while (read(rfd, &byte, 1) == 1) {
        count++;
    }
    close(rfd);
    unlink(path);
    return count;
}

int main(void) {
    long long result = 0;
    for (long long r = 0; r < 200; r++) {
        result = round_trip(15);
    }
    return (int)(result & 0xff);
}
