This hash aborts at run time with an integer-overflow trap on long inputs, because tuonelang's `+` and `*` trap on overflow instead of wrapping. Fix `hash` so it computes the 32-bit FNV-1a hash of s: start from 2166136261, and for each byte, xor it in and then multiply by 16777619 modulo 2^32. Keep the signature.

```tuo
module solution;

fn hash(in s: Str) -> Int {
    var h = 2166136261;
    var i = 0;
    while i < std::str::len(s) {
        h = h ^ std::str::byte_at(s, i);
        h = h * 16777619;
        i = i + 1;
    }
    h
}
```
