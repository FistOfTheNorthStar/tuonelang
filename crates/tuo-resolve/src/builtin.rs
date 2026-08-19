//! The language-provided builtin functions (ADR-0006 and ADR-0009 Stage A).
//!
//! These functions resolve without any declaration, exactly as the prelude's
//! `Option`/`Some`/`None` resolve without one: the **effect builtins** of
//! `std::rt`, the **pure `Str` builtins** of `std::str` (ADR-0006), and the
//! **pure allocator-core builtins** of `std::string` and `std::array`
//! (ADR-0009). They are installed by [`resolve`](crate::resolve) as real
//! symbols in real, always-present modules — reached by ordinary path
//! resolution — and have **no tuonelang bodies**: the type checker knows
//! their fixed signatures, MIR lowering turns calls to them into dedicated
//! instructions, and the stdlib's loadable `.tuo` modules are a separate,
//! host-loaded mechanism.
//!
//! Because `std::rt`/`std::str`/`std::string`/`std::array` are real modules,
//! a user file declaring `module std::rt;` shares them, and redeclaring
//! `write` there is an ordinary `R0001` duplicate definition — the builtins
//! are not shadowable at their own paths (see
//! `specification/static-semantics.md` §2.4).

/// One language-provided builtin function.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Builtin {
    /// `std::rt::write(take fd: Int, in text: Str) -> Int` — write the
    /// `Str`'s bytes to file descriptor `fd`; returns bytes written, or a
    /// negative value on host error. Never traps. **Effectful.**
    RtWrite,
    /// `std::rt::read_byte(take fd: Int) -> Int` — read one byte from
    /// `fd`; returns `0..=255`, `-1` on end of input, or another negative
    /// value on host error. Never traps. **Effectful.**
    RtReadByte,
    /// `std::rt::exit(take code: Int) -> Int` — terminate the process with
    /// `code & 0xff` as the exit status. Declared as returning `Int` so it
    /// composes in expression position, but it **never returns**.
    /// **Effectful.**
    RtExit,
    /// `std::rt::write_string(take fd: Int, in s: String) -> Int` — write
    /// the owned `String`'s bytes to `fd` (the `String` is lent read-only
    /// for the call); returns bytes written, or a negative value on host
    /// error. Never traps. **Effectful.** (ADR-0009.)
    RtWriteString,
    /// `std::str::len(in s: Str) -> Int` — the byte length of `s`. Pure;
    /// never traps.
    StrLen,
    /// `std::str::byte_at(in s: Str, take index: Int) -> Int` — the byte
    /// (`0..=255`) at `index`; traps `IndexOutOfBounds` when `index < 0`
    /// or `index >= len(s)`. Pure.
    StrByteAt,
    /// `std::str::slice(in s: Str, take start: Int, take end: Int) -> Str`
    /// — the byte range `[start, end)`; traps `IndexOutOfBounds` unless
    /// `0 <= start <= end <= len(s)`. A byte-level operation: the range may
    /// split a multi-byte code point (the documented v0 contract). Pure.
    StrSlice,
    /// `std::string::empty() -> String` — the empty owned string. Pure;
    /// never traps. (ADR-0009.)
    StringEmpty,
    /// `std::string::from_str(in s: Str) -> String` — copy `s`'s bytes
    /// into a new owned buffer. Pure; never traps. (ADR-0009.)
    StringFromStr,
    /// `std::string::push_byte(mut s: String, take b: Int)` — append one
    /// byte to `s` in place; traps `InvalidByte` when `b < 0` or `b > 255`
    /// (the byte range is enforced, never silently masked). Pure.
    /// (ADR-0009.)
    StringPushByte,
    /// `std::string::append(mut s: String, in t: Str)` — append `t`'s
    /// bytes to `s` in place. Pure; never traps. (ADR-0009.)
    StringAppend,
    /// `std::string::concat(in a: Str, in b: Str) -> String` — a new owned
    /// buffer holding `a`'s bytes then `b`'s. Pure; never traps.
    /// (ADR-0009 — the operation ADR-0006's first amendment deferred here.)
    StringConcat,
    /// `std::string::len(in s: String) -> Int` — the byte length of `s`.
    /// Pure; never traps. (ADR-0009.)
    StringLen,
    /// `std::string::byte_at(in s: String, take i: Int) -> Int` — the byte
    /// (`0..=255`) at `i`; traps `IndexOutOfBounds` when `i < 0` or
    /// `i >= len(s)`. Pure. (ADR-0009.)
    StringByteAt,
    /// `std::string::slice(in s: String, take a: Int, take b: Int) ->
    /// String` — the byte range `[a, b)` copied out as a new owned
    /// `String` (no aliasing view — Q-0012 stays deferred); traps
    /// `IndexOutOfBounds` unless `0 <= a <= b <= len(s)`. Byte-level: the
    /// range may split a multi-byte code point. Pure. (ADR-0009.)
    StringSlice,
    /// `std::string::as_str(in s: String) -> Str` — a borrowed `Str` view of
    /// `s`'s bytes (the `{ptr, len}` prefix of the header), **no copy**. The
    /// returned `Str` is a **shared borrow** of `s`: it is valid only while
    /// `s` is (the ownership checker keeps `s` shared-borrowed for as long as
    /// the view is live, refusing a move/mutate/drop of `s` while it is —
    /// `O0001`/`O0002`/`O0005`, and `O0011` for a view escaping its frame).
    /// Pure; never traps. (ADR-0010 — resolves Q-0012.)
    StringAsStr,
    /// `std::array::empty() -> Array[Int]` — the empty growable array.
    /// Pure; never traps. (ADR-0009.)
    ArrayEmpty,
    /// `std::array::push(mut xs: Array[Int], take v: Int)` — append `v`
    /// to `xs` in place. Pure; never traps. (ADR-0009.)
    ArrayPush,
    /// `std::array::pop(mut xs: Array[Int]) -> Option[Int]` — remove and
    /// return the last element, or `None` when empty. Pure; never traps.
    /// (ADR-0009.)
    ArrayPop,
    /// `std::array::len(in xs: Array[Int]) -> Int` — the element count.
    /// Pure; never traps. (ADR-0009.)
    ArrayLen,
    /// `std::array::get(in xs: Array[Int], take i: Int) -> Int` — the
    /// element at `i`; traps `IndexOutOfBounds` when `i < 0` or
    /// `i >= len(xs)`. Pure. (ADR-0009.)
    ArrayGet,
}

