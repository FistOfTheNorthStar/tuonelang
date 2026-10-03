//! The hash-map runtime (ADR-0011).
//!
//! Every native `Map[K, V]` operation beyond a plain header read passes
//! through the C-ABI `tuo_rt_map_*` symbols, exactly as every allocation
//! passes through [`crate::alloc`]: the table's internals — the hash index,
//! probing, growth, entry layout maintenance — live behind this one seam, so
//! neither backend embeds a hash table and the two cannot drift. See
//! [`specification/abi.md`](../../../specification/abi.md) §Maps.
//!
//! **The observable contract is the interpreter's** (an insertion-ordered
//! association list): `keys` returns keys in insertion order, `remove`
//! preserves the relative order of the remaining entries, an overwrite keeps
//! the key's position, and key equality is scalar `==` (integers by value,
//! strings by byte content). The hash function below places keys in buckets
//! and is **never observable** — a program sees only the dense,
//! insertion-ordered entries — but it is still fixed and vector-pinned
//! (no per-run seed) so the table's memory behavior is reproducible too.
//!
//! Memory shape (one block per map, allocated via `tuo_rt_alloc`):
//!
//! ```text
//! block:  [ u64 index[index_cap] ][ u8 live[cap] ][ u64 used ][ u64 index_cap ][ entry entries[cap] ]
//!                                                                              ^ header.ptr
//! ```
//!
//! The program-visible header is the same three words as `String`/`Array` —
//! `{ptr, len, cap}` — where `ptr` points at the **dense entries** region,
//! `len` is the live entry count, and `cap` the entry capacity. The hash
//! index (open addressing, linear probing, `index_cap == 2 × cap`, slots
//! holding `dense_index + 1`, `0` for empty, or a tombstone), the per-entry
//! live bytes, and two bookkeeping words sit *before* the entries in the same
//! block, so the shim can recover the block start from `ptr` and `cap` alone
//! (`index_cap` is at `ptr[-1]`, `used` at `ptr[-2]`) and the header never
//! carries a fourth word. An empty map is the sentinel header
//! `{ZERO_SIZE_SENTINEL, 0, 0}` with no allocation at all.
//!
//! **Removal is O(1) and moves nothing.** It clears the entry's live byte and
//! tombstones its index slot, leaving a hole in the dense region; the
//! survivors keep their positions and therefore their insertion order, and
//! `keys` skips holes. `used` counts the dense slots handed out since the
//! last rehome, live or not, so appends go at `used` and `len <= used <= cap`.
//! Holes are reclaimed only when an insert finds `used == cap`: the live
//! entries are rehomed, packed in order under a fresh tombstone-free index,
//! into a same-capacity block when at most half are live and a doubled one
//! otherwise — so each O(cap) rehome is paid for by at least `cap / 2`
//! inserts. (This is the design CPython's ordered `dict` uses.) Every index
//! tombstone matches a hole below `used`, so the index is never more than
//! half full and every probe terminates.
//!
//! Entry layout: an entry is its key followed by its value, so the **entry
//! stride is `key_size + value_stride`**. An `Int` key occupies one word
//! ([`INT_KEY_SIZE`]); a `Str` key its two-word borrowed view
//! ([`STR_KEY_SIZE`]), stored as the view and never copied. The value slot is
//! `value_stride` bytes of **opaque payload** at offset `key_size`.
//!
//! The value stride is a **parameter the compiler supplies**, not a constant
//! (ADR-0023 Stage B2). Every operation that touches a value takes it, so this
//! shim never learns which type it carries — it memcpy's `value_stride` bytes.
//! Two shapes use that:
//!
//! - a **`Copy` scalar** (`Int`/`Bool`/`Float` — ADR-0023 Stage B) rides in one
//!   machine word, which the *compiler* widens on the way in and narrows on the
//!   way out (a `Float` is bitcast, never converted, so its bits survive). This
//!   is [`WORD_VALUE_STRIDE`], and it is what [`INT_ENTRY_STRIDE`] /
//!   [`STR_ENTRY_STRIDE`] name for compatibility;
//! - a **borrowed `Str`** value ([`STR_VALUE_STRIDE`], Stage B2), a two-word
//!   `{ptr, len}` view copied bytewise like the `Str` *key* already is. `Str`
//!   borrows rather than owns, so it needs no deep copy on read-out and no drop
//!   glue — which is exactly why it lands before `String`.
//!
//! A value type that *owns* heap (`String`, a struct containing one) still
//! needs the deep-copy-on-read and recursive-drop path and stays refused by the
//! type checker (Stage B2 part two).
//!
//! What lives here in Rust is the part that is pure and testable without a C
//! compiler: the two hash functions ([`hash_int`], [`hash_str`]) with their
//! published test vectors, the layout constants, and the symbol names. The
//! module's tests pin the C source to carry the same constants.

