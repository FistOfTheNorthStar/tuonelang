/* sha256-hash — the equivalent-semantics C peer for the tuonelang sha256-hash
 * workload (ADR-0019). The identical computation: the same fixed 64-byte
 * message, the same FIPS 180-4 compression function over uint32_t with the
 * same rotations, shifts, and modular additions, across the same two padded
 * blocks. 200 rounds; the exit byte is the digest's first byte (0x96 = 150),
 * the last round's value (reassigned, not accumulated). */
#include <stdint.h>
#include <string.h>

static const uint32_t K[64] = {
    0x428a2f98u, 0x71374491u, 0xb5c0fbcfu, 0xe9b5dba5u, 0x3956c25bu, 0x59f111f1u,
    0x923f82a4u, 0xab1c5ed5u, 0xd807aa98u, 0x12835b01u, 0x243185beu, 0x550c7dc3u,
    0x72be5d74u, 0x80deb1feu, 0x9bdc06a7u, 0xc19bf174u, 0xe49b69c1u, 0xefbe4786u,
    0x0fc19dc6u, 0x240ca1ccu, 0x2de92c6fu, 0x4a7484aau, 0x5cb0a9dcu, 0x76f988dau,
    0x983e5152u, 0xa831c66du, 0xb00327c8u, 0xbf597fc7u, 0xc6e00bf3u, 0xd5a79147u,
    0x06ca6351u, 0x14292967u, 0x27b70a85u, 0x2e1b2138u, 0x4d2c6dfcu, 0x53380d13u,
    0x650a7354u, 0x766a0abbu, 0x81c2c92eu, 0x92722c85u, 0xa2bfe8a1u, 0xa81a664bu,
    0xc24b8b70u, 0xc76c51a3u, 0xd192e819u, 0xd6990624u, 0xf40e3585u, 0x106aa070u,
    0x19a4c116u, 0x1e376c08u, 0x2748774cu, 0x34b0bcb5u, 0x391c0cb3u, 0x4ed8aa4au,
    0x5b9cca4fu, 0x682e6ff3u, 0x748f82eeu, 0x78a5636fu, 0x84c87814u, 0x8cc70208u,
    0x90befffau, 0xa4506cebu, 0xbef9a3f7u, 0xc67178f2u};

static uint32_t rotr32(uint32_t x, unsigned n) {
    return (x >> n) | (x << (32 - n));
}

static unsigned first_digest_byte(const unsigned char *msg, int len) {
    uint32_t h[8] = {0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
                     0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u};

    for (int base = 0; base < len; base += 64) {
        uint32_t w[64];
        for (int t = 0; t < 16; t++) {
            int at = base + t * 4;
            w[t] = ((uint32_t)msg[at] << 24) | ((uint32_t)msg[at + 1] << 16) |
                   ((uint32_t)msg[at + 2] << 8) | (uint32_t)msg[at + 3];
        }
        for (int t = 16; t < 64; t++) {
            uint32_t s0 = rotr32(w[t - 15], 7) ^ rotr32(w[t - 15], 18) ^ (w[t - 15] >> 3);
            uint32_t s1 = rotr32(w[t - 2], 17) ^ rotr32(w[t - 2], 19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16] + s0 + w[t - 7] + s1;
        }

        uint32_t a = h[0], b = h[1], c = h[2], d = h[3];
        uint32_t e = h[4], f = h[5], g = h[6], hh = h[7];
        for (int r = 0; r < 64; r++) {
            uint32_t big1 = rotr32(e, 6) ^ rotr32(e, 11) ^ rotr32(e, 25);
            uint32_t ch = (e & f) ^ ((~e) & g);
            uint32_t t1 = hh + big1 + ch + K[r] + w[r];
            uint32_t big0 = rotr32(a, 2) ^ rotr32(a, 13) ^ rotr32(a, 22);
            uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
            uint32_t t2 = big0 + maj;
            hh = g; g = f; f = e; e = d + t1;
            d = c; c = b; b = a; a = t1 + t2;
        }
        h[0] += a; h[1] += b; h[2] += c; h[3] += d;
        h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
    }
    return (unsigned)((h[0] >> 24) & 255u);
}

int main(void) {
    /* The same 64-byte message, padded to two blocks: 0x80, zeros, then the
     * 64-bit big-endian bit length (512 = 0x200). */
    unsigned char msg[128];
    memset(msg, 0, sizeof msg);
    for (int i = 0; i < 64; i++) msg[i] = (unsigned char)(48 + (i % 10));
    msg[64] = 0x80;
    msg[126] = 0x02;

    unsigned first = 0;
    for (int round = 0; round < 200; round++) {
        first = first_digest_byte(msg, 128);
    }
    return (int)first;
}
