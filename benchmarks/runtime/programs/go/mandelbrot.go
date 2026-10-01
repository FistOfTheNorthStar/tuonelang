// mandelbrot — the equivalent-semantics Go peer for the tuonelang mandelbrot
// workload (the Benchmarks Game's mandelbrot): the 1600 x 1600 bitmap, 50
// iterations per point, packed eight pixels to a byte, each byte folded into
// sum = (sum * 31 + byte) % 1000003. The 2*Zr*Zi product is wrapped in
// float64 so Go cannot fuse it with "+ Ci" (a fused update flips pixels on
// the set's boundary). Exit byte: sum % 256.
package main

import "os"

const n = 1600

func main() {
	var sum int64
	for y := 0; y < n; y++ {
		ci := 2.0*float64(y)/float64(n) - 1.0
		var bits, count int64
		for x := 0; x < n; x++ {
			cr := 2.0*float64(x)/float64(n) - 1.5
			zr, zi, tr, ti := 0.0, 0.0, 0.0, 0.0
			for i := 0; i < 50 && tr+ti <= 4.0; i++ {
				zi = float64(2.0*zr*zi) + ci
				zr = tr - ti + cr
				tr = zr * zr
				ti = zi * zi
			}
			bit := int64(0)
			if tr+ti <= 4.0 {
				bit = 1
			}
			bits = bits*2 + bit
			count++
			if count == 8 {
				sum = (sum*31 + bits) % 1000003
				bits, count = 0, 0
			}
		}
		if count > 0 {
			for count < 8 {
				bits *= 2
				count++
			}
			sum = (sum*31 + bits) % 1000003
		}
	}
	os.Exit(int(sum % 256))
}
