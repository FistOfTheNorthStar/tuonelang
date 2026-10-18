This program compiles but fails its spec: `sum_to(n)` should return 1 + 2 + ... + n.

```tuo
module solution;

fn sum_to(take n: Int) -> Int {
    var total = 0;
    var i = 1;
    while i < n {
        total = total + i;
        i = i + 1;
    }
    total
}

spec sum_to {
    then sum_to(4) == 10;
}
```

Fix the function, not the spec, and return the program without the spec.
