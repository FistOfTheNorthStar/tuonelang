//! The language-provided builtin functions (ADR-0006 Stage A).
//!
//! Six functions resolve without any declaration, exactly as the prelude's
//! `Option`/`Some`/`None` resolve without one: the three **effect builtins**
//! of `std::rt` and the three **pure string builtins** of `std::str`. They
//! are installed by [`resolve`](crate::resolve) as real symbols in real,
//! always-present modules — reached by ordinary path resolution — and have
//! **no tuonelang bodies**: the type checker knows their fixed signatures,
//! MIR lowering turns calls to them into dedicated instructions, and the
//! stdlib's loadable `.tuo` modules are a separate, host-loaded mechanism.
//!
//! Because `std::rt`/`std::str` are real modules, a user file declaring
//! `module std::rt;` shares them, and redeclaring `write` there is an
//! ordinary `R0001` duplicate definition — the builtins are not shadowable
//! at their own paths (see `specification/static-semantics.md` §2.4).

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
}

/// How one builtin parameter receives its argument (the surface `take`/`in`
/// mode of the fixed signature). Builtins declare no `mut` parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinParamMode {
    /// `take` — the callee owns the argument.
    Take,
    /// `in` — the argument is lent read-only for the call.
    In,
}

impl Builtin {
    /// Every builtin, in a fixed installation order.
    pub const ALL: [Self; 6] = [
        Self::RtWrite,
        Self::RtReadByte,
        Self::RtExit,
        Self::StrLen,
        Self::StrByteAt,
        Self::StrSlice,
    ];

    /// The path of the module the builtin lives in.
    #[must_use]
    pub const fn module_path(self) -> &'static [&'static str] {
        match self {
            Self::RtWrite | Self::RtReadByte | Self::RtExit => &["std", "rt"],
            Self::StrLen | Self::StrByteAt | Self::StrSlice => &["std", "str"],
        }
    }

    /// The builtin's unqualified name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RtWrite => "write",
            Self::RtReadByte => "read_byte",
            Self::RtExit => "exit",
            Self::StrLen => "len",
            Self::StrByteAt => "byte_at",
            Self::StrSlice => "slice",
        }
    }

    /// The fully qualified path, for diagnostics (`std::rt::write`).
    #[must_use]
    pub const fn qualified_name(self) -> &'static str {
        match self {
            Self::RtWrite => "std::rt::write",
            Self::RtReadByte => "std::rt::read_byte",
            Self::RtExit => "std::rt::exit",
            Self::StrLen => "std::str::len",
            Self::StrByteAt => "std::str::byte_at",
            Self::StrSlice => "std::str::slice",
        }
    }

    /// Is this builtin **effectful** (a `std::rt` host effect, ADR-0006)?
    /// The `std::str` builtins are pure computation.
    #[must_use]
    pub const fn is_effect(self) -> bool {
        matches!(self, Self::RtWrite | Self::RtReadByte | Self::RtExit)
    }

    /// The declared parameter modes, in declaration order.
    #[must_use]
    pub const fn param_modes(self) -> &'static [BuiltinParamMode] {
        use BuiltinParamMode::{In, Take};
        match self {
            Self::RtWrite => &[Take, In],
            Self::RtReadByte | Self::RtExit => &[Take],
            Self::StrLen => &[In],
            Self::StrByteAt => &[In, Take],
            Self::StrSlice => &[In, Take, Take],
        }
    }
}
