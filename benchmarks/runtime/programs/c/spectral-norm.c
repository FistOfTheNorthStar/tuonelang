/* spectral-norm — the equivalent-semantics C peer for the tuonelang
 * spectral-norm workload (the Benchmarks Game's spectral-norm): the spectral
 * norm of the infinite matrix A(i,j) = 1 / ((i+j)(i+j+1)/2 + i + 1),
 * approximated by ten rounds of the power method on an N-element vector. The
 * identical computation: the same row-major sums in the same order, the same
 * integer index arithmetic, -ffp-contract=off so no multiply-add is fused.
 * The exit byte folds the norm: (long long)(norm * 1e9) % 256. */
#include <math.h>
#include <stdlib.h>

#define N 2000

static double a(long long i, long long j) {
    return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1);
}

static void mul_av(const double *v, double *out) {
    for (long long i = 0; i < N; i++) {
        double sum = 0.0;
        for (long long j = 0; j < N; j++) sum = sum + a(i, j) * v[j];
        out[i] = sum;
    }
}

static void mul_atv(const double *v, double *out) {
    for (long long i = 0; i < N; i++) {
        double sum = 0.0;
        for (long long j = 0; j < N; j++) sum = sum + a(j, i) * v[j];
        out[i] = sum;
    }
}

static void mul_atav(const double *v, double *out, double *tmp) {
    mul_av(v, tmp);
    mul_atv(tmp, out);
}

int main(void) {
    double *u = malloc(N * sizeof(double));
    double *v = malloc(N * sizeof(double));
    double *tmp = malloc(N * sizeof(double));
    for (int i = 0; i < N; i++) { u[i] = 1.0; v[i] = 0.0; tmp[i] = 0.0; }
    for (int round = 0; round < 10; round++) {
        mul_atav(u, v, tmp);
        mul_atav(v, u, tmp);
    }
    double vbv = 0.0, vv = 0.0;
    for (int i = 0; i < N; i++) {
        vbv = vbv + u[i] * v[i];
        vv = vv + v[i] * v[i];
    }
    double norm = sqrt(vbv / vv);
    free(u); free(v); free(tmp);
    return (int)((long long)(norm * 1e9) % 256);
}
