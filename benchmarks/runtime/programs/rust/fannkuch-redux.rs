//! fannkuch-redux — the equivalent-semantics Rust peer for the tuonelang
//! fannkuch-redux workload (the Benchmarks Game's fannkuch-redux): the same
//! rotation-order permutation walk and flip loop for n = 10. Checksum 73196,
//! maximum 38; exit byte (73196 + 38) % 256 = 18.

fn fannkuch(n: usize) -> i64 {
    let mut perm = vec![0i64; n];
    let mut perm1: Vec<i64> = (0..n as i64).collect();
    let mut count = vec![0i64; n];
    let (mut max_flips, mut checksum, mut perm_count) = (0i64, 0i64, 0i64);
    let mut r = n;
    loop {
        while r != 1 {
            count[r - 1] = r as i64;
            r -= 1;
        }
        perm.copy_from_slice(&perm1);
        let mut flips = 0i64;
        let mut k = perm[0] as usize;
        while k != 0 {
            let (mut lo, mut hi) = (0usize, k);
            while lo < hi {
                perm.swap(lo, hi);
                lo += 1;
                hi -= 1;
            }
            flips += 1;
            k = perm[0] as usize;
        }
        if flips > max_flips {
            max_flips = flips;
        }
        if perm_count % 2 == 0 {
            checksum += flips;
        } else {
            checksum -= flips;
        }
        loop {
            if r == n {
                return (checksum + max_flips) % 256;
            }
            let perm0 = perm1[0];
            for i in 0..r {
                perm1[i] = perm1[i + 1];
            }
            perm1[r] = perm0;
            count[r] -= 1;
            if count[r] > 0 {
                break;
            }
            r += 1;
        }
        perm_count += 1;
    }
}

fn main() {
    std::process::exit(fannkuch(10) as i32);
}