/// The C-ABI symbol for `Map[Int, V]` insert:
/// `void tuo_rt_map_int_insert(long long *hdr, long long k, const void *v,
/// unsigned long long vs, long long *out)` — `v` points at `vs` bytes of
/// value payload; `out[0]` is 1 when the key was present (its previous value
/// in the `vs` bytes at `&out[1]`), else 0.
pub const MAP_INT_INSERT_SYMBOL: &str = "tuo_rt_map_int_insert";

/// The C-ABI symbol for `Map[Int, V]` lookup:
/// `void tuo_rt_map_int_get(const long long *hdr, long long k,
/// unsigned long long vs, long long *out)` — `out` as for insert.
pub const MAP_INT_GET_SYMBOL: &str = "tuo_rt_map_int_get";

/// The C-ABI symbol for `Map[Int, V]` removal:
/// `void tuo_rt_map_int_remove(long long *hdr, long long k,
/// unsigned long long vs, long long *out)` — `out` as for insert; the
/// remaining entries keep their relative order.
pub const MAP_INT_REMOVE_SYMBOL: &str = "tuo_rt_map_int_remove";

/// The C-ABI symbol for `Map[Int, V]` key listing:
/// `void tuo_rt_map_int_keys(const long long *hdr, unsigned long long vs,
/// long long *out_hdr)` — writes a fresh `Array[Int]` header
/// (`{ptr, len, cap}`) of the keys in insertion order. `vs` is needed to
/// stride over the entries.
pub const MAP_INT_KEYS_SYMBOL: &str = "tuo_rt_map_int_keys";

/// The C-ABI symbol for `Map[Str, V]` insert (key as `{ptr, len}`, value as
/// `{const void *v, unsigned long long vs}`).
pub const MAP_STR_INSERT_SYMBOL: &str = "tuo_rt_map_str_insert";

/// The C-ABI symbol for `Map[Str, V]` lookup (key as `{ptr, len}`, plus `vs`).
pub const MAP_STR_GET_SYMBOL: &str = "tuo_rt_map_str_get";

/// The C-ABI symbol for `Map[Str, V]` removal (key as `{ptr, len}`, plus `vs`).
pub const MAP_STR_REMOVE_SYMBOL: &str = "tuo_rt_map_str_remove";

/// The C-ABI symbol for `Map[Str, V]` key listing: writes a fresh
/// `Array[Str]` header whose elements are the stored two-word views, in
/// insertion order. Takes `vs` to stride over the entries.
pub const MAP_STR_KEYS_SYMBOL: &str = "tuo_rt_map_str_keys";

/// The C-ABI symbol that frees a map's block:
/// `void tuo_rt_map_drop(long long *hdr, long long stride)` — `stride` is
/// the entry stride ([`INT_ENTRY_STRIDE`]/[`STR_ENTRY_STRIDE`]), which the
/// drop needs only to compute the block size handed back to `tuo_rt_dealloc`.
pub const MAP_DROP_SYMBOL: &str = "tuo_rt_map_drop";

/// The byte size of an `Int` key in an entry: one word.
pub const INT_KEY_SIZE: u64 = 8;

/// The byte size of a `Str` key in an entry: the two-word `{ptr, len}` view.
pub const STR_KEY_SIZE: u64 = 16;

/// The value stride of a word-sized (`Copy` scalar) value — `Int`, `Bool`, or
/// `Float`, which the compiler widens into one machine word (ADR-0023 Stage B).
pub const WORD_VALUE_STRIDE: u64 = 8;

/// The value stride of a borrowed `Str` value: its two-word `{ptr, len}` view
/// (ADR-0023 Stage B2). `Str` borrows rather than owns, so the bytes are copied
/// as-is with no deep copy and no drop glue.
pub const STR_VALUE_STRIDE: u64 = 16;

