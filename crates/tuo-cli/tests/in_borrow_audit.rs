//! The `in`-borrow audit (ADR-0034 invariant point 2): nothing writes
//! through an `in` borrow.
//!
//! For user code the ownership checker enforces this (`O0004`,
//! `specification/ownership.md` §7). A builtin's body is not user code: it is
//! lowered inline by the two backends or calls a C runtime shim. This file is
//! the audit of those builtins. [`AUDIT`] lists every builtin with an `in`
//! parameter and how both backends lower it: either inline (no runtime shim
//! receives the `in` argument) or through one or more shims, each named by its
//! `tuo_runtime` symbol constant together with the C parameter(s) that
//! receive the `in` data.
//!
//! The tests check the table against the builtin catalog
//! ([`Builtin::param_modes`]) and every listed shim parameter against the
//! runtime's C source: each must be a `const`-qualified pointer, so a shim
//! cannot write through the borrow without the C compiler rejecting it. The
//! table itself is matched to `lower.rs` in both backends by review; a builtin
//! that moves from inline lowering to a shim needs its entry updated.

use tuo_compiler::resolve::{Builtin, BuiltinParamMode};
use tuo_runtime::{effect, map};

/// A runtime shim that receives `in` data.
struct Shim {
    /// The shim's C symbol, from its `tuo_runtime` `*_SYMBOL` constant.
    symbol: &'static str,
    /// The C parameters that receive the `in` argument(s). For a split
    /// `{ptr, len}` pair only the `ptr` is listed: the `len` is a copy.
    in_params: &'static [&'static str],
}

