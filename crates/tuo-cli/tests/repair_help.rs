//! The help a diagnostic gives is the fix a model makes next, so the help for
//! the mistakes models actually make is pinned here, through the real binary.
//!
//! Each case was found by probing the compiler with the habit in question
//! (the LLM-bridge roadmap's gap 3); before it, the help either pointed at a
//! fix that changes every caller, or at nothing.

use std::process::Command;

fn help_for(name: &str, source: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tuo-repair-help-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("{name}.tuo"));
    std::fs::write(&path, source).expect("write the program");
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(["check", path.to_str().expect("UTF-8 path")])
        .output()
        .expect("the `tuo` binary runs");
    assert!(!output.status.success(), "{name} must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    stderr
}

#[test]
fn a_for_loop_over_a_borrowed_array_is_told_to_walk_its_indices() {
    // "Declare the parameter `take xs`" would make a read-only loop consume
    // its caller's array — the wrong fix, and the one models took.
    let stderr = help_for(
        "for_over_in",
        "module app;\n\nfn total(in xs: Array[Int]) -> Int {\n    var t = 0;\n    for x in xs {\n        t = t + x;\n    }\n    t\n}\n",
    );
    assert!(stderr.contains("error[O0003]"), "{stderr}");
    assert!(
        stderr.contains("loop over its indices") && stderr.contains("std::array::get(xs, i)"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("declare the parameter `take xs`"),
        "{stderr}"
    );
}

#[test]
fn an_owning_element_read_through_an_index_points_at_get() {
    let stderr = help_for(
        "index_string",
        "module app;\n\nfn first(in names: Array[String]) -> String {\n    names[0]\n}\n",
    );
    assert!(stderr.contains("error[O0007]"), "{stderr}");
    assert!(stderr.contains("std::array::get(names, i)"), "{stderr}");
}

#[test]
fn an_effect_reached_from_a_spec_names_todays_effect_modules() {
    // The help listed only the four ADR-0006 `std::rt` primitives, which is
    // not how a program reaches an effect any more.
    let stderr = help_for(
        "effect_in_spec",
        "module app;\n\nfn greet() -> Int {\n    let _ = std::io::println(\"hi\");\n    0\n}\n\nspec greet {\n    then greet() == 0;\n}\n",
    );
    assert!(stderr.contains("error[R0007]"), "{stderr}");
    assert!(
        stderr.contains("`std::io`") && stderr.contains("pure function"),
        "{stderr}"
    );
}
