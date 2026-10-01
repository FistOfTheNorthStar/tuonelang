// fannkuch-redux — the equivalent-semantics Go peer for the tuonelang
// fannkuch-redux workload (the Benchmarks Game's fannkuch-redux): the same
// rotation-order permutation walk and flip loop for n = 10. Checksum 73196,
// maximum 38; exit byte (73196 + 38) % 256 = 18.
package main

import "os"

func fannkuch(n int) int64 {
	perm := make([]int64, n)
	perm1 := make([]int64, n)
	count := make([]int64, n)
	var maxFlips, checksum, permCount int64
	for i := range perm1 {
		perm1[i] = int64(i)
	}
	r := n
	for {
		for r != 1 {
			count[r-1] = int64(r)
			r--
		}
		copy(perm, perm1)
		var flips int64
		k := perm[0]
		for k != 0 {
			lo, hi := int64(0), k
			for lo < hi {
				perm[lo], perm[hi] = perm[hi], perm[lo]
				lo++
				hi--
			}
			flips++
			k = perm[0]
		}
		if flips > maxFlips {
			maxFlips = flips
		}
		if permCount%2 == 0 {
			checksum += flips
		} else {
			checksum -= flips
		}
		for {
			if r == n {
				return (checksum + maxFlips) % 256
			}
			perm0 := perm1[0]
			for i := 0; i < r; i++ {
				perm1[i] = perm1[i+1]
			}
			perm1[r] = perm0
			count[r]--
			if count[r] > 0 {
				break
			}
			r++
		}
		permCount++
	}
}

func main() {
	os.Exit(int(fannkuch(10)))
}
