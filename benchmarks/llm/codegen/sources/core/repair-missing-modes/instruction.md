Fix this program so it compiles. Every parameter needs a mode; pick the one that fits how the parameter is used.

```tuo
module solution;

struct Counter { count: Int }

fn bump(c: Counter, by: Int) {
    c.count = c.count + by;
}

fn bumped_three_times(start: Int) -> Int {
    var c = Counter { count: start };
    bump(c, 1);
    bump(c, 2);
    bump(c, 3);
    c.count
}
```
