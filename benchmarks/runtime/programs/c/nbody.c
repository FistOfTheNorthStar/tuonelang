/* nbody — the equivalent-semantics C peer for the tuonelang nbody workload
 * (the Computer Language Benchmarks Game's n-body): a Jovian-planet orbit
 * simulation with symplectic integration. The identical computation: the
 * same five bodies, the same pairwise update order, the same arithmetic
 * (compiled with -ffp-contract=off so no multiply-add is fused, matching the
 * tuonelang and Rust peers), STEPS steps of dt = 0.01. The exit byte folds the
 * final energy: (long long)(-energy * 1e9) % 256. */
#include <math.h>

#define BODIES 5
#define STEPS 1000000

static const double PI = 3.141592653589793;
static const double DAYS_PER_YEAR = 365.24;

static double x[BODIES], y[BODIES], z[BODIES];
static double vx[BODIES], vy[BODIES], vz[BODIES], mass[BODIES];

static void body(int i, double px, double py, double pz, double qx, double qy,
                 double qz, double m) {
    double solar_mass = 4.0 * PI * PI;
    x[i] = px; y[i] = py; z[i] = pz;
    vx[i] = qx * DAYS_PER_YEAR; vy[i] = qy * DAYS_PER_YEAR; vz[i] = qz * DAYS_PER_YEAR;
    mass[i] = m * solar_mass;
}

static void init(void) {
    body(0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
    body(1, 4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
         1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
         9.54791938424326609e-04);
    body(2, 8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
         -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
         2.85885980666130812e-04);
    body(3, 1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
         2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
         4.36624404335156298e-05);
    body(4, 1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
         2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
         5.15138902046611451e-05);
}

static void offset_momentum(void) {
    double px = 0.0, py = 0.0, pz = 0.0;
    for (int i = 0; i < BODIES; i++) {
        px = px + vx[i] * mass[i];
        py = py + vy[i] * mass[i];
        pz = pz + vz[i] * mass[i];
    }
    double solar_mass = 4.0 * PI * PI;
    vx[0] = -px / solar_mass;
    vy[0] = -py / solar_mass;
    vz[0] = -pz / solar_mass;
}

static double energy(void) {
    double e = 0.0;
    for (int i = 0; i < BODIES; i++) {
        e = e + 0.5 * mass[i] * (vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]);
        for (int j = i + 1; j < BODIES; j++) {
            double dx = x[i] - x[j], dy = y[i] - y[j], dz = z[i] - z[j];
            e = e - mass[i] * mass[j] / sqrt(dx * dx + dy * dy + dz * dz);
        }
    }
    return e;
}

static void advance(double dt) {
    for (int i = 0; i < BODIES; i++) {
        for (int j = i + 1; j < BODIES; j++) {
            double dx = x[i] - x[j], dy = y[i] - y[j], dz = z[i] - z[j];
            double d2 = dx * dx + dy * dy + dz * dz;
            double mag = dt / (d2 * sqrt(d2));
            vx[i] = vx[i] - dx * mass[j] * mag;
            vy[i] = vy[i] - dy * mass[j] * mag;
            vz[i] = vz[i] - dz * mass[j] * mag;
            vx[j] = vx[j] + dx * mass[i] * mag;
            vy[j] = vy[j] + dy * mass[i] * mag;
            vz[j] = vz[j] + dz * mass[i] * mag;
        }
    }
    for (int i = 0; i < BODIES; i++) {
        x[i] = x[i] + dt * vx[i];
        y[i] = y[i] + dt * vy[i];
        z[i] = z[i] + dt * vz[i];
    }
}

int main(void) {
    init();
    offset_momentum();
    for (int step = 0; step < STEPS; step++) advance(0.01);
    return (int)((long long)(-energy() * 1e9) % 256);
}
