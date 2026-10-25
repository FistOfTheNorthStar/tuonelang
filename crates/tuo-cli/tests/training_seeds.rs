//! The training seeds, kept honest by the real compiler.
//!
//! `training/seeds/<concept>/<name>.*` holds the hand-written seeds the
//! fine-tuning generator (`training/generate.py`) builds its datasets from.
//! The generator refuses a seed that does not compile, but it only runs when
//! someone regenerates; this suite re-admits every seed on each `cargo test`,
//! so a language change that breaks one fails in the same PR. It drives the
//! real `tuo` binary the way the generator does:
//!
//! * the solution `<name>.tuo` is canonically formatted, `tuo check` accepts
//!   it, and `tuo spec` runs its specs green, skipping none (a skipped spec
//!   exits 0 but verified nothing);
//! * when `<name>.toml` declares `run_exit`, `tuo run` exits with exactly it;
//! * each mutant `<name>.mutantN.tuo` checks and **fails** a spec, since it
//!   becomes a spec-failure repair transcript and a mutant no spec catches
//!   would teach nothing; and
//! * no file in a concept directory belongs to no seed, mirroring the
//!   loader's refusal in `training/seeds.py`.
//!
//! The held-out eval seeds share the layout but live in a private checkout
//! outside this public repository, since a published eval task is no longer
//! held out. Here a held-out spec file `<name>.heldout.tuo` is therefore an
//! error; with `TUO_EVAL_DIR` naming that checkout, the suite also admits its
//! `seeds/`, where each seed's held-out specs, appended to its solution, must
//! pass and a mutant is an error instead (an eval seed emits no transcripts).
//! CI has no such checkout, so a language change that breaks an eval seed
//! surfaces when someone runs this locally or regenerates the datasets.
//!
//! The metadata's other keys (the task text, tier, difficulty) are the
//! generator's to validate; this suite reads only `run_exit`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn seeds_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../training/seeds")
}

