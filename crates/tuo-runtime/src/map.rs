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
//! block:  [ u64 index[index_cap] ][ u64 index_cap ][ entry entries[cap] ]
//!                                                    ^ header.ptr
//! ```
//!
//! The program-visible header is the same three words as `String`/`Array` —
//! `{ptr, len, cap}` — where `ptr` points at the **dense entries** region,
//! `len` is the live entry count, and `cap` the entry capacity. The hash
//! index (open addressing, linear probing, `index_cap == 2 × cap`, slots
//! holding `dense_index + 1` or `0` for empty) and its length word sit
//! *before* the entries in the same block, so the shim can recover the block
//! start from `ptr` alone (`index_cap` is at `ptr[-1]`) and the header never
//! carries a fourth word. An empty map is the sentinel header
//! `{ZERO_SIZE_SENTINEL, 0, 0}` with no allocation at all.
//!
//! Entry strides: `Map[Int, Int]` entries are `{i64 key, i64 value}`
//! ([`INT_ENTRY_STRIDE`]); `Map[Str, Int]` entries are
//! `{const u8 *key_ptr, u64 key_len, i64 value}` ([`STR_ENTRY_STRIDE`]) —
//! the borrowed `Str` key is stored as its two-word view, never copied.
//!
//! What lives here in Rust is the part that is pure and testable without a C
//! compiler: the two hash functions ([`hash_int`], [`hash_str`]) with their
//! published test vectors, the layout constants, and the symbol names. The
//! module's tests pin the C source to carry the same constants.

/// The C-ABI symbol for `Map[Int, Int]` insert:
/// `void tuo_rt_map_int_insert(long long *hdr, long long k, long long v,
/// long long *out)` — `out[0]` is 1 when the key was present (its previous
/// value in `out[1]`), else 0.
pub const MAP_INT_INSERT_SYMBOL: &str = "tuo_rt_map_int_insert";

/// The C-ABI symbol for `Map[Int, Int]` lookup:
/// `void tuo_rt_map_int_get(const long long *hdr, long long k,
/// long long *out)` — `out[0]`/`out[1]` as for insert.
pub const MAP_INT_GET_SYMBOL: &str = "tuo_rt_map_int_get";

/// The C-ABI symbol for `Map[Int, Int]` removal:
/// `void tuo_rt_map_int_remove(long long *hdr, long long k,
/// long long *out)` — `out[0]`/`out[1]` as for insert; the remaining
/// entries keep their relative order.
pub const MAP_INT_REMOVE_SYMBOL: &str = "tuo_rt_map_int_remove";

/// The C-ABI symbol for `Map[Int, Int]` key listing:
/// `void tuo_rt_map_int_keys(const long long *hdr, long long *out_hdr)` —
/// writes a fresh `Array[Int]` header (`{ptr, len, cap}`) of the keys in
/// insertion order.
pub const MAP_INT_KEYS_SYMBOL: &str = "tuo_rt_map_int_keys";

/// The C-ABI symbol for `Map[Str, Int]` insert (key as `{ptr, len}`).
pub const MAP_STR_INSERT_SYMBOL: &str = "tuo_rt_map_str_insert";

/// The C-ABI symbol for `Map[Str, Int]` lookup (key as `{ptr, len}`).
pub const MAP_STR_GET_SYMBOL: &str = "tuo_rt_map_str_get";

/// The C-ABI symbol for `Map[Str, Int]` removal (key as `{ptr, len}`).
pub const MAP_STR_REMOVE_SYMBOL: &str = "tuo_rt_map_str_remove";

/// The C-ABI symbol for `Map[Str, Int]` key listing: writes a fresh
/// `Array[Str]` header whose elements are the stored two-word views, in
/// insertion order.
pub const MAP_STR_KEYS_SYMBOL: &str = "tuo_rt_map_str_keys";

/// The C-ABI symbol that frees a map's block:
/// `void tuo_rt_map_drop(long long *hdr, long long stride)` — `stride` is
/// the entry stride ([`INT_ENTRY_STRIDE`]/[`STR_ENTRY_STRIDE`]), which the
/// drop needs only to compute the block size handed back to `tuo_rt_dealloc`.
pub const MAP_DROP_SYMBOL: &str = "tuo_rt_map_drop";

/// The byte stride of one `Map[Int, Int]` entry: `{i64 key, i64 value}`.
pub const INT_ENTRY_STRIDE: u64 = 16;

