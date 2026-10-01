//! mandelbrot — the equivalent-semantics Rust peer for the tuonelang
//! mandelbrot workload (the Benchmarks Game's mandelbrot): the 1600 x 1600
//! bitmap, 50 iterations per point, packed eight pixels to a byte, each byte
//! folded into sum = (sum * 31 + byte) % 1000003. Exit byte: sum % 256.

const N: i64 = 1600;

fn main() {
    let mut sum: i64 = 0;
    for y in 0..N {
        let ci = 2.0 * y as f64 / N as f64 - 1.0;
        let (mut bits, mut count) = (0i64, 0i64);
        for x in 0..N {
            let cr = 2.0 * x as f64 / N as f64 - 1.5;
            let (mut zr, mut zi, mut tr, mut ti) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            let mut i = 0;
            while i < 50 && tr + ti <= 4.0 {
                zi = 2.0 * zr * zi + ci;
                zr = tr - ti + cr;
                tr = zr * zr;
                ti = zi * zi;
                i += 1;
            }
            bits = bits * 2 + if tr + ti <= 4.0 { 1 } else { 0 };
            count += 1;
            if count == 8 {
                sum = (sum * 31 + bits) % 1000003;
                bits = 0;
                count = 0;
            }
        }
        if count > 0 {
            while count < 8 {
                bits *= 2;
                count += 1;
            }
            sum = (sum * 31 + bits) % 1000003;
        }
    }
    std::process::exit((sum % 256) as i32);
}