/// The private eval checkout's seeds, when `TUO_EVAL_DIR` names one.
fn eval_seeds_root() -> Option<PathBuf> {
    std::env::var_os("TUO_EVAL_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("seeds"))
}

fn tuo(args: &[&str], file: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .arg(file)
        .output()
        .expect("the tuo binary runs")
}

fn report(output: &Output) -> String {
    format!(
        "status {:?}\n{}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Whether `tuo spec` skipped a spec it could not run: its summary line then
/// ends `; N skipped`, though the exit status is still 0.
fn skipped_a_spec(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
        .any(|line| line.contains(" passed, ") && line.ends_with(" skipped"))
}

/// `run_exit = N` from a seed's metadata, skipping the inside of multi-line
/// strings so a task's prose can never be mistaken for the key.
fn run_exit(metadata: &str) -> Option<i32> {
    let mut in_string = false;
    for line in metadata.lines() {
        if !in_string && let Some(value) = line.strip_prefix("run_exit = ") {
            return Some(value.trim().parse().expect("run_exit is an integer"));
        }
        if line.matches("\"\"\"").count() % 2 == 1 {
            in_string = !in_string;
        }
    }
    None
}

struct Seed {
    label: String,
    solution: PathBuf,
    heldout: Option<PathBuf>,
    mutants: Vec<PathBuf>,
    run_exit: Option<i32>,
}

/// Every seed under `root`, or the files that belong to none.
fn read_seeds(root: &Path) -> (Vec<Seed>, Vec<String>) {
    let mut seeds = Vec::new();
    let mut stray = Vec::new();
    let mut concepts: Vec<PathBuf> = fs::read_dir(root)
        .unwrap_or_else(|err| panic!("{} is readable: {err}", root.display()))
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.is_dir())
        .collect();
    concepts.sort();
    for dir in concepts {
        let mut files: BTreeSet<String> = fs::read_dir(&dir)
            .expect("readable concept directory")
            .map(|entry| entry.expect("readable entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        let names: Vec<String> = files
            .iter()
            .filter_map(|file| file.strip_suffix(".toml"))
            .map(str::to_owned)
            .collect();
        for name in names {
            let concept = dir.file_name().unwrap().to_string_lossy();
            let metadata = fs::read_to_string(dir.join(format!("{name}.toml"))).unwrap();
            files.remove(&format!("{name}.toml"));
            files.remove(&format!("{name}.tuo"));
            let heldout = files
                .remove(&format!("{name}.heldout.tuo"))
                .then(|| dir.join(format!("{name}.heldout.tuo")));
            let mut mutants = Vec::new();
            while files.remove(&format!("{name}.mutant{}.tuo", mutants.len() + 1)) {
                mutants.push(dir.join(format!("{name}.mutant{}.tuo", mutants.len() + 1)));
            }
            seeds.push(Seed {
                label: format!("{concept}/{name}"),
                solution: dir.join(format!("{name}.tuo")),
                heldout,
                mutants,
                run_exit: run_exit(&metadata),
            });
        }
        stray.extend(
            files
                .into_iter()
                .map(|file| dir.join(file).display().to_string()),
        );
    }
    (seeds, stray)
}

/// Every way `seed` fails its contract; empty when it is admitted. A
/// `held_out` seed belongs to the private eval set.
fn problems(seed: &Seed, held_out: bool, scratch: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    if !seed.solution.is_file() {
        return vec![format!("no solution file {}", seed.solution.display())];
    }
    if seed.heldout.is_some() && !held_out {
        problems.push(
            "a public seed has held-out specs, which are not held out once published: \
             move the seed to the private eval set, or the specs into the solution"
                .to_owned(),
        );
    }
    if !seed.mutants.is_empty() && held_out {
        problems.push("an eval seed has mutants, which nothing would use".to_owned());
    }
    let fmt = tuo(&["fmt", "--check"], &seed.solution);
    if !fmt.status.success() {
        problems.push(format!(
            "not canonically formatted (run `tuo fmt`)\n{}",
            report(&fmt)
        ));
    }
    let check = tuo(&["check"], &seed.solution);
    if !check.status.success() {
        problems.push(format!("tuo check rejects it\n{}", report(&check)));
        return problems;
    }
    let spec = tuo(&["spec"], &seed.solution);
    if !spec.status.success() {
        problems.push(format!("a spec fails\n{}", report(&spec)));
    } else if skipped_a_spec(&spec) {
        problems.push(format!("a spec is skipped\n{}", report(&spec)));
    }
    if let Some(expected) = seed.run_exit {
        let run = tuo(&["run"], &seed.solution);
        if run.status.code() != Some(expected) {
            problems.push(format!("tuo run should exit {expected}\n{}", report(&run)));
        }
    }
    if let Some(heldout) = &seed.heldout {
        let program = format!(
            "{}\n\n{}",
            fs::read_to_string(&seed.solution).unwrap().trim_end(),
            fs::read_to_string(heldout).unwrap()
        );
        let combined = scratch.join(format!("eval--{}.tuo", seed.label.replace('/', "--")));
        fs::write(&combined, program).unwrap();
        let spec = tuo(&["spec"], &combined);
        if !spec.status.success() {
            problems.push(format!("a held-out spec fails\n{}", report(&spec)));
        } else if skipped_a_spec(&spec) {
            problems.push(format!("a held-out spec is skipped\n{}", report(&spec)));
        }
    }
    for mutant in &seed.mutants {
        let name = mutant.file_name().unwrap().to_string_lossy();
        let check = tuo(&["check"], mutant);
        if !check.status.success() {
            problems.push(format!("{name} must check\n{}", report(&check)));
            continue;
        }
        let spec = tuo(&["spec"], mutant);
        if spec.status.code() != Some(1) || !report(&spec).contains("FAILED ") {
            problems.push(format!("{name} must fail a spec\n{}", report(&spec)));
        }
    }
    problems
}

/// Admit `seeds` in parallel, returning every failure.
fn admit(seeds: &[Seed], held_out: bool) -> Vec<String> {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("training_seeds");
    fs::create_dir_all(&scratch).unwrap();
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = seeds.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = seeds
            .chunks(chunk)
            .map(|batch| {
                let scratch = &scratch;
                scope.spawn(move || {
                    batch
                        .iter()
                        .flat_map(|seed| {
                            problems(seed, held_out, scratch)
                                .into_iter()
                                .map(|problem| format!("{}: {problem}", seed.label))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    })
}

#[test]
fn every_training_seed_is_admitted() {
    let (seeds, stray) = read_seeds(&seeds_root());
    assert!(stray.is_empty(), "files that belong to no seed: {stray:?}");
    // Guard against a silently emptied or misplaced seed tree.
    assert!(
        seeds.len() >= 130,
        "expected the seed library, found {} seeds",
        seeds.len()
    );
    let failures = admit(&seeds, false);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_private_eval_seed_is_admitted() {
    let Some(root) = eval_seeds_root() else {
        return;
    };
    let (seeds, stray) = read_seeds(&root);
    assert!(stray.is_empty(), "files that belong to no seed: {stray:?}");
    assert!(!seeds.is_empty(), "no eval seeds under {}", root.display());
    let failures = admit(&seeds, true);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn run_exit_is_read_outside_strings_only() {
    let metadata = "task = \"\"\"\nrun_exit = 9 is prose here\"\"\"\nrun_exit = 30\n";
    assert_eq!(run_exit(metadata), Some(30));
    assert_eq!(run_exit("task = 'pure'\n"), None);
}
