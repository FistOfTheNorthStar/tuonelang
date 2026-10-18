Fix this program so it compiles. `find` returns the index of the first element equal to `needle`, or None.

```tuo
module solution;

fn find(in xs: [Int; 4], take needle: Int) -> Option<Int> {
    var i = 0;
    while i < 4 {
        if xs[i] == needle {
            return Some(i);
        }
        i = i + 1;
    }
    None
}

fn find_or(in xs: [Int; 4], take needle: Int) -> Int {
    match find(xs, needle) {
        Some(i) => i,
        None => -1,
    }
}
```
