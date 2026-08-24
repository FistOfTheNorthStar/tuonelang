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
    /// `std::rt::par_map(take f: fn(take Int) -> Int, in tasks: Array[Int],
    /// take workers: Int) -> Array[Int]` — apply the non-capturing function
    /// value `f` to every task, distributing the tasks round-robin over
    /// `workers` OS threads (task `i` runs on thread `i % workers`; a
    /// `workers` below 1 is treated as 1), and return the results **in task
    /// order**. The call joins every thread before returning (structured
    /// fork-join — nothing outlives the call), and the only values that
    /// cross a thread boundary are the `Copy` code pointer, `Copy` `Int`
    /// tasks read from a shared read-only buffer, and each thread's own
    /// disjoint result slot — so no data race is expressible through it
    /// (ADR-0007). Deterministic in its result whenever `f` is pure. Never
    /// traps. **Effectful** (spawning is a typed effect).
    RtParMap,
    /// `std::rt::now_nanos() -> Int` — the monotonic clock, in nanoseconds
    /// since an arbitrary process-local epoch; only differences are
    /// meaningful. Never traps. **Effectful** (non-deterministic).
    /// (ADR-0013.)
    RtNowNanos,
    /// `std::rt::arg_count() -> Int` — the number of process arguments,
    /// including the program name (argv\[0\]). Never traps. **Effectful.**
    /// (ADR-0013.)
    RtArgCount,
    /// `std::rt::arg_byte(take i: Int, take j: Int) -> Int` — byte `j`
    /// (`0..=255`) of process argument `i`, or `-1` when `i` is out of
    /// range or `j` is past that argument's end. Never traps.
    /// **Effectful.** (ADR-0013.)
    RtArgByte,
    /// `std::rt::open(in path: Str, take mode: Int) -> Int` — open the file
    /// at `path`; returns a file descriptor (`>= 0`), `-2` when the path
    /// does not exist, or another negative value on host error (an unknown
    /// `mode` included). Modes: `0` read, `1` write (create + truncate),
    /// `2` append (create). Never traps. **Effectful.** (ADR-0013.)
    RtOpen,
    /// `std::rt::close(take fd: Int) -> Int` — close `fd`; `0` on success,
    /// negative on host error. Never traps. **Effectful.** (ADR-0013.)
    RtClose,
    /// `std::rt::remove_file(in path: Str) -> Int` — remove the file at
    /// `path`; `0` on success, `-2` when it does not exist, another
    /// negative value on other host errors. Never traps. **Effectful.**
    /// (ADR-0013.)
    RtRemoveFile,
    /// `std::rt::listen(take port: Int) -> Int` — create an IPv4 TCP socket
    /// bound to `127.0.0.1:port` and listening (backlog 16, `SO_REUSEADDR`);
    /// returns the listening descriptor (`>= 0`) or a negative value on host
    /// error. Port `0` asks the host for an ephemeral port — pair with
    /// `bound_port`. Never traps. **Effectful.** (ADR-0014.)
    RtListen,
    /// `std::rt::bound_port(take fd: Int) -> Int` — the local port a
    /// listening descriptor is actually bound to (`getsockname`), or a
    /// negative value on host error. Never traps. **Effectful.**
    /// (ADR-0014.)
    RtBoundPort,
    /// `std::rt::accept(take fd: Int) -> Int` — accept one pending
    /// connection on a listening descriptor; returns the connected
    /// descriptor (`>= 0`) or a negative value on host error. Blocks until
    /// a connection arrives. Never traps. **Effectful.** (ADR-0014.)
    RtAccept,
    /// `std::rt::connect(in host: Str, take port: Int) -> Int` — open a TCP
    /// connection to `host:port` (`host` a numeric IPv4 address such as
    /// `"127.0.0.1"` — no name resolution); returns the connected
    /// descriptor (`>= 0`) or a negative value on host error. Never traps.
    /// **Effectful.** (ADR-0014.)
    RtConnect,
    /// `std::rt::chan_new() -> Int` — create an unbounded FIFO channel of
    /// non-negative `Int` values; returns a channel handle (`>= 0`) or `-1`
    /// when the registry is exhausted. Handles are process-lived. Never
    /// traps. **Effectful.** (ADR-0015.)
    RtChanNew,
    /// `std::rt::chan_send(take ch: Int, take v: Int) -> Int` — enqueue
    /// `v`; `0` on success, `-1` on an invalid handle, a closed channel, or
    /// a negative `v` (refused so `chan_recv`'s `-1` stays unambiguous).
    /// Never traps. **Effectful.** (ADR-0015.)
    RtChanSend,
    /// `std::rt::chan_recv(take ch: Int) -> Int` — dequeue the oldest
    /// value, blocking until one is available; returns the value, or `-1`
    /// once the channel is closed and drained (or the handle is invalid).
    /// Never traps. **Effectful.** (ADR-0015.)
    RtChanRecv,
    /// `std::rt::chan_close(take ch: Int) -> Int` — close the channel:
    /// sends are refused and every blocked or future receive returns `-1`
    /// after the queue drains. `0` on success (idempotent), `-1` on an
    /// invalid handle. Never traps. **Effectful.** (ADR-0015.)
    RtChanClose,
    /// `std::rt::mutex_new() -> Int` — create a mutex; returns a handle
    /// (`>= 0`) or `-1` when the registry is exhausted. Handles are
    /// process-lived. Never traps. **Effectful.** (ADR-0015.)
    RtMutexNew,
    /// `std::rt::mutex_lock(take m: Int) -> Int` — acquire, blocking until
    /// available; `0` on success, `-1` on an invalid handle or a host error
    /// (a relock by the holding thread included — error-checking, never
    /// undefined behavior). Never traps. **Effectful.** (ADR-0015.)
    RtMutexLock,
    /// `std::rt::mutex_unlock(take m: Int) -> Int` — release; `0` on
    /// success, `-1` on an invalid handle or when the calling thread does
    /// not hold it. Never traps. **Effectful.** (ADR-0015.)
    RtMutexUnlock,
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
    /// `std::array::set(mut xs: Array[T], take i: Int, take v: T)` —
    /// overwrite the element at `i` with `v` in place (the previous element
    /// is dropped); traps `IndexOutOfBounds` when `i < 0` or `i >= len(xs)`
    /// — `set` never grows the array. Pure. (ADR-0016.)
    ArraySet,
    /// `std::map::empty() -> Map[K, V]` — a new empty hash map. The key and
    /// value types are witnessed by context (an undetermined pair is
    /// `T0011`); the v0 surface supports `Map[Int, Int]` and
    /// `Map[Str, Int]`. Pure; never traps. (ADR-0011.)
    MapEmpty,
    /// `std::map::insert(mut m: Map[K, V], take k: K, take v: V) ->
    /// Option[V]` — insert/overwrite `k → v`, returning the **previous**
    /// value for `k` (`Some`) or `None` if `k` was absent. Pure; never
    /// traps. (ADR-0011.)
    MapInsert,
    /// `std::map::get(in m: Map[K, V], take k: K) -> Option[V]` — the value
    /// for `k`, or `None`. Pure; never traps. (ADR-0011.)
    MapGet,
    /// `std::map::contains_key(in m: Map[K, V], take k: K) -> Bool` — is
    /// `k` present? Pure; never traps. (ADR-0011.)
    MapContainsKey,
    /// `std::map::remove(mut m: Map[K, V], take k: K) -> Option[V]` —
    /// remove `k`, returning its value (`Some`) or `None`. The remaining
    /// entries keep their relative insertion order. Pure; never traps.
    /// (ADR-0011.)
    MapRemove,
    /// `std::map::len(in m: Map[K, V]) -> Int` — the entry count. Pure;
    /// never traps. (ADR-0011.)
    MapLen,
    /// `std::map::keys(in m: Map[K, V]) -> Array[K]` — a **new** array of
    /// the map's keys in **insertion order** (the deterministic order the
    /// ADR requires; `remove` preserves the relative order of the rest).
    /// Pure; never traps. (ADR-0011.)
    MapKeys,
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
    pub const ALL: [Self; 47] = [
        Self::RtWrite,
        Self::RtReadByte,
        Self::RtExit,
        Self::RtWriteString,
        Self::RtParMap,
        Self::RtNowNanos,
        Self::RtArgCount,
        Self::RtArgByte,
        Self::RtOpen,
        Self::RtClose,
        Self::RtRemoveFile,
        Self::RtListen,
        Self::RtBoundPort,
        Self::RtAccept,
        Self::RtConnect,
        Self::RtChanNew,
        Self::RtChanSend,
        Self::RtChanRecv,
        Self::RtChanClose,
        Self::RtMutexNew,
        Self::RtMutexLock,
        Self::RtMutexUnlock,
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
        Self::ArraySet,
        Self::MapEmpty,
        Self::MapInsert,
        Self::MapGet,
        Self::MapContainsKey,
        Self::MapRemove,
        Self::MapLen,
        Self::MapKeys,
    ];

    /// The path of the module the builtin lives in.
    #[must_use]
    pub const fn module_path(self) -> &'static [&'static str] {
        match self {
            Self::RtWrite
            | Self::RtReadByte
            | Self::RtExit
            | Self::RtWriteString
            | Self::RtParMap
            | Self::RtNowNanos
            | Self::RtArgCount
            | Self::RtArgByte
            | Self::RtOpen
            | Self::RtClose
            | Self::RtRemoveFile
            | Self::RtListen
            | Self::RtBoundPort
            | Self::RtAccept
            | Self::RtConnect
            | Self::RtChanNew
            | Self::RtChanSend
            | Self::RtChanRecv
            | Self::RtChanClose
            | Self::RtMutexNew
            | Self::RtMutexLock
            | Self::RtMutexUnlock => &["std", "rt"],
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
            | Self::ArrayGet
            | Self::ArraySet => &["std", "array"],
            Self::MapEmpty
            | Self::MapInsert
            | Self::MapGet
            | Self::MapContainsKey
            | Self::MapRemove
            | Self::MapLen
            | Self::MapKeys => &["std", "map"],
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
            Self::RtParMap => "par_map",
            Self::RtNowNanos => "now_nanos",
            Self::RtArgCount => "arg_count",
            Self::RtArgByte => "arg_byte",
            Self::RtOpen => "open",
            Self::RtClose => "close",
            Self::RtRemoveFile => "remove_file",
            Self::RtListen => "listen",
            Self::RtBoundPort => "bound_port",
            Self::RtAccept => "accept",
            Self::RtConnect => "connect",
            Self::RtChanNew => "chan_new",
            Self::RtChanSend => "chan_send",
            Self::RtChanRecv => "chan_recv",
            Self::RtChanClose => "chan_close",
            Self::RtMutexNew => "mutex_new",
            Self::RtMutexLock => "mutex_lock",
            Self::RtMutexUnlock => "mutex_unlock",
            Self::StrLen | Self::StringLen | Self::ArrayLen | Self::MapLen => "len",
            Self::StrByteAt | Self::StringByteAt => "byte_at",
            Self::StrSlice | Self::StringSlice => "slice",
            Self::StringEmpty | Self::ArrayEmpty | Self::MapEmpty => "empty",
            Self::StringFromStr => "from_str",
            Self::StringPushByte => "push_byte",
            Self::StringAppend => "append",
            Self::StringConcat => "concat",
            Self::StringAsStr => "as_str",
            Self::ArrayPush => "push",
            Self::ArrayPop => "pop",
            Self::ArraySet => "set",
            Self::ArrayGet | Self::MapGet => "get",
            Self::MapInsert => "insert",
            Self::MapContainsKey => "contains_key",
            Self::MapRemove => "remove",
            Self::MapKeys => "keys",
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
            Self::RtParMap => "std::rt::par_map",
            Self::RtNowNanos => "std::rt::now_nanos",
            Self::RtArgCount => "std::rt::arg_count",
            Self::RtArgByte => "std::rt::arg_byte",
            Self::RtOpen => "std::rt::open",
            Self::RtClose => "std::rt::close",
            Self::RtRemoveFile => "std::rt::remove_file",
            Self::RtListen => "std::rt::listen",
            Self::RtBoundPort => "std::rt::bound_port",
            Self::RtAccept => "std::rt::accept",
            Self::RtConnect => "std::rt::connect",
            Self::RtChanNew => "std::rt::chan_new",
            Self::RtChanSend => "std::rt::chan_send",
            Self::RtChanRecv => "std::rt::chan_recv",
            Self::RtChanClose => "std::rt::chan_close",
            Self::RtMutexNew => "std::rt::mutex_new",
            Self::RtMutexLock => "std::rt::mutex_lock",
            Self::RtMutexUnlock => "std::rt::mutex_unlock",
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
            Self::ArraySet => "std::array::set",
            Self::MapEmpty => "std::map::empty",
            Self::MapInsert => "std::map::insert",
            Self::MapGet => "std::map::get",
            Self::MapContainsKey => "std::map::contains_key",
            Self::MapRemove => "std::map::remove",
            Self::MapLen => "std::map::len",
            Self::MapKeys => "std::map::keys",
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
            Self::RtWrite
                | Self::RtReadByte
                | Self::RtExit
                | Self::RtWriteString
                | Self::RtParMap
                | Self::RtNowNanos
                | Self::RtArgCount
                | Self::RtArgByte
                | Self::RtOpen
                | Self::RtClose
                | Self::RtRemoveFile
                | Self::RtListen
                | Self::RtBoundPort
                | Self::RtAccept
                | Self::RtConnect
                | Self::RtChanNew
                | Self::RtChanSend
                | Self::RtChanRecv
                | Self::RtChanClose
                | Self::RtMutexNew
                | Self::RtMutexLock
                | Self::RtMutexUnlock
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
            Self::RtParMap => &[Take, In, Take],
            Self::RtNowNanos | Self::RtArgCount => &[],
            Self::RtArgByte => &[Take, Take],
            Self::RtOpen => &[In, Take],
            Self::RtClose => &[Take],
            Self::RtRemoveFile => &[In],
            Self::RtListen | Self::RtBoundPort | Self::RtAccept => &[Take],
            Self::RtConnect => &[In, Take],
            Self::RtChanNew | Self::RtMutexNew => &[],
            Self::RtChanSend => &[Take, Take],
            Self::RtChanRecv | Self::RtChanClose | Self::RtMutexLock | Self::RtMutexUnlock => {
                &[Take]
            }
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
            Self::ArraySet => &[Mut, Take, Take],
            Self::MapEmpty => &[],
            Self::MapInsert => &[Mut, Take, Take],
            Self::MapGet | Self::MapContainsKey => &[In, Take],
            Self::MapRemove => &[Mut, Take],
            Self::MapLen | Self::MapKeys => &[In],
        }
    }
}
