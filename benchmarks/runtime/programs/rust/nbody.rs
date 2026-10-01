//! nbody — the equivalent-semantics Rust peer for the tuonelang nbody workload
//! (the Benchmarks Game's n-body): the same five bodies in parallel columns,
//! the same pairwise update order, the same arithmetic (rustc never fuses a
//! multiply-add), 1,000,000 steps of dt = 0.01. Exit byte:
//! (-energy * 1e9) as i64 % 256.

const BODIES: usize = 5;
const STEPS: usize = 1_000_000;
const PI: f64 = 3.141592653589793;
const DAYS_PER_YEAR: f64 = 365.24;

struct Bodies {
    x: [f64; BODIES],
    y: [f64; BODIES],
    z: [f64; BODIES],
    vx: [f64; BODIES],
    vy: [f64; BODIES],
    vz: [f64; BODIES],
    mass: [f64; BODIES],
}

fn system() -> Bodies {
    let p: [[f64; 7]; BODIES] = [
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        [
            4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
            1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
            9.54791938424326609e-04,
        ],
        [
            8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
            -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
            2.85885980666130812e-04,
        ],
        [
            1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
            2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
            4.36624404335156298e-05,
        ],
        [
            1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
            2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
            5.15138902046611451e-05,
        ],
    ];
    let solar_mass = 4.0 * PI * PI;
    let mut b = Bodies {
        x: [0.0; BODIES],
        y: [0.0; BODIES],
        z: [0.0; BODIES],
        vx: [0.0; BODIES],
        vy: [0.0; BODIES],
        vz: [0.0; BODIES],
        mass: [0.0; BODIES],
    };
    for i in 0..BODIES {
        b.x[i] = p[i][0];
        b.y[i] = p[i][1];
        b.z[i] = p[i][2];
        b.vx[i] = p[i][3] * DAYS_PER_YEAR;
        b.vy[i] = p[i][4] * DAYS_PER_YEAR;
        b.vz[i] = p[i][5] * DAYS_PER_YEAR;
        b.mass[i] = p[i][6] * solar_mass;
    }
    b
}

fn offset_momentum(b: &mut Bodies) {
    let (mut px, mut py, mut pz) = (0.0, 0.0, 0.0);
    for i in 0..BODIES {
        px = px + b.vx[i] * b.mass[i];
        py = py + b.vy[i] * b.mass[i];
        pz = pz + b.vz[i] * b.mass[i];
    }
    let solar_mass = 4.0 * PI * PI;
    b.vx[0] = -px / solar_mass;
    b.vy[0] = -py / solar_mass;
    b.vz[0] = -pz / solar_mass;
}

fn energy(b: &Bodies) -> f64 {
    let mut e = 0.0;
    for i in 0..BODIES {
        e = e + 0.5 * b.mass[i] * (b.vx[i] * b.vx[i] + b.vy[i] * b.vy[i] + b.vz[i] * b.vz[i]);
        for j in i + 1..BODIES {
            let dx = b.x[i] - b.x[j];
            let dy = b.y[i] - b.y[j];
            let dz = b.z[i] - b.z[j];
            e = e - b.mass[i] * b.mass[j] / (dx * dx + dy * dy + dz * dz).sqrt();
        }
    }
    e
}

fn advance(b: &mut Bodies, dt: f64) {
    for i in 0..BODIES {
        for j in i + 1..BODIES {
            let dx = b.x[i] - b.x[j];
            let dy = b.y[i] - b.y[j];
            let dz = b.z[i] - b.z[j];
            let d2 = dx * dx + dy * dy + dz * dz;
            let mag = dt / (d2 * d2.sqrt());
            let (mi, mj) = (b.mass[i], b.mass[j]);
            b.vx[i] = b.vx[i] - dx * mj * mag;
            b.vy[i] = b.vy[i] - dy * mj * mag;
            b.vz[i] = b.vz[i] - dz * mj * mag;
            b.vx[j] = b.vx[j] + dx * mi * mag;
            b.vy[j] = b.vy[j] + dy * mi * mag;
            b.vz[j] = b.vz[j] + dz * mi * mag;
        }
    }
    for i in 0..BODIES {
        b.x[i] = b.x[i] + dt * b.vx[i];
        b.y[i] = b.y[i] + dt * b.vy[i];
        b.z[i] = b.z[i] + dt * b.vz[i];
    }
}

fn main() {
    let mut b = system();
    offset_momentum(&mut b);
    for _ in 0..STEPS {
        advance(&mut b, 0.01);
    }
    std::process::exit(((-energy(&b) * 1e9) as i64 % 256) as i32);
}
