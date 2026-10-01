/* mandelbrot — the equivalent-semantics C peer for the tuonelang mandelbrot
 * workload (the Benchmarks Game's mandelbrot): the N x N bitmap of the
 * Mandelbrot set over [-1.5, 0.5] x [-1, 1], 50 iterations per point, packed
 * eight pixels to a byte. Instead of writing the PBM to stdout, every packed
 * byte is folded into a checksum (sum = (sum * 31 + byte) % 1000003) so the
 * exit byte witnesses the whole image: sum % 256. -ffp-contract=off keeps
 * 2*Zr*Zi + Ci unfused, as in the other peers. */
#define N 1600

int main(void) {
    long long sum = 0;
    for (long long y = 0; y < N; y++) {
        double ci = 2.0 * (double)y / (double)N - 1.0;
        long long bits = 0, count = 0;
        for (long long x = 0; x < N; x++) {
            double cr = 2.0 * (double)x / (double)N - 1.5;
            double zr = 0.0, zi = 0.0, tr = 0.0, ti = 0.0;
            long long i = 0;
            while (i < 50 && tr + ti <= 4.0) {
                zi = 2.0 * zr * zi + ci;
                zr = tr - ti + cr;
                tr = zr * zr;
                ti = zi * zi;
                i = i + 1;
            }
            bits = bits * 2 + (tr + ti <= 4.0 ? 1 : 0);
            count = count + 1;
            if (count == 8) {
                sum = (sum * 31 + bits) % 1000003;
                bits = 0;
                count = 0;
            }
        }
        if (count > 0) {
            while (count < 8) { bits = bits * 2; count = count + 1; }
            sum = (sum * 31 + bits) % 1000003;
        }
    }
    return (int)(sum % 256);
}