/// The byte stride of one entry, given its key size and value stride. The
/// entry is `{key, value}` packed in that order; both parts are 8-aligned
/// multiples of a word, so no padding is needed between or after them.
#[must_use]
pub const fn entry_stride(key_size: u64, value_stride: u64) -> u64 {
    key_size + value_stride
}

/// The byte stride of one `Map[Int, V]` entry for a word-sized `V`:
/// `{i64 key, i64 value}`.
pub const INT_ENTRY_STRIDE: u64 = entry_stride(INT_KEY_SIZE, WORD_VALUE_STRIDE);

/// The byte stride of one `Map[Str, V]` entry for a word-sized `V`:
/// `{const u8 *key_ptr, u64 key_len, i64 value}`.
pub const STR_ENTRY_STRIDE: u64 = entry_stride(STR_KEY_SIZE, WORD_VALUE_STRIDE);

/// The smallest non-zero entry capacity the table grows to.
pub const INITIAL_CAPACITY: u64 = 8;

/// The fixed 64-bit integer-key hash: the splitmix64 finalizer. Unseeded and
/// documented (reproducibility over hash-flooding resistance — the ADR-0011
/// trade), and unobservable from tuonelang (only bucket placement uses it).
#[must_use]
pub fn hash_int(key: i64) -> u64 {
    #[expect(
        clippy::cast_sign_loss,
        reason = "bit-reinterpretation is the hash's input"
    )]
    let mut x = key as u64;
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

