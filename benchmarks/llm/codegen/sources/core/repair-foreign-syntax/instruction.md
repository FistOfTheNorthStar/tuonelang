This program was written with habits from another language and does not compile. Fix it so `total` returns the sum of the four elements, keeping the same function name and parameter type.

```tuo
module solution;

fn total(xs: [Int; 4]) -> Int {
    let mut sum = 0;
    for x in xs {
        sum += x;
    }
    return sum;
}
```
