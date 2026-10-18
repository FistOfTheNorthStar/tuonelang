Implement `gcd(take a: Int, take b: Int) -> Int`, the greatest common divisor of two non-negative integers, so that these specs pass:

```tuo
spec gcd {
    then gcd(12, 18) == 6;
    then gcd(7, 0) == 7;
    then gcd(0, 7) == 7;
}
```

Return only the implementation; the specs are added for you.