/// How one builtin parameter receives its argument (the surface
/// `take`/`in`/`mut` mode of the fixed signature).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinParamMode {
    /// `take` — the callee owns the argument.
    Take,
    /// `in` — the argument is lent read-only for the call.
    In,
    /// `mut` — the argument is lent exclusively for the call and may be
    /// mutated through (the ADR-0009 in-place mutators). Requires a
    /// mutable place, exactly as any `mut` argument does (`O0004`).
    Mut,
}

impl Builtin {
    /// Every builtin, in a fixed installation order.
    pub const ALL: [Self; 21] = [
        Self::RtWrite,
        Self::RtReadByte,
        Self::RtExit,
        Self::RtWriteString,
        Self::StrLen,
        Self::StrByteAt,
        Self::StrSlice,
        Self::StringEmpty,
        Self::StringFromStr,
        Self::StringPushByte,
        Self::StringAppend,
        Self::StringConcat,
        Self::StringLen,
        Self::StringByteAt,
        Self::StringSlice,
        Self::StringAsStr,
        Self::ArrayEmpty,
        Self::ArrayPush,
        Self::ArrayPop,
        Self::ArrayLen,
        Self::ArrayGet,
    ];

    /// The path of the module the builtin lives in.
    #[must_use]
    pub const fn module_path(self) -> &'static [&'static str] {
        match self {
            Self::RtWrite | Self::RtReadByte | Self::RtExit | Self::RtWriteString => &["std", "rt"],
            Self::StrLen | Self::StrByteAt | Self::StrSlice => &["std", "str"],
            Self::StringEmpty
            | Self::StringFromStr
            | Self::StringPushByte
            | Self::StringAppend
            | Self::StringConcat
            | Self::StringLen
            | Self::StringByteAt
            | Self::StringSlice
            | Self::StringAsStr => &["std", "string"],
            Self::ArrayEmpty
            | Self::ArrayPush
            | Self::ArrayPop
            | Self::ArrayLen
            | Self::ArrayGet => &["std", "array"],
        }
    }

    /// The builtin's unqualified name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RtWrite => "write",
            Self::RtReadByte => "read_byte",
            Self::RtExit => "exit",
            Self::RtWriteString => "write_string",
            Self::StrLen | Self::StringLen | Self::ArrayLen => "len",
            Self::StrByteAt | Self::StringByteAt => "byte_at",
            Self::StrSlice | Self::StringSlice => "slice",
            Self::StringEmpty | Self::ArrayEmpty => "empty",
            Self::StringFromStr => "from_str",
            Self::StringPushByte => "push_byte",
            Self::StringAppend => "append",
            Self::StringConcat => "concat",
            Self::StringAsStr => "as_str",
            Self::ArrayPush => "push",
            Self::ArrayPop => "pop",
            Self::ArrayGet => "get",
        }
    }

    /// The fully qualified path, for diagnostics (`std::rt::write`).
    #[must_use]
    pub const fn qualified_name(self) -> &'static str {
        match self {
            Self::RtWrite => "std::rt::write",
            Self::RtReadByte => "std::rt::read_byte",
            Self::RtExit => "std::rt::exit",
            Self::RtWriteString => "std::rt::write_string",
            Self::StrLen => "std::str::len",
            Self::StrByteAt => "std::str::byte_at",
            Self::StrSlice => "std::str::slice",
            Self::StringEmpty => "std::string::empty",
            Self::StringFromStr => "std::string::from_str",
            Self::StringPushByte => "std::string::push_byte",
            Self::StringAppend => "std::string::append",
            Self::StringConcat => "std::string::concat",
            Self::StringLen => "std::string::len",
            Self::StringByteAt => "std::string::byte_at",
            Self::StringSlice => "std::string::slice",
            Self::StringAsStr => "std::string::as_str",
            Self::ArrayEmpty => "std::array::empty",
            Self::ArrayPush => "std::array::push",
            Self::ArrayPop => "std::array::pop",
            Self::ArrayLen => "std::array::len",
            Self::ArrayGet => "std::array::get",
        }
    }

    /// Is this builtin **effectful** (a `std::rt` host effect, ADR-0006)?
    /// The `std::str` builtins and the ADR-0009 allocator-core builtins
    /// (`std::string`/`std::array`) are pure computation — allocation is
    /// deterministic, not I/O — so specs may reach them freely.
    #[must_use]
    pub const fn is_effect(self) -> bool {
        matches!(
            self,
            Self::RtWrite | Self::RtReadByte | Self::RtExit | Self::RtWriteString
        )
    }

    /// The declared parameter modes, in declaration order.
    #[must_use]
    pub const fn param_modes(self) -> &'static [BuiltinParamMode] {
        use BuiltinParamMode::{In, Mut, Take};
        match self {
            Self::RtWrite => &[Take, In],
            Self::RtReadByte | Self::RtExit => &[Take],
            Self::RtWriteString => &[Take, In],
            Self::StrLen => &[In],
            Self::StrByteAt => &[In, Take],
            Self::StrSlice => &[In, Take, Take],
            Self::StringEmpty | Self::ArrayEmpty => &[],
            Self::StringFromStr => &[In],
            Self::StringPushByte => &[Mut, Take],
            Self::StringAppend => &[Mut, In],
            Self::StringConcat => &[In, In],
            Self::StringLen | Self::ArrayLen | Self::StringAsStr => &[In],
            Self::StringByteAt | Self::ArrayGet => &[In, Take],
            Self::StringSlice => &[In, Take, Take],
            Self::ArrayPush => &[Mut, Take],
            Self::ArrayPop => &[Mut],
        }
    }
}