/// How both backends lower a builtin with an `in` parameter.
enum Lowering {
    /// Every shim either backend calls for this builtin.
    Shims(&'static [Shim]),
    /// No shim, lowered inline by both backends.
    Inline,
}

/// The audit table: every builtin whose `param_modes()` has an `In`, exactly
/// once.
const AUDIT: &[(Builtin, Lowering)] = &[
    (
        Builtin::RtWrite,
        Lowering::Shims(&[Shim {
            symbol: effect::WRITE_SYMBOL,
            in_params: &["ptr"],
        }]),
    ),
    // `write_string` passes the borrowed header's `{ptr, len}` to the same
    // symbol as `write`.
    (
        Builtin::RtWriteString,
        Lowering::Shims(&[Shim {
            symbol: effect::WRITE_SYMBOL,
            in_params: &["ptr"],
        }]),
    ),
    (
        Builtin::RtParMap,
        Lowering::Shims(&[Shim {
            symbol: effect::PAR_MAP_SYMBOL,
            in_params: &["tasks"],
        }]),
    ),
    (
        Builtin::RtOpen,
        Lowering::Shims(&[Shim {
            symbol: effect::OPEN_SYMBOL,
            in_params: &["ptr"],
        }]),
    ),
    (
        Builtin::RtRemoveFile,
        Lowering::Shims(&[Shim {
            symbol: effect::REMOVE_FILE_SYMBOL,
            in_params: &["ptr"],
        }]),
    ),
    (
        Builtin::RtUdpSend,
        Lowering::Shims(&[Shim {
            symbol: effect::UDP_SEND_SYMBOL,
            in_params: &["hptr", "bptr"],
        }]),
    ),
    (
        Builtin::RtConnect,
        Lowering::Shims(&[Shim {
            symbol: effect::CONNECT_SYMBOL,
            in_params: &["ptr"],
        }]),
    ),
    (
        Builtin::RtConnectTimeout,
        Lowering::Shims(&[Shim {
            symbol: effect::CONNECT_TIMEOUT_SYMBOL,
            in_params: &["ptr"],
        }]),
    ),
    (Builtin::StrLen, Lowering::Inline),
    (Builtin::StrByteAt, Lowering::Inline),
    (Builtin::StrSlice, Lowering::Inline),
    (Builtin::StringFromStr, Lowering::Inline),
    (Builtin::StringAppend, Lowering::Inline),
    (Builtin::StringConcat, Lowering::Inline),
    (Builtin::StringLen, Lowering::Inline),
    (Builtin::StringByteAt, Lowering::Inline),
    (Builtin::StringSlice, Lowering::Inline),
    (Builtin::StringAsStr, Lowering::Inline),
    (Builtin::ArrayLen, Lowering::Inline),
    (Builtin::ArrayGet, Lowering::Inline),
    // `get` and `contains_key` both probe through the lookup shim; the key
    // type picks the int or str variant.
    (
        Builtin::MapGet,
        Lowering::Shims(&[
            Shim {
                symbol: map::MAP_INT_GET_SYMBOL,
                in_params: &["hdr"],
            },
            Shim {
                symbol: map::MAP_STR_GET_SYMBOL,
                in_params: &["hdr"],
            },
        ]),
    ),
    (
        Builtin::MapContainsKey,
        Lowering::Shims(&[
            Shim {
                symbol: map::MAP_INT_GET_SYMBOL,
                in_params: &["hdr"],
            },
            Shim {
                symbol: map::MAP_STR_GET_SYMBOL,
                in_params: &["hdr"],
            },
        ]),
    ),
    (Builtin::MapLen, Lowering::Inline),
    (
        Builtin::MapKeys,
        Lowering::Shims(&[
            Shim {
                symbol: map::MAP_INT_KEYS_SYMBOL,
                in_params: &["hdr"],
            },
            Shim {
                symbol: map::MAP_STR_KEYS_SYMBOL,
                in_params: &["hdr"],
            },
        ]),
    ),
    (Builtin::BoxGet, Lowering::Inline),
];

fn has_in_param(builtin: Builtin) -> bool {
    builtin.param_modes().contains(&BuiltinParamMode::In)
}

/// The catalog's disagreements with the audit table, one message each.
fn catalog_problems() -> Vec<String> {
    let mut problems = Vec::new();
    for builtin in Builtin::ALL {
        let entries = AUDIT.iter().filter(|(b, _)| *b == builtin).count();
        let name = builtin.name();
        match (has_in_param(builtin), entries) {
            (true, 0) => problems.push(format!(
                "builtin `{name}` has an `in` parameter but no entry in AUDIT: \
                 add it to the audit with the shim(s) both backends call, or \
                 `Lowering::Inline`"
            )),
            (false, n) if n > 0 => problems.push(format!(
                "builtin `{name}` has no `in` parameter but is listed in AUDIT: \
                 remove it from the audit"
            )),
            (_, n) if n > 1 => problems.push(format!(
                "builtin `{name}` is listed {n} times in AUDIT: remove the \
                 duplicate entries"
            )),
            _ => {}
        }
    }
    problems
}

#[test]
fn audit_table_matches_the_builtin_catalog() {
    let problems = catalog_problems();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// `source` with its comments blanked to spaces, keeping string and
/// character literals intact, so a signature quoted in a comment is never
/// mistaken for a definition.
fn strip_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &source[i..];
        if let Some(body) = rest.strip_prefix("/*") {
            let end = body.find("*/").map_or(rest.len(), |e| e + 4);
            out.extend(std::iter::repeat_n(' ', end));
            i += end;
        } else if rest.starts_with("//") {
            let end = rest.find('\n').unwrap_or(rest.len());
            out.extend(std::iter::repeat_n(' ', end));
            i += end;
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != quote {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(bytes.len());
            out.push_str(&source[i..end]);
            i = end;
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The parameters of every definition of `symbol` in `source`, each one with
/// its whitespace collapsed. A definition is the symbol followed by a
/// parameter list and then a `{` body; a declaration (`extern ...;`) or a
/// mention in a comment is not one.
fn definitions(source: &str, symbol: &str) -> Vec<Vec<String>> {
    let code = strip_comments(source);
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    for (start, _) in code.match_indices(symbol) {
        let end = start + symbol.len();
        if start > 0 && is_ident_byte(bytes[start - 1]) {
            continue;
        }
        let after = code[end..].trim_start();
        if !after.starts_with('(') {
            continue;
        }
        let open = code.len() - after.len();
        let mut depth = 0usize;
        let mut close = None;
        for (offset, b) in bytes[open..].iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        if !code[close + 1..].trim_start().starts_with('{') {
            continue;
        }
        let mut params = Vec::new();
        let mut depth = 0usize;
        let mut current = String::new();
        for c in code[open + 1..close].chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    params.push(std::mem::take(&mut current));
                    continue;
                }
                _ => {}
            }
            current.push(c);
        }
        params.push(current);
        found.push(
            params
                .iter()
                .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect(),
        );
    }
    found
}

/// The C parameter's name: its last identifier.
fn param_name(param: &str) -> &str {
    param
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .next()
        .unwrap_or_default()
}

/// Whether `param` is a single-level pointer to `const` data, such as
/// `const unsigned char *ptr` or `const long long *hdr`.
fn is_const_pointer(param: &str) -> bool {
    let Some((pointee, declarator)) = param.split_once('*') else {
        return false;
    };
    pointee.split_whitespace().any(|token| token == "const")
        && declarator.trim().bytes().all(is_ident_byte)
}

/// Check that the C definition of `symbol` in `source` takes each of
/// `in_params` as a `const`-qualified pointer, naming `builtin` in each
/// problem.
fn shim_problems(source: &str, builtin: &str, symbol: &str, in_params: &[&str]) -> Vec<String> {
    let defs = definitions(source, symbol);
    let params = match defs.as_slice() {
        [params] => params,
        [] => {
            return vec![format!(
                "builtin `{builtin}`: shim `{symbol}` has no definition in the \
                 runtime C source"
            )];
        }
        _ => {
            return vec![format!(
                "builtin `{builtin}`: shim `{symbol}` is defined {} times in the \
                 runtime C source",
                defs.len()
            )];
        }
    };
    let mut problems = Vec::new();
    for &wanted in in_params {
        match params.iter().find(|p| param_name(p) == wanted) {
            None => problems.push(format!(
                "builtin `{builtin}`: shim `{symbol}` has no parameter `{wanted}` \
                 (its parameters are `{}`)",
                params.join(", ")
            )),
            Some(param) if !is_const_pointer(param) => problems.push(format!(
                "builtin `{builtin}`: shim `{symbol}` takes its `in` data in \
                 parameter `{wanted}` as `{param}`, which is not a `const`-qualified \
                 pointer; make it `const` if the shim only reads it"
            )),
            Some(_) => {}
        }
    }
    problems
}

#[test]
fn every_audited_shim_takes_its_in_data_as_a_const_pointer() {
    let source = format!(
        "{}\n{}",
        effect::effect_runtime_c_source(),
        map::map_runtime_c_source()
    );
    let mut problems = Vec::new();
    for (builtin, lowering) in AUDIT {
        if let Lowering::Shims(shims) = lowering {
            assert!(
                !shims.is_empty(),
                "builtin `{}` lists no shims: use `Lowering::Inline`",
                builtin.name()
            );
            for shim in *shims {
                problems.extend(shim_problems(
                    &source,
                    builtin.name(),
                    shim.symbol,
                    shim.in_params,
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Sabotage: the checker rejects a shim that takes its `in` data as a
/// non-`const` pointer, and accepts the `const` version. The declarations in
/// a comment and on an `extern` line are `const`, so only the wrapped
/// definition decides.
#[test]
fn checker_rejects_a_non_const_in_pointer() {
    let snippet = |qualifier: &str| {
        format!(
            "/* long long tuo_rt_fake(const unsigned char *ptr, unsigned long long len); */\n\
             extern long long tuo_rt_fake(const unsigned char *ptr, unsigned long long len);\n\
             long long tuo_rt_fake({qualifier}unsigned char *ptr,\n\
             \x20                     unsigned long long len) {{\n\
             \x20   return (long long)len;\n\
             }}\n"
        )
    };
    let rejected = shim_problems(&snippet(""), "fake", "tuo_rt_fake", &["ptr"]);
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert!(
        rejected[0].contains("tuo_rt_fake") && rejected[0].contains("`ptr`"),
        "{rejected:?}"
    );
    let accepted = shim_problems(&snippet("const "), "fake", "tuo_rt_fake", &["ptr"]);
    assert!(accepted.is_empty(), "{accepted:?}");
    let missing = shim_problems(&snippet("const "), "fake", "tuo_rt_fake", &["buf"]);
    assert_eq!(missing.len(), 1, "{missing:?}");
    let undefined = shim_problems(&snippet("const "), "fake", "tuo_rt_other", &["ptr"]);
    assert_eq!(undefined.len(), 1, "{undefined:?}");
}

/// The checker does not accept a `const` pointer whose pointee is mutable or
/// a by-value scalar as `const` data.
#[test]
fn checker_rejects_const_pointers_to_mutable_data() {
    assert!(is_const_pointer("const unsigned char *ptr"));
    assert!(is_const_pointer("const long long *hdr"));
    assert!(!is_const_pointer("unsigned char *ptr"));
    assert!(!is_const_pointer("unsigned char *const ptr"));
    assert!(!is_const_pointer("const unsigned char **ptr"));
    assert!(!is_const_pointer("const long long len"));
}