/// The byte stride of one `Map[Str, Int]` entry:
/// `{const u8 *key_ptr, u64 key_len, i64 value}`.
pub const STR_ENTRY_STRIDE: u64 = 24;

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
/// [`INITIAL_CAPACITY`], removal by shifting the dense tail down one slot and
/// rebuilding the index (O(n), preserving order — the documented v0
/// contract), and the [`hash_int`]/[`hash_str`] functions with the same
/// constants pinned by this module's tests.
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
/* Block: [ u64 index[index_cap] ][ u64 index_cap ][ entries[cap] ].   */
/* Index slot: 0 = empty, else dense_index + 1. An empty map is the    */
/* sentinel header {{{sentinel}, 0, 0}} with no allocation.            */

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

static uint64_t *tuo_map_index(unsigned char *entries) {{
    uint64_t index_cap = ((uint64_t *)entries)[-1];
    return (uint64_t *)(entries - 8 - 8 * index_cap);
}}

static uint64_t tuo_map_index_cap(const unsigned char *entries) {{
    return ((const uint64_t *)entries)[-1];
}}

static size_t tuo_map_block_size(uint64_t index_cap, uint64_t cap, uint64_t stride) {{
    return (size_t)(8 * index_cap + 8 + cap * stride);
}}

/* Rebuild the whole index from the dense entries (after removal or    */
/* growth). `hash_of(entries + i * stride)` is inlined per key kind    */
/* via the two concrete rebuild functions below.                       */
static void tuo_map_index_clear(uint64_t *index, uint64_t index_cap) {{
    memset(index, 0, (size_t)(8 * index_cap));
}}

static void tuo_map_index_place(uint64_t *index, uint64_t index_cap,
                                uint64_t hash, uint64_t dense) {{
    uint64_t mask = index_cap - 1;
    uint64_t slot = hash & mask;
    while (index[slot] != 0) slot = (slot + 1) & mask;
    index[slot] = dense + 1;
}}

static void tuo_map_rebuild_int(unsigned char *entries, uint64_t len) {{
    uint64_t index_cap = tuo_map_index_cap(entries);
    uint64_t *index = tuo_map_index(entries);
    tuo_map_index_clear(index, index_cap);
    for (uint64_t i = 0; i < len; i++) {{
        int64_t key;
        memcpy(&key, entries + i * {int_stride}, 8);
        tuo_map_index_place(index, index_cap, tuo_map_hash_int(key), i);
    }}
}}

static void tuo_map_rebuild_str(unsigned char *entries, uint64_t len) {{
    uint64_t index_cap = tuo_map_index_cap(entries);
    uint64_t *index = tuo_map_index(entries);
    tuo_map_index_clear(index, index_cap);
    for (uint64_t i = 0; i < len; i++) {{
        const unsigned char *kp;
        uint64_t kn;
        memcpy(&kp, entries + i * {str_stride}, 8);
        memcpy(&kn, entries + i * {str_stride} + 8, 8);
        tuo_map_index_place(index, index_cap, tuo_map_hash_str(kp, kn), i);
    }}
}}

/* Grow (or first-allocate) to `new_cap` entries, copying the dense     */
/* region and rebuilding the index. Returns the new entries pointer.    */
static unsigned char *tuo_map_grow(long long *hdr, uint64_t new_cap,
                                   uint64_t stride, int is_str) {{
    uint64_t len = (uint64_t)hdr[1];
    uint64_t old_cap = (uint64_t)hdr[2];
    uint64_t new_index_cap = 2 * new_cap;
    unsigned char *block =
        (unsigned char *)tuo_rt_alloc(tuo_map_block_size(new_index_cap, new_cap, stride), 8);
    unsigned char *entries = block + 8 * new_index_cap + 8;
    ((uint64_t *)entries)[-1] = new_index_cap;
    if (old_cap != 0) {{
        unsigned char *old_entries = (unsigned char *)hdr[0];
        memcpy(entries, old_entries, (size_t)(len * stride));
        uint64_t old_index_cap = tuo_map_index_cap(old_entries);
        tuo_rt_dealloc(tuo_map_index(old_entries),
                       tuo_map_block_size(old_index_cap, old_cap, stride), 8);
    }}
    if (is_str) tuo_map_rebuild_str(entries, len);
    else tuo_map_rebuild_int(entries, len);
    hdr[0] = (long long)entries;
    hdr[2] = (long long)new_cap;
    return entries;
}}

/* Probe for an int key. Returns the dense index or (uint64_t)-1.       */
static uint64_t tuo_map_find_int(const long long *hdr, int64_t key) {{
    if (hdr[2] == 0) return (uint64_t)-1;
    const unsigned char *entries = (const unsigned char *)hdr[0];
    uint64_t index_cap = tuo_map_index_cap(entries);
    const uint64_t *index = (const uint64_t *)(entries - 8 - 8 * index_cap);
    uint64_t mask = index_cap - 1;
    uint64_t slot = tuo_map_hash_int(key) & mask;
    while (index[slot] != 0) {{
        uint64_t dense = index[slot] - 1;
        int64_t stored;
        memcpy(&stored, entries + dense * {int_stride}, 8);
        if (stored == key) return dense;
        slot = (slot + 1) & mask;
    }}
    return (uint64_t)-1;
}}

