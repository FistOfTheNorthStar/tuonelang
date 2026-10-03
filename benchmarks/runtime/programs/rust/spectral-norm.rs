//! spectral-norm — the equivalent-semantics Rust peer for the tuonelang
//! spectral-norm workload (the Benchmarks Game's spectral-norm): ten rounds of
//! the power method on a 2000-element vector, the same row sums in the same
//! order. Exit byte: (norm * 1e9) as i64 % 256.

const N: usize = 2000;

fn a(i: i64, j: i64) -> f64 {
    1.0 / (((i + j) * (i + j + 1) / 2 + i + 1) as f64)
}

fn mul_av(v: &[f64], out: &mut [f64]) {
    for i in 0..v.len() {
        let mut sum = 0.0;
        for j in 0..v.len() {
            sum = sum + a(i as i64, j as i64) * v[j];
        }
        out[i] = sum;
    }
}

fn mul_atv(v: &[f64], out: &mut [f64]) {
    for i in 0..v.len() {
        let mut sum = 0.0;
        for j in 0..v.len() {
            sum = sum + a(j as i64, i as i64) * v[j];
        }
        out[i] = sum;
    }
}

fn mul_atav(v: &[f64], out: &mut [f64], tmp: &mut [f64]) {
    mul_av(v, tmp);
    mul_atv(tmp, out);
}

fn main() {
    let mut u = vec![1.0; N];
    let mut v = vec![0.0; N];
    let mut tmp = vec![0.0; N];
    for _ in 0..10 {
        mul_atav(&u, &mut v, &mut tmp);
        mul_atav(&v, &mut u, &mut tmp);
    }
    let (mut vbv, mut vv) = (0.0, 0.0);
    for i in 0..N {
        vbv = vbv + u[i] * v[i];
        vv = vv + v[i] * v[i];
    }
    let norm: f64 = (vbv / vv).sqrt();
    std::process::exit(((norm * 1e9) as i64 % 256) as i32);
}
