// spectral-norm — the equivalent-semantics Go peer for the tuonelang
// spectral-norm workload (the Benchmarks Game's spectral-norm): ten rounds of
// the power method on a 2000-element vector, the same row sums in the same
// order; every product feeding a sum is wrapped in float64 so Go cannot fuse
// it into an FMA. Exit byte: int64(norm * 1e9) % 256.
package main

import (
	"math"
	"os"
)

const n = 2000

func a(i, j int64) float64 {
	return 1.0 / float64((i+j)*(i+j+1)/2+i+1)
}

func mulAv(v, out []float64) {
	for i := range v {
		sum := 0.0
		for j := range v {
			sum = sum + float64(a(int64(i), int64(j))*v[j])
		}
		out[i] = sum
	}
}

func mulAtv(v, out []float64) {
	for i := range v {
		sum := 0.0
		for j := range v {
			sum = sum + float64(a(int64(j), int64(i))*v[j])
		}
		out[i] = sum
	}
}

func mulAtAv(v, out, tmp []float64) {
	mulAv(v, tmp)
	mulAtv(tmp, out)
}

func main() {
	u := make([]float64, n)
	v := make([]float64, n)
	tmp := make([]float64, n)
	for i := range u {
		u[i] = 1.0
	}
	for round := 0; round < 10; round++ {
		mulAtAv(u, v, tmp)
		mulAtAv(v, u, tmp)
	}
	vbv, vv := 0.0, 0.0
	for i := 0; i < n; i++ {
		vbv = vbv + float64(u[i]*v[i])
		vv = vv + float64(v[i]*v[i])
	}
	norm := math.Sqrt(vbv / vv)
	os.Exit(int(int64(norm*1e9) % 256))
}