static uint64_t tuo_map_find_str(const long long *hdr, const unsigned char *kp, uint64_t kn) {{
    if (hdr[2] == 0) return (uint64_t)-1;
    const unsigned char *entries = (const unsigned char *)hdr[0];
    uint64_t index_cap = tuo_map_index_cap(entries);
    const uint64_t *index = (const uint64_t *)(entries - 8 - 8 * index_cap);
    uint64_t mask = index_cap - 1;
    uint64_t slot = tuo_map_hash_str(kp, kn) & mask;
    while (index[slot] != 0) {{
        uint64_t dense = index[slot] - 1;
        const unsigned char *sp;
        uint64_t sn;
        memcpy(&sp, entries + dense * {str_stride}, 8);
        memcpy(&sn, entries + dense * {str_stride} + 8, 8);
        if (sn == kn && (kn == 0 || memcmp(sp, kp, (size_t)kn) == 0)) return dense;
        slot = (slot + 1) & mask;
    }}
    return (uint64_t)-1;
}}

void tuo_rt_map_int_get(const long long *hdr, long long k, long long *out) {{
    uint64_t dense = tuo_map_find_int(hdr, k);
    if (dense == (uint64_t)-1) {{ out[0] = 0; out[1] = 0; return; }}
    const unsigned char *entries = (const unsigned char *)hdr[0];
    out[0] = 1;
    memcpy(&out[1], entries + dense * {int_stride} + 8, 8);
}}

void tuo_rt_map_int_insert(long long *hdr, long long k, long long v, long long *out) {{
    uint64_t dense = tuo_map_find_int(hdr, k);
    if (dense != (uint64_t)-1) {{
        unsigned char *entries = (unsigned char *)hdr[0];
        memcpy(&out[1], entries + dense * {int_stride} + 8, 8);
        out[0] = 1;
        memcpy(entries + dense * {int_stride} + 8, &v, 8);
        return;
    }}
    out[0] = 0; out[1] = 0;
    uint64_t len = (uint64_t)hdr[1];
    uint64_t cap = (uint64_t)hdr[2];
    unsigned char *entries;
    if (len == cap) {{
        entries = tuo_map_grow(hdr, cap == 0 ? {initial_cap} : cap * 2, {int_stride}, 0);
    }} else {{
        entries = (unsigned char *)hdr[0];
    }}
    memcpy(entries + len * {int_stride}, &k, 8);
    memcpy(entries + len * {int_stride} + 8, &v, 8);
    tuo_map_index_place(tuo_map_index(entries), tuo_map_index_cap(entries),
                        tuo_map_hash_int(k), len);
    hdr[1] = (long long)(len + 1);
}}

void tuo_rt_map_int_remove(long long *hdr, long long k, long long *out) {{
    uint64_t dense = tuo_map_find_int(hdr, k);
    if (dense == (uint64_t)-1) {{ out[0] = 0; out[1] = 0; return; }}
    unsigned char *entries = (unsigned char *)hdr[0];
    uint64_t len = (uint64_t)hdr[1];
    out[0] = 1;
    memcpy(&out[1], entries + dense * {int_stride} + 8, 8);
    memmove(entries + dense * {int_stride}, entries + (dense + 1) * {int_stride},
            (size_t)((len - 1 - dense) * {int_stride}));
    hdr[1] = (long long)(len - 1);
    tuo_map_rebuild_int(entries, len - 1);
}}

void tuo_rt_map_int_keys(const long long *hdr, long long *out_hdr) {{
    uint64_t len = (uint64_t)hdr[1];
    if (len == 0) {{ out_hdr[0] = {sentinel}; out_hdr[1] = 0; out_hdr[2] = 0; return; }}
    const unsigned char *entries = (const unsigned char *)hdr[0];
    unsigned char *buf = (unsigned char *)tuo_rt_alloc((size_t)(8 * len), 8);
    for (uint64_t i = 0; i < len; i++) {{
        memcpy(buf + i * 8, entries + i * {int_stride}, 8);
    }}
    out_hdr[0] = (long long)buf;
    out_hdr[1] = (long long)len;
    out_hdr[2] = (long long)len;
}}

void tuo_rt_map_str_get(const long long *hdr, const unsigned char *kp, unsigned long long kn,
                        long long *out) {{
    uint64_t dense = tuo_map_find_str(hdr, kp, kn);
    if (dense == (uint64_t)-1) {{ out[0] = 0; out[1] = 0; return; }}
    const unsigned char *entries = (const unsigned char *)hdr[0];
    out[0] = 1;
    memcpy(&out[1], entries + dense * {str_stride} + 16, 8);
}}

