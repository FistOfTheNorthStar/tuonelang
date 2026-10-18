This program is rejected by the ownership checker. Fix it so `twice_len` returns twice the length of its argument, without copying the string.

```tuo
module solution;

fn length(take s: String) -> Int {
    std::string::len(s)
}

fn twice_len(take s: String) -> Int {
    length(s) + length(s)
}
```
