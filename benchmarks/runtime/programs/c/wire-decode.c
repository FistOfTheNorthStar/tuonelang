/* wire-decode — the equivalent-semantics C peer for the tuonelang wire-decode
 * workload (ADR-0019). The identical walk: the same 256-byte buffer of 16
 * length-prefixed frames, per frame a big-endian 4-byte length and 2-byte
 * type decoded with the same shifts and masks, the length re-encoded and
 * checked byte-for-byte, and the payload folded into the same checksum.
 * 200 rounds; the exit byte is the last round's checksum (reassigned, not
 * accumulated) = 120. */
#include <stdint.h>

#define BUF_LEN 256

static void fill(unsigned char *buf) {
    int at = 0;
    for (int frame = 0; frame < 16; frame++) {
        buf[at++] = 0;
        buf[at++] = 0;
        buf[at++] = 0;
        buf[at++] = 16;
        buf[at++] = 0;
        buf[at++] = (unsigned char)(64 + frame);
        for (int i = 0; i < 10; i++) {
            buf[at++] = (unsigned char)((frame * 7 + i) & 255);
        }
    }
}

static long long decode(const unsigned char *buf) {
    long long checksum = 0;
    int pos = 0;
    while (pos + 6 <= BUF_LEN) {
        uint32_t length = ((uint32_t)buf[pos] << 24) | ((uint32_t)buf[pos + 1] << 16) |
                          ((uint32_t)buf[pos + 2] << 8) | (uint32_t)buf[pos + 3];
        uint32_t kind = ((uint32_t)buf[pos + 4] << 8) | (uint32_t)buf[pos + 5];

        /* The round-trip: re-encoding the length must reproduce the bytes. */
        long long rebuilt = 0;
        for (int j = 0; j < 4; j++) {
            if ((int)((length >> ((3 - j) * 8)) & 255) == buf[pos + j]) rebuilt++;
        }
        checksum = (checksum + rebuilt) & 255;

        checksum = (checksum + (long long)(length & 255) + (long long)(kind & 255)) & 255;
        for (int p = pos + 6; p < pos + (int)length && p < BUF_LEN; p++) {
            checksum = (checksum ^ buf[p]) & 255;
        }
        if (length == 0) return checksum;
        pos += (int)length;
    }
    return checksum;
}

int main(void) {
    unsigned char buf[BUF_LEN];
    fill(buf);
    long long checksum = 0;
    for (int round = 0; round < 200; round++) {
        checksum = decode(buf);
    }
    return (int)checksum;
}