void tuo_rt_map_str_insert(long long *hdr, const unsigned char *kp, unsigned long long kn,
                           long long v, long long *out) {{
    uint64_t dense = tuo_map_find_str(hdr, kp, kn);
    if (dense != (uint64_t)-1) {{
        unsigned char *entries = (unsigned char *)hdr[0];
        memcpy(&out[1], entries + dense * {str_stride} + 16, 8);
        out[0] = 1;
        memcpy(entries + dense * {str_stride} + 16, &v, 8);
        return;
    }}
    out[0] = 0; out[1] = 0;
    uint64_t len = (uint64_t)hdr[1];
    uint64_t cap = (uint64_t)hdr[2];
    unsigned char *entries;
    if (len == cap) {{
        entries = tuo_map_grow(hdr, cap == 0 ? {initial_cap} : cap * 2, {str_stride}, 1);
    }} else {{
        entries = (unsigned char *)hdr[0];
    }}
    memcpy(entries + len * {str_stride}, &kp, 8);
    memcpy(entries + len * {str_stride} + 8, &kn, 8);
    memcpy(entries + len * {str_stride} + 16, &v, 8);
    tuo_map_index_place(tuo_map_index(entries), tuo_map_index_cap(entries),
                        tuo_map_hash_str(kp, kn), len);
    hdr[1] = (long long)(len + 1);
}}

void tuo_rt_map_str_remove(long long *hdr, const unsigned char *kp, unsigned long long kn,
                           long long *out) {{
    uint64_t dense = tuo_map_find_str(hdr, kp, kn);
    if (dense == (uint64_t)-1) {{ out[0] = 0; out[1] = 0; return; }}
    unsigned char *entries = (unsigned char *)hdr[0];
    uint64_t len = (uint64_t)hdr[1];
    out[0] = 1;
    memcpy(&out[1], entries + dense * {str_stride} + 16, 8);
    memmove(entries + dense * {str_stride}, entries + (dense + 1) * {str_stride},
            (size_t)((len - 1 - dense) * {str_stride}));
    hdr[1] = (long long)(len - 1);
    tuo_map_rebuild_str(entries, len - 1);
}}

void tuo_rt_map_str_keys(const long long *hdr, long long *out_hdr) {{
    uint64_t len = (uint64_t)hdr[1];
    if (len == 0) {{ out_hdr[0] = {sentinel}; out_hdr[1] = 0; out_hdr[2] = 0; return; }}
    const unsigned char *entries = (const unsigned char *)hdr[0];
    unsigned char *buf = (unsigned char *)tuo_rt_alloc((size_t)(16 * len), 8);
    for (uint64_t i = 0; i < len; i++) {{
        memcpy(buf + i * 16, entries + i * {str_stride}, 16);
    }}
    out_hdr[0] = (long long)buf;
    out_hdr[1] = (long long)len;
    out_hdr[2] = (long long)len;
}}

void tuo_rt_map_drop(long long *hdr, long long stride) {{
    uint64_t cap = (uint64_t)hdr[2];
    if (cap == 0) return;
    unsigned char *entries = (unsigned char *)hdr[0];
    uint64_t index_cap = tuo_map_index_cap(entries);
    tuo_rt_dealloc(tuo_map_index(entries),
                   tuo_map_block_size(index_cap, cap, (uint64_t)stride), 8);
}}
"#,
        sentinel = sentinel,
        int_stride = INT_ENTRY_STRIDE,
        str_stride = STR_ENTRY_STRIDE,
        initial_cap = INITIAL_CAPACITY,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        INITIAL_CAPACITY, INT_ENTRY_STRIDE, MAP_DROP_SYMBOL, MAP_INT_GET_SYMBOL,
        MAP_INT_INSERT_SYMBOL, MAP_INT_KEYS_SYMBOL, MAP_INT_REMOVE_SYMBOL, MAP_STR_GET_SYMBOL,
        MAP_STR_INSERT_SYMBOL, MAP_STR_KEYS_SYMBOL, MAP_STR_REMOVE_SYMBOL, STR_ENTRY_STRIDE,
        hash_int, hash_str, map_runtime_c_source,
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
        // The layout constants match.
        assert!(source.contains(&format!("* {INT_ENTRY_STRIDE}")));
        assert!(source.contains(&format!("* {STR_ENTRY_STRIDE}")));
        assert!(source.contains(&format!("cap == 0 ? {INITIAL_CAPACITY} : cap * 2")));
        // Allocation flows through the existing boundary only.
        assert!(source.contains("tuo_rt_alloc"));
        assert!(source.contains("tuo_rt_dealloc"));
        assert!(!source.contains("malloc"));
    }
}
