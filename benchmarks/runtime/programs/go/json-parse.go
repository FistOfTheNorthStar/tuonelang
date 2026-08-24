// json-parse — the equivalent-semantics Go peer for the tuonelang json-parse
// workload, and the parity target ADR-0016 answers: Go's standard
// `encoding/json` unmarshals the same fixed document into `any` and the walk
// folds the identical structural checksum — one node per JSON value, every
// number summed — (nodes * 3 + int64(sum)) % 256 = 54, the exit byte (the
// last round's value, reassigned, not accumulated). The walk is
// order-independent (count + sum), so Go's randomized map iteration cannot
// change the answer.
package main

import (
	"encoding/json"
	"os"
)

const doc = `{"id":42,"name":"tuonelang","tags":["fast","safe","native"],` +
	`"metrics":{"stars":128,"forks":32,"score":9.5},"active":true,` +
	`"refs":[1,2,3,4,5,6,7,8]}`

func walk(v any, nodes *int64, sum *float64) {
	*nodes++
	switch value := v.(type) {
	case map[string]any:
		for _, child := range value {
			walk(child, nodes, sum)
		}
	case []any:
		for _, child := range value {
			walk(child, nodes, sum)
		}
	case float64:
		*sum += value
	}
}

func roundTrip() int64 {
	var parsed any
	if err := json.Unmarshal([]byte(doc), &parsed); err != nil {
		return -1
	}
	var nodes int64 = 0
	var sum float64 = 0.0
	walk(parsed, &nodes, &sum)
	return (nodes*3 + int64(sum)) % 256
}

func main() {
	var result int64 = 0
	for r := int64(0); r < 200; r++ {
		result = roundTrip()
	}
	os.Exit(int(result & 0xff))
}
