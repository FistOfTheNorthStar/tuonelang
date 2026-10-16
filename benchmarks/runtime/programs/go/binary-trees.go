// binary-trees — the equivalent-semantics Go peer for the tuonelang
// binary-trees workload (the Benchmarks Game's binary-trees): a stretch tree
// of depth MAX + 1, a long-lived tree of depth MAX, and 2^(MAX - d + 4) trees
// of each depth d = 4, 6, ..., MAX, every node its own allocation. A walk
// counts a tree's nodes; Go's collector, not the walk, frees them — the
// honest comparison, and a different cost shape. Exit byte: the summed node
// counts % 256.
package main

import "os"

const minDepth = 4
const maxDepth = 16

type Node struct {
	left, right *Node
}

func make_(depth int64) *Node {
	if depth == 0 {
		return &Node{}
	}
	return &Node{left: make_(depth - 1), right: make_(depth - 1)}
}

func check(node *Node) int64 {
	var left, right int64
	if node.left != nil {
		left = check(node.left)
	}
	if node.right != nil {
		right = check(node.right)
	}
	return 1 + left + right
}

func main() {
	total := check(make_(maxDepth + 1))
	longLived := make_(maxDepth)
	for depth := int64(minDepth); depth <= maxDepth; depth += 2 {
		iterations := int64(1) << (maxDepth - depth + minDepth)
		for i := int64(0); i < iterations; i++ {
			total += check(make_(depth))
		}
	}
	total += check(longLived)
	os.Exit(int(total % 256))
}
