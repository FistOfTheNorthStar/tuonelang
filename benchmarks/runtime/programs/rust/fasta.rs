//! fasta — the equivalent-semantics Rust peer for the tuonelang fasta workload
//! (the Benchmarks Game's fasta): the same three generated sequences, the same
//! linear congruential generator, 60 bases per line, every byte folded into
//! sum = (sum * 31 + byte) % 1000003. Exit byte: sum % 256.

const N: i64 = 1_000_000;
const LINE: i64 = 60;
const ALU: &[u8] = b"GGCCGGGCGCGGTGGCTCACGCCTGTAATCCCAGCACTTTGGGAGGCCGAGGCGGGCGGATCACCTGAGGTCAGGAGTTCGAGACCAGCCTGGCCAACATGGTGAAACCCCGTCTCTACTAAAAATACAAAAATTAGCCGGGCGTGGTGGCGCGCGCCTGTAATCCCAGCTACTCGGGAGGCTGAGGCAGGAGAATCGCTTGAACCCGGGAGGCGGAGGTTGCAGTGAGCCGAGATCGCGCCACTGCACTCCAGCCTGGGCGACAGAGCGAGACTCCGTCTCAAAAA";

struct Gen {
    sum: i64,
    last: i64,
}

impl Gen {
    fn emit(&mut self, byte: i64) {
        self.sum = (self.sum * 31 + byte) % 1000003;
    }

    fn random_unit(&mut self) -> f64 {
        self.last = (self.last * 3877 + 29573) % 139968;
        self.last as f64 / 139968.0
    }

    fn repeat(&mut self, n: i64) {
        let (mut at, mut col) = (0usize, 0i64);
        for _ in 0..n {
            self.emit(ALU[at] as i64);
            at += 1;
            if at == ALU.len() {
                at = 0;
            }
            col += 1;
            if col == LINE {
                self.emit(10);
                col = 0;
            }
        }
        if col > 0 {
            self.emit(10);
        }
    }

    fn random_seq(&mut self, codes: &[u8], cumulative: &[f64], n: i64) {
        let mut col = 0;
        for _ in 0..n {
            let r = self.random_unit();
            let mut k = 0;
            while k < cumulative.len() - 1 && r >= cumulative[k] {
                k += 1;
            }
            self.emit(codes[k] as i64);
            col += 1;
            if col == LINE {
                self.emit(10);
                col = 0;
            }
        }
        if col > 0 {
            self.emit(10);
        }
    }
}

fn cumulate(p: &[f64]) -> Vec<f64> {
    let mut acc = 0.0;
    p.iter()
        .map(|x| {
            acc = acc + x;
            acc
        })
        .collect()
}

fn main() {
    let iub = cumulate(&[
        0.27, 0.12, 0.12, 0.27, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02,
    ]);
    let homo = cumulate(&[0.3029549426680, 0.1979883004921, 0.1975473066391, 0.3015094502008]);
    let mut g = Gen { sum: 0, last: 42 };
    g.repeat(2 * N);
    g.random_seq(b"acgtBDHKMNRSVWY", &iub, 3 * N);
    g.random_seq(b"acgt", &homo, 5 * N);
    std::process::exit((g.sum % 256) as i32);
}
