Implement `clamp(take value: Int, take low: Int, take high: Int) -> Int` so that these specs pass:

```tuo
spec clamp {
    then clamp(5, 0, 10) == 5;
    then clamp(0 - 3, 0, 10) == 0;
    then clamp(42, 0, 10) == 10;
}
```

Return only the implementation; the specs are added for you.