/// The fixed byte-string hash: 64-bit FNV-1a (offset basis
/// `0xcbf29ce484222325`, prime `0x100000001b3`) — the same hand-rolled,
/// vector-pinned, no-new-dependency precedent `tuo-package`'s `sha256` set.
#[must_use]
pub fn hash_str(key: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in key {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The C source of the map runtime.
///
/// The build driver writes this to a `.c` file and links it into every built
/// binary (exactly as [`crate::alloc`]'s source is linked), so the
/// `tuo_rt_map_*` symbols resolve. Freestanding C over `<string.h>` and
/// `<stdint.h>`; allocation flows through the existing
/// `tuo_rt_alloc`/`tuo_rt_dealloc` boundary — the map runtime performs no
/// allocation of its own.
///
/// The table mirrors the module docs: dense insertion-ordered entries, an
/// open-addressing index of `dense_index + 1` slots with linear probing,
/// `index_cap == 2 × cap` (both powers of two), growth by doubling from
/// [`INITIAL_CAPACITY`], O(1) order-preserving removal by hole + tombstone
/// with holes reclaimed when the dense region fills, and the
/// [`hash_int`]/[`hash_str`] functions with the same constants pinned by this
/// module's tests. `tests/map_model.rs` checks it against the interpreter's
/// association-list semantics.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn map_runtime_c_source() -> String {
    let sentinel = crate::alloc::ZERO_SIZE_SENTINEL;
    format!(
        r#"#include <stdint.h>
#include <string.h>

extern void *tuo_rt_alloc(size_t size, size_t align);
extern void tuo_rt_dealloc(void *ptr, size_t size, size_t align);

/* The program-visible header: {{entries_ptr, len, cap}}, three words.   */
/* `len` is the LIVE entry count. With E = entries_ptr, the block is     */
/*   [ u64 index[index_cap] ][ u8 live[cap] ][ u64 used ][ u64 index_cap ][ entries[cap] ] */
/*                                            = E[-2]    = E[-1]          ^ E             */
/* `used` counts the dense slots handed out since the last rehome, live */
/* or removed, and `live[i]` is 1 while dense slot i holds an entry.    */
/* Index slot: 0 = empty, TUO_MAP_TOMB = a removed entry, else          */
/* dense_index + 1. An empty map is the sentinel header                 */
/* {{{sentinel}, 0, 0}} with no allocation.                             */
/*                                                                      */
/* Removal is O(1): it clears the entry's live byte and tombstones its  */
/* index slot, so the survivors keep their dense positions and hence    */
/* their insertion order. Holes are squeezed out only when an insert    */
/* finds every dense slot used — by a same-capacity compaction when at  */
/* most half are live, else by doubling — so each O(cap) rehome is paid */
/* for by at least cap/2 inserts. Every index tombstone matches a hole  */
/* below `used`, and `used <= cap == index_cap / 2`, so the index never */
/* passes half full and every probe reaches an empty slot.              */

#define TUO_MAP_TOMB UINT64_MAX

/* Copy or clear `n` bytes of an entry part. Values are a word (`Int`,   */
/* `Bool`, `Float`) or two (`Str`) and entries two to four words, so    */
/* those sizes are copied as explicit word loads and stores. A          */
/* `memcpy` whose size is known only at run time is a libc call per map */
/* operation (measured: ~40% of a lookup-heavy workload) — and so is a  */
/* `switch` of fixed-size `memcpy`s, which the compiler folds back into */
/* one variable-size call; word assignments it cannot fold. The         */
/* `may_alias` type makes reading the entry bytes as words well-defined. */
/* Every entry part is 8-byte aligned (the block is, and every stride   */
/* and key size is a multiple of 8).                                    */
typedef uint64_t __attribute__((may_alias)) tuo_map_word;

static void tuo_map_copy(void *dst, const void *src, uint64_t n) {{
    tuo_map_word *d = (tuo_map_word *)dst;
    const tuo_map_word *s = (const tuo_map_word *)src;
    switch (n) {{
        case 8: d[0] = s[0]; return;
        case 16: d[0] = s[0]; d[1] = s[1]; return;
        case 24: d[0] = s[0]; d[1] = s[1]; d[2] = s[2]; return;
        case 32: d[0] = s[0]; d[1] = s[1]; d[2] = s[2]; d[3] = s[3]; return;
        default: memcpy(dst, src, (size_t)n); return;
    }}
}}

static void tuo_map_zero(void *dst, uint64_t n) {{
    tuo_map_word *d = (tuo_map_word *)dst;
    switch (n) {{
        case 8: d[0] = 0; return;
        case 16: d[0] = 0; d[1] = 0; return;
        default: memset(dst, 0, (size_t)n); return;
    }}
}}

static uint64_t tuo_map_hash_int(int64_t key) {{
    uint64_t x = (uint64_t)key;
    x ^= x >> 30; x *= 0xbf58476d1ce4e5b9ULL;
    x ^= x >> 27; x *= 0x94d049bb133111ebULL;
    x ^= x >> 31;
    return x;
}}

static uint64_t tuo_map_hash_str(const unsigned char *ptr, uint64_t len) {{
    uint64_t h = 0xcbf29ce484222325ULL;
    for (uint64_t i = 0; i < len; i++) {{
        h ^= (uint64_t)ptr[i];
        h *= 0x100000001b3ULL;
    }}
    return h;
}}

static uint64_t tuo_map_index_cap(const unsigned char *entries) {{
    return ((const uint64_t *)entries)[-1];
}}

static uint64_t tuo_map_used(const unsigned char *entries) {{
    return ((const uint64_t *)entries)[-2];
}}

static void tuo_map_set_used(unsigned char *entries, uint64_t used) {{
    ((uint64_t *)entries)[-2] = used;
}}

/* `cap` is a power of two >= 8, so the live bytes end word-aligned.   */
static unsigned char *tuo_map_live(const unsigned char *entries, uint64_t cap) {{
    return (unsigned char *)entries - 16 - cap;
}}

static uint64_t *tuo_map_index(const unsigned char *entries, uint64_t cap) {{
    return (uint64_t *)(tuo_map_live(entries, cap) - 8 * tuo_map_index_cap(entries));
}}

static size_t tuo_map_block_size(uint64_t index_cap, uint64_t cap, uint64_t stride) {{
    return (size_t)(8 * index_cap + cap + 16 + cap * stride);
}}

static uint64_t tuo_map_key_hash(const unsigned char *entry, int is_str) {{
    if (is_str) {{
        const unsigned char *kp;
        uint64_t kn;
        memcpy(&kp, entry, 8);
        memcpy(&kn, entry + 8, 8);
        return tuo_map_hash_str(kp, kn);
    }}
    int64_t key;
    memcpy(&key, entry, 8);
    return tuo_map_hash_int(key);
}}

static void tuo_map_index_place(uint64_t *index, uint64_t index_cap,
                                uint64_t hash, uint64_t dense) {{
    uint64_t mask = index_cap - 1;
    uint64_t slot = hash & mask;
    while (index[slot] != 0) slot = (slot + 1) & mask;
    index[slot] = dense + 1;
}}

/* Rehome the live entries into a fresh block of `new_cap` entries — a */
/* first allocation, a doubling, or a same-capacity compaction — packed */
/* in their existing order under a freshly built, tombstone-free index.*/
static unsigned char *tuo_map_rehome(long long *hdr, uint64_t new_cap,
                                     uint64_t stride, int is_str) {{
    uint64_t old_cap = (uint64_t)hdr[2];
    uint64_t new_index_cap = 2 * new_cap;
    unsigned char *block =
        (unsigned char *)tuo_rt_alloc(tuo_map_block_size(new_index_cap, new_cap, stride), 8);
    uint64_t *index = (uint64_t *)block;
    unsigned char *live = block + 8 * new_index_cap;
    unsigned char *entries = live + new_cap + 16;
    ((uint64_t *)entries)[-1] = new_index_cap;
    memset(index, 0, (size_t)(8 * new_index_cap));
    memset(live, 0, (size_t)new_cap);
    uint64_t packed = 0;
    if (old_cap != 0) {{
        unsigned char *old_entries = (unsigned char *)hdr[0];
        const unsigned char *old_live = tuo_map_live(old_entries, old_cap);
        uint64_t old_used = tuo_map_used(old_entries);
        for (uint64_t i = 0; i < old_used; i++) {{
            if (!old_live[i]) continue;
            unsigned char *dst = entries + packed * stride;
            tuo_map_copy(dst, old_entries + i * stride, stride);
            live[packed] = 1;
            tuo_map_index_place(index, new_index_cap, tuo_map_key_hash(dst, is_str), packed);
            packed++;
        }}
        tuo_rt_dealloc(tuo_map_index(old_entries, old_cap),
                       tuo_map_block_size(tuo_map_index_cap(old_entries), old_cap, stride), 8);
    }}
    tuo_map_set_used(entries, packed);
    hdr[0] = (long long)entries;
    hdr[2] = (long long)new_cap;
    return entries;
}}

/* Append a new entry (key bytes, then value bytes) at the next dense  */
/* slot and index it. The caller has already proven the key absent.    */
static void tuo_map_append(long long *hdr, const void *key, uint64_t key_size,
                           const void *v, uint64_t vs, uint64_t hash, int is_str) {{
    uint64_t stride = key_size + vs;
    uint64_t cap = (uint64_t)hdr[2];
    unsigned char *entries = (unsigned char *)hdr[0];
    if (cap == 0) {{
        entries = tuo_map_rehome(hdr, {initial_cap}, stride, is_str);
    }} else if (tuo_map_used(entries) == cap) {{
        uint64_t len = (uint64_t)hdr[1];
        entries = tuo_map_rehome(hdr, 2 * len > cap ? cap * 2 : cap, stride, is_str);
    }}
    cap = (uint64_t)hdr[2];
    uint64_t used = tuo_map_used(entries);
    tuo_map_copy(entries + used * stride, key, key_size);
    tuo_map_copy(entries + used * stride + key_size, v, vs);
    tuo_map_live(entries, cap)[used] = 1;
    tuo_map_index_place(tuo_map_index(entries, cap), tuo_map_index_cap(entries), hash, used);
    tuo_map_set_used(entries, used + 1);
    hdr[1] = hdr[1] + 1;
}}

/* Probe for an int key. Returns the INDEX SLOT that refers to it, or   */
/* (uint64_t)-1; the entry is then `tuo_map_entry_at(hdr, slot, ...)`.  */
static uint64_t tuo_map_find_int(const long long *hdr, int64_t key, uint64_t stride) {{
    uint64_t cap = (uint64_t)hdr[2];
    if (cap == 0) return (uint64_t)-1;
    const unsigned char *entries = (const unsigned char *)hdr[0];
    const uint64_t *index = tuo_map_index(entries, cap);
    uint64_t mask = tuo_map_index_cap(entries) - 1;
    uint64_t slot = tuo_map_hash_int(key) & mask;
    while (index[slot] != 0) {{
        if (index[slot] != TUO_MAP_TOMB) {{
            int64_t stored;
            memcpy(&stored, entries + (index[slot] - 1) * stride, 8);
            if (stored == key) return slot;
        }}
        slot = (slot + 1) & mask;
    }}
    return (uint64_t)-1;
}}

static uint64_t tuo_map_find_str(const long long *hdr, const unsigned char *kp, uint64_t kn,
                                 uint64_t stride) {{
    uint64_t cap = (uint64_t)hdr[2];
    if (cap == 0) return (uint64_t)-1;
    const unsigned char *entries = (const unsigned char *)hdr[0];
    const uint64_t *index = tuo_map_index(entries, cap);
    uint64_t mask = tuo_map_index_cap(entries) - 1;
    uint64_t slot = tuo_map_hash_str(kp, kn) & mask;
    while (index[slot] != 0) {{
        if (index[slot] != TUO_MAP_TOMB) {{
            const unsigned char *entry = entries + (index[slot] - 1) * stride;
            const unsigned char *sp;
            uint64_t sn;
            memcpy(&sp, entry, 8);
            memcpy(&sn, entry + 8, 8);
            if (sn == kn && (kn == 0 || memcmp(sp, kp, (size_t)kn) == 0)) return slot;
        }}
        slot = (slot + 1) & mask;
    }}
    return (uint64_t)-1;
}}

/* The entry an index slot returned by `tuo_map_find_*` refers to.     */
static unsigned char *tuo_map_entry_at(const long long *hdr, uint64_t slot, uint64_t stride) {{
    const unsigned char *entries = (const unsigned char *)hdr[0];
    uint64_t dense = tuo_map_index(entries, (uint64_t)hdr[2])[slot] - 1;
    return (unsigned char *)entries + dense * stride;
}}

/* Remove the entry at index slot `slot`: copy its value out, then      */
/* leave a hole and a tombstone. Nothing moves, so order is preserved. */
static void tuo_map_remove_at(long long *hdr, uint64_t slot, uint64_t key_size,
                              uint64_t vs, long long *out) {{
    unsigned char *entries = (unsigned char *)hdr[0];
    uint64_t cap = (uint64_t)hdr[2];
    uint64_t *index = tuo_map_index(entries, cap);
    uint64_t dense = index[slot] - 1;
    out[0] = 1;
    tuo_map_copy(&out[1], entries + dense * (key_size + vs) + key_size, vs);
    tuo_map_live(entries, cap)[dense] = 0;
    index[slot] = TUO_MAP_TOMB;
    hdr[1] = hdr[1] - 1;
}}

/* Write a fresh Array header of the live keys (`key_size` bytes each) */
/* in insertion order, skipping holes.                                 */
static void tuo_map_keys(const long long *hdr, uint64_t key_size, uint64_t vs,
                         long long *out_hdr) {{
    uint64_t len = (uint64_t)hdr[1];
    if (len == 0) {{ out_hdr[0] = {sentinel}; out_hdr[1] = 0; out_hdr[2] = 0; return; }}
    const unsigned char *entries = (const unsigned char *)hdr[0];
    const unsigned char *live = tuo_map_live(entries, (uint64_t)hdr[2]);
    uint64_t used = tuo_map_used(entries);
    uint64_t stride = key_size + vs;
    unsigned char *buf = (unsigned char *)tuo_rt_alloc((size_t)(key_size * len), 8);
    uint64_t n = 0;
    for (uint64_t i = 0; i < used; i++) {{
        if (!live[i]) continue;
        tuo_map_copy(buf + n * key_size, entries + i * stride, key_size);
        n++;
    }}
    out_hdr[0] = (long long)buf;
    out_hdr[1] = (long long)len;
    out_hdr[2] = (long long)len;
}}

/* Every value-touching entry point takes `vs` — the value stride in bytes  */
/* (ADR-0023 Stage B2). The value sits at offset `key_size` in the entry,   */
/* and the shim copies `vs` opaque bytes: it never learns the value's type. */
/* `out` is the caller's buffer: out[0] is the found flag, and the previous */
/* value (when found) occupies `vs` bytes starting at &out[1].              */

void tuo_rt_map_int_get(const long long *hdr, long long k, unsigned long long vs,
                        long long *out) {{
    uint64_t stride = {int_key} + vs;
    uint64_t slot = tuo_map_find_int(hdr, k, stride);
    if (slot == (uint64_t)-1) {{ out[0] = 0; tuo_map_zero(&out[1], vs); return; }}
    out[0] = 1;
    tuo_map_copy(&out[1], tuo_map_entry_at(hdr, slot, stride) + {int_key}, vs);
}}

void tuo_rt_map_int_insert(long long *hdr, long long k, const void *v,
                           unsigned long long vs, long long *out) {{
    uint64_t stride = {int_key} + vs;
    uint64_t slot = tuo_map_find_int(hdr, k, stride);
    if (slot != (uint64_t)-1) {{
        unsigned char *value = tuo_map_entry_at(hdr, slot, stride) + {int_key};
        tuo_map_copy(&out[1], value, vs);
        out[0] = 1;
        tuo_map_copy(value, v, vs);
        return;
    }}
    out[0] = 0; tuo_map_zero(&out[1], vs);
    tuo_map_append(hdr, &k, {int_key}, v, vs, tuo_map_hash_int(k), 0);
}}

void tuo_rt_map_int_remove(long long *hdr, long long k, unsigned long long vs,
                           long long *out) {{
    uint64_t stride = {int_key} + vs;
    uint64_t slot = tuo_map_find_int(hdr, k, stride);
    if (slot == (uint64_t)-1) {{ out[0] = 0; tuo_map_zero(&out[1], vs); return; }}
    tuo_map_remove_at(hdr, slot, {int_key}, vs, out);
}}

void tuo_rt_map_int_keys(const long long *hdr, unsigned long long vs, long long *out_hdr) {{
    tuo_map_keys(hdr, {int_key}, vs, out_hdr);
}}

void tuo_rt_map_str_get(const long long *hdr, const unsigned char *kp, unsigned long long kn,
                        unsigned long long vs, long long *out) {{
    uint64_t stride = {str_key} + vs;
    uint64_t slot = tuo_map_find_str(hdr, kp, kn, stride);
    if (slot == (uint64_t)-1) {{ out[0] = 0; tuo_map_zero(&out[1], vs); return; }}
    out[0] = 1;
    tuo_map_copy(&out[1], tuo_map_entry_at(hdr, slot, stride) + {str_key}, vs);
}}

void tuo_rt_map_str_insert(long long *hdr, const unsigned char *kp, unsigned long long kn,
                           const void *v, unsigned long long vs, long long *out) {{
    uint64_t stride = {str_key} + vs;
    uint64_t slot = tuo_map_find_str(hdr, kp, kn, stride);
    if (slot != (uint64_t)-1) {{
        unsigned char *value = tuo_map_entry_at(hdr, slot, stride) + {str_key};
        tuo_map_copy(&out[1], value, vs);
        out[0] = 1;
        tuo_map_copy(value, v, vs);
        return;
    }}
    out[0] = 0; tuo_map_zero(&out[1], vs);
    unsigned char key[{str_key}];
    memcpy(key, &kp, 8);
    memcpy(key + 8, &kn, 8);
    tuo_map_append(hdr, key, {str_key}, v, vs, tuo_map_hash_str(kp, kn), 1);
}}

void tuo_rt_map_str_remove(long long *hdr, const unsigned char *kp, unsigned long long kn,
                           unsigned long long vs, long long *out) {{
    uint64_t stride = {str_key} + vs;
    uint64_t slot = tuo_map_find_str(hdr, kp, kn, stride);
    if (slot == (uint64_t)-1) {{ out[0] = 0; tuo_map_zero(&out[1], vs); return; }}
    tuo_map_remove_at(hdr, slot, {str_key}, vs, out);
}}

void tuo_rt_map_str_keys(const long long *hdr, unsigned long long vs, long long *out_hdr) {{
    tuo_map_keys(hdr, {str_key}, vs, out_hdr);
}}

void tuo_rt_map_drop(long long *hdr, long long stride) {{
    uint64_t cap = (uint64_t)hdr[2];
    if (cap == 0) return;
    unsigned char *entries = (unsigned char *)hdr[0];
    tuo_rt_dealloc(tuo_map_index(entries, cap),
                   tuo_map_block_size(tuo_map_index_cap(entries), cap, (uint64_t)stride), 8);
}}
"#,
        sentinel = sentinel,
        int_key = INT_KEY_SIZE,
        str_key = STR_KEY_SIZE,
        initial_cap = INITIAL_CAPACITY,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        INITIAL_CAPACITY, INT_KEY_SIZE, MAP_DROP_SYMBOL, MAP_INT_GET_SYMBOL, MAP_INT_INSERT_SYMBOL,
        MAP_INT_KEYS_SYMBOL, MAP_INT_REMOVE_SYMBOL, MAP_STR_GET_SYMBOL, MAP_STR_INSERT_SYMBOL,
        MAP_STR_KEYS_SYMBOL, MAP_STR_REMOVE_SYMBOL, STR_KEY_SIZE, hash_int, hash_str,
        map_runtime_c_source,
    };

    #[test]
    fn the_int_hash_matches_the_splitmix64_finalizer_vectors() {
        // Published splitmix64-finalizer values; the C source carries the
        // identical constants, pinned below.
        assert_eq!(hash_int(0), 0x0000_0000_0000_0000);
        assert_eq!(hash_int(1), 0x5692_161d_100b_05e5);
        assert_eq!(hash_int(42), 0xa759_ea27_d472_7622);
        assert_eq!(hash_int(i64::MIN), 0x25c2_6ea5_79ce_a98a);
        assert_eq!(hash_int(-1), 0xb4d0_55fc_f2cb_bd7b);
    }

    #[test]
    fn the_str_hash_matches_the_fnv1a_vectors() {
        // The canonical FNV-1a 64 vectors (offset basis for "", the
        // published value for "a") plus a longer probe.
        assert_eq!(hash_str(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash_str(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash_str(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(hash_str(b"transport"), 0xf939_3d95_a0b4_81f2);
    }

    #[test]
    fn the_c_source_defines_every_map_symbol_with_the_same_constants() {
        let source = map_runtime_c_source();
        for symbol in [
            MAP_INT_INSERT_SYMBOL,
            MAP_INT_GET_SYMBOL,
            MAP_INT_REMOVE_SYMBOL,
            MAP_INT_KEYS_SYMBOL,
            MAP_STR_INSERT_SYMBOL,
            MAP_STR_GET_SYMBOL,
            MAP_STR_REMOVE_SYMBOL,
            MAP_STR_KEYS_SYMBOL,
            MAP_DROP_SYMBOL,
        ] {
            assert!(
                source.contains(&format!("void {symbol}(")),
                "the map C source must define `{symbol}`"
            );
        }
        // The hash constants match the Rust policy's vector-pinned functions.
        assert!(source.contains("0xbf58476d1ce4e5b9ULL"));
        assert!(source.contains("0x94d049bb133111ebULL"));
        assert!(source.contains("0xcbf29ce484222325ULL"));
        assert!(source.contains("0x100000001b3ULL"));
        // The layout constants match. Since ADR-0023 Stage B2 the entry stride
        // is *computed* (`key_size + vs`) rather than a literal, so what is
        // pinned is the key size each kind adds to the caller's value stride.
        assert!(source.contains(&format!("uint64_t stride = {INT_KEY_SIZE} + vs;")));
        assert!(source.contains(&format!("uint64_t stride = {STR_KEY_SIZE} + vs;")));
        assert!(source.contains(&format!(
            "tuo_map_rehome(hdr, {INITIAL_CAPACITY}, stride, is_str)"
        )));
        assert!(source.contains("2 * len > cap ? cap * 2 : cap"));
        // Every value-touching entry point takes the stride as a parameter, so
        // the shim stays type-agnostic. A hardcoded 8-byte value copy would be
        // the Stage B regression this pins against.
        assert!(source.contains("unsigned long long vs"));
        assert!(source.contains("tuo_map_copy(&out[1]"));
        // ...and the copies take a fixed-size path for the common sizes
        // rather than a libc call per operation.
        assert!(source.contains("case 8: d[0] = s[0]; return;"));
        // Allocation flows through the existing boundary only.
        assert!(source.contains("tuo_rt_alloc"));
        assert!(source.contains("tuo_rt_dealloc"));
        assert!(!source.contains("malloc"));
    }
}
