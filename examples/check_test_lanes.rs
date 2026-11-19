//! Check that CI executes the Cargo test coverage it claims.
//!
//! Cargo and libtest remain the only discovery and execution authorities.
//! This tool reads the executing `cargo test` steps of
//! `.github/workflows/ci.yml` — the selectors CI actually runs, not a copy of
//! them — and asks libtest, through `--list`, what each step selects on the
//! host platform. Matching, `--skip`, `--exact`, features and conditional
//! compilation are therefore libtest's and the compiler's own.
//!
//! It fails when:
//!
//! - a Cargo test target (lib, bin, example, integration test, bench, doc) is not
//!   selected by any executing Linux step — `--no-run` compilation and
//!   `--all-targets` without `--doc` do not count;
//! - a Linux-discovered runnable test or doctest is executed by no Linux step;
//! - an executing step selects no runnable test of a lib, integration-test,
//!   bench or doc target (bin and example harnesses may be deliberately empty);
//! - a positive filter selects no runnable test, including when every match
//!   is `--skip`ped or `#[ignore]`d;
//! - a `--skip` excludes nothing, so the name it once excluded has moved;
//! - a [`REQUIREMENTS`] suite is absent, or not entirely executed, on its
//!   platform;
//! - a workflow step, Cargo artifact receipt, libtest output or rustdoc block
//!   output cannot be interpreted exactly.
//!
//! Discovery is native and CI runs it on Linux. `--job <id>` restricts
//! discovery to the targets that job already built. Every step that selects
//! one of those targets is evaluated, so a target's coverage is complete
//! across lanes. A host whose workflow has no native executing lane fails
//! clearly.
//!
//! ```text
//! cargo run --profile test --all-features --locked --example check_test_lanes -- --job rust-contracts
//! cargo run --profile test --all-features --locked --example check_test_lanes # all targets on this Linux host
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Suites that must run in full in a profile on a platform, beyond FULL's
/// Linux completeness.
///
/// FULL on Linux is complete: every runnable test there must execute. FAST
/// entries name what every FAST run must execute (the capacity suite
/// excluded), whatever the change touches.
const REQUIREMENTS: &[Requirement] = &[
    // FAST, on every push touching Rust: the deterministic Agent Loop and
    // runtime contracts and the real-process App Server and CLI workflows.
    // libtest filters are substrings, so each names an unambiguous path.
    Requirement::new(Profile::Fast, Platform::Linux, "lib", "scripted_suites::"),
    Requirement::new(
        Profile::Fast,
        Platform::Linux,
        "lib",
        "runtime::conversation_runtime::",
    ),
    Requirement::new(Profile::Fast, Platform::Linux, "test process", ""),
    // FULL executes the whole capacity suite (Linux completeness covers it
    // too; this names it so an emptied or renamed suite fails).
    Requirement::new(Profile::Full, Platform::Linux, "lib", SCALE_SUITE),
    // Stage 3/4 modularized and hardened namespaces whose silent disappearance
    // or accidental skip must fail immediately.
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "lib",
        "tools::workspace_python::tests::",
    ),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "lib",
        "runtime::workflow::tests::",
    ),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "lib",
        "app_server::command::tests::",
    ),
    Requirement::new(Profile::Full, Platform::Linux, "test contracts", ""),
    Requirement::new(Profile::Full, Platform::Linux, "test provider", ""),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "test conformance",
        "agent_loop::runtime_benchmark::",
    ),
    // Explicit doctests (not covered by `--all-targets`) including paired
    // positive API examples and negative compile/import `compile_fail` proofs.
    Requirement::new(Profile::Full, Platform::Linux, "doc", ""),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "doc",
        "context::status::AgentStatusSurfaceView",
    ),
    Requirement::new(Profile::Full, Platform::Linux, "doc", "ConversationState"),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "doc",
        "local_runtime::session_controller::SessionController",
    ),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "doc",
        "message::types::AgentStatusGenerationMetadata",
    ),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "doc",
        "message::types::CompactionSummaryMetadata",
    ),
    Requirement::new(
        Profile::Full,
        Platform::Linux,
        "doc",
        "tools::todo::ConversationTodoList",
    ),
];

/// The module name that marks a capacity test: its value comes from its scale
/// (a thousand turns, hundreds of messages, deep pagination), so it runs in
/// FULL, which must execute every one of them, and FAST requirements exclude
/// it. FAST lanes skip it by this name.
const SCALE_SUITE: &str = "::scale::";

/// Command prefixes that only measure or prepare the Cargo process they wrap.
const TRANSPARENT_WRAPPERS: &[&[&str]] = &[
    &["/usr/bin/time", "-l"],
    &["python3", "test-support/prepare_native_tests.py", "--"],
];

fn main() {
    let mut jobs = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match (argument.as_str(), arguments.next()) {
            ("--job", Some(job)) => jobs.push(job),
            _ => fail(&["usage: check_test_lanes [--job <workflow job id>]...".into()]),
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cargo = cargo_program();
    let host = match std::env::consts::OS {
        "linux" => Platform::Linux,
        "macos" => Platform::Macos,
        other => fail(&[format!("unsupported host platform {other}")]),
    };
    let metadata = metadata_targets(&cargo, root).unwrap_or_else(|error| fail(&[error]));
    let workflow = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))
        .unwrap_or_else(|error| fail(&[format!("cannot read the CI workflow: {error}")]));
    let steps = parse_workflow(&workflow, &metadata.targets).unwrap_or_else(|errors| fail(&errors));
    if !steps
        .iter()
        .any(|step| step.platform == host && step.executes)
    {
        fail(&[format!(
            "the workflow has no executing cargo test lane on {host}"
        )]);
    }
    let discovery = CargoDiscovery::from_metadata(cargo, root, metadata.clone());
    let report = check(
        &steps,
        &metadata.targets,
        &metadata.members,
        host,
        &jobs,
        REQUIREMENTS,
        &discovery,
    );
    for line in &report.summary {
        println!("{line}");
    }
    if !report.findings.is_empty() {
        fail(&report.findings);
    }
    println!("test lane coverage holds on {host}");
}

/// The Cargo that runs this process, so discovery uses the same toolchain.
fn cargo_program() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn fail(findings: &[String]) -> ! {
    for finding in findings {
        eprintln!("error: {finding}");
    }
    std::process::exit(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Platform {
    Linux,
    Macos,
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
        })
    }
}

/// The verification profile a job belongs to: FAST jobs give per-push
/// feedback, FULL jobs are the complete verification of an integration
/// candidate, `main` and the nightly run. A job declares it as
/// `env: VERIFICATION_PROFILE` (not a `RUST*` or `CARGO*` name, which the
/// Rust cache would hash into its key), and runs only when the workflow's
/// `plan` job selects it (`.github/scripts/verification.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Profile {
    Fast,
    Full,
}

impl fmt::Display for Profile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Fast => "FAST",
            Self::Full => "FULL",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Lib,
    Bin,
    Example,
    Test,
    Bench,
    Doc,
}

/// One Cargo target that `cargo test` can build as a libtest harness or run as doctests.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Target {
    kind: Kind,
    name: String,
}

impl Target {
    fn new(kind: Kind, name: &str) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }

    fn selector(&self) -> Vec<String> {
        let flag = match self.kind {
            Kind::Lib => return vec!["--lib".into()],
            Kind::Doc => return vec!["--doc".into()],
            Kind::Bin => "--bin",
            Kind::Example => "--example",
            Kind::Test => "--test",
            Kind::Bench => "--bench",
        };
        vec![flag.into(), self.name.clone()]
    }

    /// Binary and example harnesses may deliberately define no test; they
    /// stay selected so Cargo discovers a future one.
    fn may_be_empty(&self) -> bool {
        matches!(self.kind, Kind::Bin | Kind::Example)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Kind::Lib => formatter.write_str("lib"),
            Kind::Bin => write!(formatter, "bin {}", self.name),
            Kind::Example => write!(formatter, "example {}", self.name),
            Kind::Test => write!(formatter, "test {}", self.name),
            Kind::Bench => write!(formatter, "bench {}", self.name),
            Kind::Doc => formatter.write_str("doc"),
        }
    }
}

/// A suite that must execute entirely in `profile` on `platform`: every
/// runnable test of `target` matching the libtest `filter` (all of them when
/// empty).
#[derive(Clone, Copy, Debug)]
struct Requirement {
    profile: Profile,
    platform: Platform,
    target: &'static str,
    filter: &'static str,
}

impl Requirement {
    const fn new(
        profile: Profile,
        platform: Platform,
        target: &'static str,
        filter: &'static str,
    ) -> Self {
        Self {
            profile,
            platform,
            target,
            filter,
        }
    }

    fn describe(&self) -> String {
        if self.filter.is_empty() {
            format!("{} on {} {}", self.target, self.platform, self.profile)
        } else {
            format!(
                "{} `{}` on {} {}",
                self.target, self.filter, self.platform, self.profile
            )
        }
    }
}

/// Which packages a cargo test step's package selectors cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageScope {
    /// The root package (or explicit `-p rustx`) only.
    Root,
    /// `--workspace` present: every member not dropped by `--exclude`.
    Workspace,
}

/// One `cargo test` step of the workflow.
#[derive(Clone, Debug)]
struct Step {
    job: String,
    name: String,
    platform: Platform,
    profile: Profile,
    /// Selected by the plan for the change's affected components only
    /// (`fromJSON(needs.plan.outputs.steps)['<job>:<key>']`); it never
    /// satisfies a FAST requirement.
    affected: bool,
    /// `--no-run` compiles without executing; it assigns nothing.
    executes: bool,
    features: Vec<String>,
    targets: Vec<Target>,
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
    /// `--workspace` present: the step selects every member not in
    /// `member_excludes`.
    scope: PackageScope,
    /// Extracted (`rustx-*`) packages named by `-p` selectors.
    member_packages: Vec<String>,
    /// Packages dropped by `--exclude` selectors.
    member_excludes: Vec<String>,
    /// Target kinds the step selects for extracted packages; empty means the
    /// command selects every non-doc kind (a bare `-p`/`--workspace` step).
    member_kinds: BTreeSet<Kind>,
}

impl Step {
    /// Whether the step runs tests of extracted workspace members rather
    /// than the root package.
    fn member_step(&self) -> bool {
        !self.member_packages.is_empty()
            || (self.scope == PackageScope::Workspace
                && self.member_excludes.iter().any(|name| name == "rustx"))
    }
    fn label(&self) -> String {
        format!(
            "{} {} job `{}` step `{}`",
            self.platform, self.profile, self.job, self.name
        )
    }

    fn libtest(&self, filters: &[String], skips: &[String]) -> Vec<String> {
        let mut arguments: Vec<String> = filters.to_vec();
        for skip in skips {
            arguments.extend(["--skip".into(), skip.clone()]);
        }
        if self.exact {
            arguments.push("--exact".into());
        }
        arguments
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageMetadata {
    package_id: String,
    canonical_manifest: PathBuf,
    targets: Vec<Target>,
    sources: BTreeMap<Target, PathBuf>,
    /// Extracted workspace members (`rustx-*`): name → their target kinds
    /// (Doc excluded). A member target no lane selects fails the member
    /// coverage check.
    members: BTreeMap<String, BTreeSet<Kind>>,
}

fn emit_stderr(stderr: &str, emit: &mut dyn FnMut(&str)) {
    let trimmed = stderr.trim_end();
    if !trimmed.is_empty() {
        emit(trimmed);
    }
}

fn metadata_targets(cargo: &str, root: &Path) -> Result<PackageMetadata, String> {
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    emit_stderr(&String::from_utf8_lossy(&output.stderr), &mut |line| {
        eprintln!("{line}");
    });
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("malformed cargo metadata: {error}"))?;
    parse_metadata(&metadata)
}

/// Target kinds each extracted workspace member exposes; used to prove every
/// non-doc kind is selected by some executing step on every platform.
fn member_kinds(
    packages: &[serde_json::Value],
) -> Result<BTreeMap<String, BTreeSet<Kind>>, String> {
    let mut members = BTreeMap::new();
    for member in packages
        .iter()
        .filter(|package| package["name"].as_str().is_some_and(|name| name != "rustx"))
    {
        let name = member["name"].as_str().ok_or("a package has no name")?;
        let mut kinds = BTreeSet::new();
        for target in member["targets"]
            .as_array()
            .ok_or("a member package has no target list")?
        {
            let target_kinds: Vec<&str> = target["kind"]
                .as_array()
                .ok_or("a member target has no kind")?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            match target_kinds.as_slice() {
                ["bin"] => {
                    kinds.insert(Kind::Bin);
                }
                ["example"] => {
                    kinds.insert(Kind::Example);
                }
                ["test"] => {
                    kinds.insert(Kind::Test);
                }
                ["bench"] => {
                    kinds.insert(Kind::Bench);
                }
                ["custom-build"] => {}
                all_lib
                    if all_lib.iter().all(|kind| kind.ends_with("lib")) && !all_lib.is_empty() =>
                {
                    kinds.insert(Kind::Lib);
                }
                other => {
                    return Err(format!(
                        "member {name} target has unsupported kinds {other:?}"
                    ));
                }
            }
        }
        members.insert(name.to_owned(), kinds);
    }
    Ok(members)
}

fn parse_metadata(metadata: &serde_json::Value) -> Result<PackageMetadata, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;
    let package = if packages.len() == 1 {
        &packages[0]
    } else {
        // The workspace also hosts the extracted `rustx-*` crates; lanes only
        // cover the root package's targets.
        packages
            .iter()
            .find(|package| package["name"].as_str() == Some("rustx"))
            .ok_or("cargo metadata has no rustx package")?
    };
    let members = member_kinds(packages)?;
    let package_id = package["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("cargo metadata package has no id")?
        .to_owned();
    let manifest_path = package["manifest_path"]
        .as_str()
        .ok_or("cargo metadata package has no manifest_path")?;
    let canonical_manifest = std::fs::canonicalize(manifest_path).map_err(|error| {
        format!("cannot canonicalize package manifest `{manifest_path}`: {error}")
    })?;
    let mut targets = Vec::new();
    let mut sources = BTreeMap::new();
    for target in package["targets"]
        .as_array()
        .ok_or("cargo metadata has no target list")?
    {
        let name = target["name"].as_str().ok_or("a target has no name")?;
        let kinds: Vec<&str> = target["kind"]
            .as_array()
            .ok_or("a target has no kind")?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        let kind = match kinds.as_slice() {
            ["bin"] => Kind::Bin,
            ["example"] => Kind::Example,
            ["test"] => Kind::Test,
            ["bench"] => Kind::Bench,
            ["custom-build"] => continue,
            kinds if kinds.iter().all(|kind| kind.ends_with("lib")) && !kinds.is_empty() => {
                if target["doctest"].as_bool() != Some(false) {
                    targets.push(Target::new(Kind::Doc, name));
                }
                Kind::Lib
            }
            other => return Err(format!("target {name} has unsupported kinds {other:?}")),
        };
        let src_path = target["src_path"]
            .as_str()
            .ok_or_else(|| format!("target {name} has no src_path"))?;
        let canonical_src = std::fs::canonicalize(src_path).map_err(|error| {
            format!("cannot canonicalize source `{src_path}` for target {name}: {error}")
        })?;
        let classified = Target::new(kind, name);
        sources.insert(classified.clone(), canonical_src);
        targets.push(classified);
    }
    targets.sort();
    Ok(PackageMetadata {
        package_id,
        canonical_manifest,
        targets,
        sources,
        members,
    })
}

fn parse_workflow(text: &str, targets: &[Target]) -> Result<Vec<Step>, Vec<String>> {
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)
        .map_err(|error| vec![format!("malformed workflow: {error}")])?;
    let jobs = workflow["jobs"]
        .as_mapping()
        .ok_or_else(|| vec!["the workflow has no jobs".to_owned()])?;
    let mut steps = Vec::new();
    let mut errors = Vec::new();
    for (id, job) in jobs {
        let id = id.as_str().unwrap_or("<non-string job id>");
        for (index, step) in job["steps"].as_sequence().into_iter().flatten().enumerate() {
            let Some(run) = step["run"].as_str() else {
                continue;
            };
            let name = step["name"]
                .as_str()
                .map_or_else(|| format!("#{}", index + 1), str::to_owned);
            let located = |problem: String| format!("job `{id}` step `{name}`: {problem}");
            if !mentions_cargo_test(run) {
                continue;
            }
            let platform = match platform_of(job) {
                Ok(platform) => platform,
                Err(problem) => {
                    errors.push(located(problem));
                    continue;
                }
            };
            let profiles = match profiles_of(id, job) {
                Ok(profiles) => profiles,
                Err(problem) => {
                    errors.push(located(problem));
                    continue;
                }
            };
            let affected = match affected_step(id, &profiles, &step["if"]) {
                Ok(affected) => affected,
                Err(problem) => {
                    errors.push(located(problem));
                    continue;
                }
            };
            for (key, value) in [
                ("strategy", &job["strategy"]),
                ("continue-on-error", &job["continue-on-error"]),
                ("defaults", &job["defaults"]),
                ("continue-on-error", &step["continue-on-error"]),
                ("working-directory", &step["working-directory"]),
                ("shell", &step["shell"]),
            ] {
                if !value.is_null() {
                    errors.push(located(format!(
                        "`{key}` makes the cargo test step conditional or relocated; \
                         the lane check cannot interpret it"
                    )));
                }
            }
            match parse_command(run, targets) {
                // A job in both profiles contributes its steps to each.
                Ok(command) => steps.extend(profiles.iter().map(|profile| Step {
                    job: id.into(),
                    name: name.clone(),
                    platform,
                    profile: *profile,
                    affected,
                    ..command.clone()
                })),
                Err(problem) => errors.push(located(problem)),
            }
        }
    }
    if errors.is_empty() {
        Ok(steps)
    } else {
        Err(errors)
    }
}

fn platform_of(job: &serde_yaml_ng::Value) -> Result<Platform, String> {
    match job["runs-on"].as_str() {
        Some(runner) if runner.starts_with("ubuntu-") => Ok(Platform::Linux),
        Some(runner) if runner.starts_with("macos-") => Ok(Platform::Macos),
        _ => Err(format!("unsupported runs-on {:?}", job["runs-on"])),
    }
}

/// A test job's profiles (`VERIFICATION_PROFILE`: `fast`, `full`, or both), and the
/// one job condition the check understands: selection by the `plan` job's
/// `run` map under the job's own id. Any other condition would make the job's
/// selection something this check cannot see.
fn profiles_of(id: &str, job: &serde_yaml_ng::Value) -> Result<Vec<Profile>, String> {
    let gate = format!("fromJSON(needs.plan.outputs.run)['{id}']");
    if job["if"].as_str() != Some(gate.as_str()) {
        return Err(format!(
            "a cargo test job must be selected by exactly `if: {gate}`, found {:?}",
            job["if"]
        ));
    }
    let declared = job["env"]["VERIFICATION_PROFILE"]
        .as_str()
        .unwrap_or_default();
    let profiles: Option<Vec<Profile>> = declared
        .split_whitespace()
        .map(|profile| match profile {
            "fast" => Some(Profile::Fast),
            "full" => Some(Profile::Full),
            _ => None,
        })
        .collect();
    match profiles {
        Some(mut profiles) if !profiles.is_empty() => {
            profiles.sort();
            profiles.dedup();
            Ok(profiles)
        }
        _ => Err(format!(
            "a cargo test job must declare `env: VERIFICATION_PROFILE` as `fast`, `full` or \
             `fast full`, found {declared:?}"
        )),
    }
}

/// Whether a step runs only for affected components. The one step condition
/// understood is the plan's `steps` map under `<job>:<key>`, and only in a
/// FAST-only job: FULL never depends on what a change touches.
fn affected_step(
    job: &str,
    profiles: &[Profile],
    condition: &serde_yaml_ng::Value,
) -> Result<bool, String> {
    let Some(condition) = condition.as_str() else {
        return if condition.is_null() {
            Ok(false)
        } else {
            Err(format!("unsupported step `if` {condition:?}"))
        };
    };
    let prefix = format!("fromJSON(needs.plan.outputs.steps)['{job}:");
    let key = condition
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix("']"))
        .filter(|key| !key.is_empty() && !key.contains(['\'', ':']));
    match key {
        Some(_) if profiles == [Profile::Fast] => Ok(true),
        Some(_) => Err("`if` makes a FULL cargo test step conditional; FULL steps \
                        always run"
            .into()),
        None => Err(format!(
            "`if` makes the cargo test step conditional; the lane check understands only \
             `{prefix}<component>']`, found {condition:?}"
        )),
    }
}

fn mentions_cargo_test(run: &str) -> bool {
    run.lines().any(|line| {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        tokens
            .iter()
            .position(|token| *token == "cargo")
            .is_some_and(|cargo| match tokens.get(cargo + 1) {
                Some(&"test") => true,
                Some(first) if first.starts_with('-') || first.starts_with('+') => {
                    tokens[cargo + 1..].contains(&"test")
                }
                _ => false,
            })
    })
}

/// Parses one executing or compile-only `cargo test` command, strictly: any
/// shell construct or option whose selection meaning is not modeled here is
/// an error rather than a silently different reading.
fn parse_command(run: &str, targets: &[Target]) -> Result<Step, String> {
    let lines: Vec<&str> = run.lines().filter(|line| !line.trim().is_empty()).collect();
    let [line] = lines.as_slice() else {
        return Err("a cargo test step must be a single command line".into());
    };
    let mut tokens: Vec<&str> = line.split_whitespace().collect();
    if let Some(token) = tokens.iter().find(|token| {
        token.contains([
            '\'', '"', '$', '`', '|', '&', ';', '<', '>', '(', ')', '\\', '*', '?', '[', ']', '{',
            '}', '~', '#', '=',
        ])
    }) {
        return Err(format!("unsupported shell syntax in `{token}`"));
    }
    while let Some(wrapper) = TRANSPARENT_WRAPPERS
        .iter()
        .find(|wrapper| tokens.starts_with(wrapper))
    {
        tokens.drain(..wrapper.len());
    }
    let ["cargo", "test", arguments @ ..] = tokens.as_slice() else {
        return Err(format!("`{line}` is not a plain `cargo test` command"));
    };
    let (cargo, libtest) = match arguments.iter().position(|token| *token == "--") {
        Some(separator) => (&arguments[..separator], &arguments[separator + 1..]),
        None => (arguments, &[][..]),
    };
    let mut step = Step {
        job: String::new(),
        name: String::new(),
        platform: Platform::Linux,
        executes: true,
        features: Vec::new(),
        targets: Vec::new(),
        filters: Vec::new(),
        skips: Vec::new(),
        exact: false,
        scope: PackageScope::Root,
        member_packages: Vec::new(),
        member_excludes: Vec::new(),
        member_kinds: BTreeSet::new(),
        profile: Profile::Full,
        affected: false,
    };
    let (names_rustx, names_other) = parse_cargo_arguments(&mut step, cargo, targets)?;
    parse_libtest_arguments(&mut step, libtest)?;
    if names_other && names_rustx {
        // `rustx` and extracted packages selected together: per-package
        // feature and target resolution is not modeled, so reject instead
        // of silently dropping the other selectors.
        return Err(
            "a `cargo test` step mixing `rustx` with other `-p` packages is not modeled by the lanes"
                .into(),
        );
    }
    if names_rustx && step.member_excludes.iter().any(|name| name == "rustx") {
        return Err("a `cargo test` step cannot both select and exclude `rustx`".into());
    }
    if step.member_step() {
        // The step exercises extracted `rustx-*` packages, not the root.
        if !step.filters.is_empty() || !step.skips.is_empty() || step.exact {
            // The lanes model only the root package's tests, so a filter or
            // skip here is unverifiable and could silently run zero tests.
            return Err(
                "a member-package step must not carry libtest selections the lanes cannot verify"
                    .into(),
            );
        }
        // Kind selectors (`--lib` &c) resolved against the root package's
        // targets; they name kinds the member packages share, not root
        // coverage. Move them so the root lanes never claim them.
        if step.targets.is_empty() {
            // A bare `-p`/`--workspace --exclude rustx` step selects every
            // non-doc kind each member has.
            step.member_kinds.extend([
                Kind::Lib,
                Kind::Bin,
                Kind::Example,
                Kind::Test,
                Kind::Bench,
            ]);
        } else {
            step.member_kinds
                .extend(step.targets.iter().map(|target| target.kind));
            step.targets.clear();
        }
        return Ok(step);
    }
    if step.targets.is_empty() {
        return Err("a cargo test step must select its targets explicitly".into());
    }
    Ok(step)
}

fn parse_cargo_arguments(
    step: &mut Step,
    cargo: &[&str],
    targets: &[Target],
) -> Result<(bool, bool), String> {
    let of = |kind: Kind| targets.iter().filter(move |target| target.kind == kind);
    // `targets` holds only the root `rustx` package's targets; a `-p` step that
    // names other packages selects none of them and contributes no coverage.
    let (mut names_rustx, mut names_other) = (false, false);
    let mut cargo = cargo.iter();
    while let Some(argument) = cargo.next() {
        let mut named = |kind: Kind| -> Result<Target, String> {
            let name = cargo
                .next()
                .ok_or(format!("{argument} requires a target name"))?;
            of(kind)
                .find(|target| target.name == *name)
                .cloned()
                .ok_or(format!("{argument} {name} names no Cargo target"))
        };
        match *argument {
            "--lib" => step.targets.extend(of(Kind::Lib).cloned()),
            "--doc" => step.targets.extend(of(Kind::Doc).cloned()),
            "--bins" => step.targets.extend(of(Kind::Bin).cloned()),
            "--examples" => step.targets.extend(of(Kind::Example).cloned()),
            "--all-targets" => step.targets.extend(
                targets
                    .iter()
                    .filter(|target| target.kind != Kind::Doc)
                    .cloned(),
            ),
            "--bin" => step.targets.push(named(Kind::Bin)?),
            "--example" => step.targets.push(named(Kind::Example)?),
            "--test" => step.targets.push(named(Kind::Test)?),
            "--bench" => step.targets.push(named(Kind::Bench)?),
            // `--workspace` selects every member crate; it is forwarded
            // verbatim like a feature flag so doctest discovery covers the
            // extracted `rustx-*` crates too.
            "--workspace" => {
                step.scope = PackageScope::Workspace;
                step.features.push((*argument).into());
            }
            "--all-features" | "--no-default-features" => {
                step.features.push((*argument).into());
            }
            "--exclude" => {
                let name = cargo
                    .next()
                    .ok_or(format!("{argument} requires a package name"))?;
                step.member_excludes.push((*name).into());
            }
            "--features" | "-F" => {
                let features = cargo
                    .next()
                    .ok_or(format!("{argument} requires a feature list"))?;
                step.features
                    .extend([(*argument).into(), (*features).into()]);
            }
            "-p" | "--package" => {
                let name = cargo
                    .next()
                    .ok_or(format!("{argument} requires a package name"))?;
                if *name == "rustx" {
                    names_rustx = true;
                } else {
                    names_other = true;
                    step.member_packages.push((*name).into());
                }
            }
            "--no-run" => step.executes = false,
            "--locked" | "--timings" => {}
            other => return Err(format!("unsupported cargo test option `{other}`")),
        }
    }
    step.targets.sort();
    step.targets.dedup();
    Ok((names_rustx, names_other))
}

fn parse_libtest_arguments(step: &mut Step, libtest: &[&str]) -> Result<(), String> {
    let mut libtest = libtest.iter();
    while let Some(argument) = libtest.next() {
        match *argument {
            "--skip" => step
                .skips
                .push((*libtest.next().ok_or("--skip requires a filter")?).into()),
            "--exact" => step.exact = true,
            other if other.starts_with('-') => {
                return Err(format!("unsupported libtest option `{other}`"));
            }
            filter => step.filters.push(filter.into()),
        }
    }
    Ok(())
}

/// libtest's own view of one harness.
trait Discovery {
    /// The tests `cargo test <features> <target> -- <libtest> --list` prints;
    /// with `ignored`, only the `#[ignore]`d ones.
    fn list(
        &self,
        features: &[String],
        target: &Target,
        libtest: &[String],
        ignored: bool,
    ) -> Result<BTreeSet<String>, String>;

    /// Selected tests that an ordinary run executes: listed minus ignored.
    fn runnable(
        &self,
        features: &[String],
        target: &Target,
        libtest: &[String],
    ) -> Result<(BTreeSet<String>, usize), String> {
        let listed = self.list(features, target, libtest, false)?;
        let ignored = self.list(features, target, libtest, true)?;
        let count = ignored.len();
        Ok((listed.difference(&ignored).cloned().collect(), count))
    }
}

type ListKey = (Vec<String>, Target, Vec<String>, bool);
type HarnessKey = (Vec<String>, Target);

struct CargoDiscovery<'a> {
    cargo: String,
    root: &'a Path,
    metadata: PackageMetadata,
    #[cfg(test)]
    acquire_count: std::cell::Cell<usize>,
    harnesses: std::cell::RefCell<BTreeMap<HarnessKey, PathBuf>>,
    cache: std::cell::RefCell<BTreeMap<ListKey, BTreeSet<String>>>,
}

impl<'a> CargoDiscovery<'a> {
    #[cfg(test)]
    fn new(cargo: String, root: &'a Path) -> Result<Self, String> {
        let metadata = metadata_targets(&cargo, root)?;
        Ok(Self::from_metadata(cargo, root, metadata))
    }

    fn from_metadata(cargo: String, root: &'a Path, metadata: PackageMetadata) -> Self {
        Self {
            cargo,
            root,
            metadata,
            #[cfg(test)]
            acquire_count: std::cell::Cell::new(0),
            harnesses: std::cell::RefCell::default(),
            cache: std::cell::RefCell::default(),
        }
    }

    /// Acquires and validates the compiled libtest harness executable once per
    /// `(features, target)` via Cargo `--no-run --message-format=json`, keeping
    /// real compiler errors and metadata-bound artifact identity authoritative.
    fn acquire_harness(&self, features: &[String], target: &Target) -> Result<PathBuf, String> {
        let key = (features.to_vec(), target.clone());
        if let Some(executable) = self.harnesses.borrow().get(&key) {
            return Ok(executable.clone());
        }
        let expected_src = self
            .metadata
            .sources
            .get(target)
            .ok_or_else(|| format!("target {target} missing source path in cargo metadata"))?;
        #[cfg(test)]
        self.acquire_count.set(self.acquire_count.get() + 1);
        let mut command = Command::new(&self.cargo);
        command
            .args([
                "test",
                "--locked",
                "--color",
                "never",
                "--no-run",
                "--message-format",
                "json",
            ])
            .args(features)
            .args(target.selector());
        let rendered = format!("{command:?}");
        let output = command
            .current_dir(self.root)
            .output()
            .map_err(|error| format!("cannot run {rendered}: {error}"))?;
        let identity = ArtifactIdentity {
            package_id: &self.metadata.package_id,
            canonical_manifest: &self.metadata.canonical_manifest,
            canonical_src: expected_src,
            target,
        };
        let executable = parse_artifact_build(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
            &identity,
        )
        .map_err(|problem| format!("{rendered}: {problem}"))?;
        self.harnesses.borrow_mut().insert(key, executable.clone());
        Ok(executable)
    }

    fn list_doctests(
        &self,
        features: &[String],
        libtest: &[String],
        ignored: bool,
    ) -> Result<BTreeSet<String>, String> {
        let mut command = Command::new(&self.cargo);
        command
            .args(["test", "--doc", "--locked", "--color", "never"])
            .args(features)
            .arg("--")
            .args(libtest)
            .arg("--list");
        if ignored {
            command.arg("--ignored");
        }
        let rendered = format!("{command:?}");
        let output = command
            .current_dir(self.root)
            .output()
            .map_err(|error| format!("cannot run {rendered}: {error}"))?;
        let parsed = parse_doctest_list(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
        .map_err(|problem| format!("{rendered}: {problem}"))?;
        Ok(parsed.tests)
    }
}

impl Discovery for CargoDiscovery<'_> {
    fn list(
        &self,
        features: &[String],
        target: &Target,
        libtest: &[String],
        ignored: bool,
    ) -> Result<BTreeSet<String>, String> {
        let key = (features.to_vec(), target.clone(), libtest.to_vec(), ignored);
        if let Some(listed) = self.cache.borrow().get(&key) {
            return Ok(listed.clone());
        }
        // A single `[A-Za-z0-9_:]+` atom is purely a positional narrowing filter
        // (never an option, whitespace-split rustdoc arg, or NUL-containing
        // spawn failure). If a successful native global `--ignored` query for
        // the exact same `(features, target)` already returned empty, any
        // narrowed `--ignored` subset is empty too without spawning another
        // native listing.
        if ignored
            && let [filter] = libtest
            && !filter.is_empty()
            && filter
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':')
            && self
                .cache
                .borrow()
                .get(&(features.to_vec(), target.clone(), Vec::new(), true))
                .is_some_and(BTreeSet::is_empty)
        {
            return Ok(BTreeSet::new());
        }
        let listed = if target.kind == Kind::Doc {
            self.list_doctests(features, libtest, ignored)?
        } else {
            let executable = self.acquire_harness(features, target)?;
            let mut command = Command::new(&executable);
            command.args(libtest).args(["--list", "--format", "terse"]);
            if ignored {
                command.arg("--ignored");
            }
            let rendered = format!("{command:?}");
            let output = command
                .current_dir(self.root)
                .output()
                .map_err(|error| format!("cannot run {rendered}: {error}"))?;
            parse_libtest_terse(
                output.status.success(),
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            )
            .map_err(|problem| format!("{rendered}: {problem}"))?
        };
        self.cache.borrow_mut().insert(key, listed.clone());
        Ok(listed)
    }
}

fn target_kinds_match(kinds: &[&str], expected: Kind) -> bool {
    match expected {
        Kind::Bin => kinds == ["bin"],
        Kind::Example => kinds == ["example"],
        Kind::Test => kinds == ["test"],
        Kind::Bench => kinds == ["bench"],
        Kind::Lib => !kinds.is_empty() && kinds.iter().all(|kind| kind.ends_with("lib")),
        Kind::Doc => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactIdentity<'a> {
    package_id: &'a str,
    canonical_manifest: &'a Path,
    canonical_src: &'a Path,
    target: &'a Target,
}

/// Validates Cargo `--no-run --message-format=json` output and returns the one
/// compiled libtest harness executable matching `identity`.
fn parse_artifact_build(
    success: bool,
    stdout: &str,
    stderr: &str,
    identity: &ArtifactIdentity<'_>,
) -> Result<PathBuf, String> {
    parse_artifact_build_with(success, stdout, stderr, identity, &mut |line| {
        eprintln!("{line}");
    })
}

fn match_harness_artifact(
    line: &str,
    record: &serde_json::Value,
    identity: &ArtifactIdentity<'_>,
) -> Result<Option<PathBuf>, String> {
    if record["profile"]["test"].as_bool() != Some(true)
        || record["target"]["name"].as_str() != Some(identity.target.name.as_str())
    {
        return Ok(None);
    }
    let Some(kinds) = record["target"]["kind"].as_array().map(|items| {
        items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    }) else {
        return Ok(None);
    };
    if !target_kinds_match(&kinds, identity.target.kind) {
        return Ok(None);
    }
    let package_id = record["package_id"]
        .as_str()
        .ok_or_else(|| format!("matched compiler-artifact missing package_id: {line}"))?;
    if package_id != identity.package_id {
        return Ok(None);
    }
    let manifest = record["manifest_path"]
        .as_str()
        .ok_or_else(|| format!("matched compiler-artifact missing manifest_path: {line}"))?;
    let artifact_manifest = std::fs::canonicalize(manifest)
        .map_err(|error| format!("cannot canonicalize artifact manifest `{manifest}`: {error}"))?;
    if artifact_manifest != identity.canonical_manifest {
        return Ok(None);
    }
    let src_path = record["target"]["src_path"]
        .as_str()
        .ok_or_else(|| format!("matched compiler-artifact missing target.src_path: {line}"))?;
    let artifact_src = std::fs::canonicalize(src_path).map_err(|error| {
        format!("cannot canonicalize artifact target.src_path `{src_path}`: {error}")
    })?;
    if artifact_src != identity.canonical_src {
        return Err(format!(
            "matched compiler-artifact source {} does not match metadata source {} for {}",
            artifact_src.display(),
            identity.canonical_src.display(),
            identity.target
        ));
    }
    let executable = record["executable"].as_str().ok_or_else(|| {
        format!(
            "matched test harness artifact for {} has no executable path",
            identity.target
        )
    })?;
    let path = PathBuf::from(executable);
    if !path.is_file() {
        return Err(format!(
            "matched test harness executable for {} does not exist: {}",
            identity.target,
            path.display()
        ));
    }
    Ok(Some(path))
}

fn parse_artifact_build_with(
    success: bool,
    stdout: &str,
    stderr: &str,
    identity: &ArtifactIdentity<'_>,
    emit: &mut dyn FnMut(&str),
) -> Result<PathBuf, String> {
    emit_stderr(stderr, emit);
    let mut diagnostics = Vec::new();
    let mut executables = Vec::new();
    let mut build_finished = None;

    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let record: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("malformed cargo json record `{line}`: {error}"))?;
        match record["reason"].as_str() {
            Some("compiler-message") => {
                if let Some(rendered) = record["message"]["rendered"].as_str() {
                    let trimmed = rendered.trim_end();
                    if !trimmed.is_empty() {
                        emit(trimmed);
                        diagnostics.push(trimmed.to_owned());
                    }
                }
            }
            Some("build-finished") => {
                if build_finished.is_some() {
                    return Err("duplicate build-finished record in cargo json stream".into());
                }
                let finished = record["success"]
                    .as_bool()
                    .ok_or_else(|| format!("malformed build-finished record `{line}`"))?;
                build_finished = Some(finished);
            }
            Some("compiler-artifact") => {
                if let Some(path) = match_harness_artifact(line, &record, identity)? {
                    executables.push(path);
                }
            }
            _ => {}
        }
    }

    let diagnostic_context = || {
        let mut parts = Vec::new();
        if !diagnostics.is_empty() {
            parts.push(diagnostics.join("\n"));
        }
        if !stderr.trim().is_empty() {
            parts.push(stderr.trim_end().to_owned());
        }
        parts.join("\n")
    };

    if !success || build_finished != Some(true) {
        return Err(format!(
            "harness build failed (exit_ok={success}, build_finished={build_finished:?}):\n{}",
            diagnostic_context()
        ));
    }
    match executables.as_slice() {
        [executable] => Ok(executable.clone()),
        [] => Err(format!(
            "expected one compiled test harness artifact for {}, found none:\n{}",
            identity.target,
            diagnostic_context()
        )),
        many => Err(format!(
            "expected one compiled test harness artifact for {}, found {} ({many:?})",
            identity.target,
            many.len()
        )),
    }
}

/// Parses `--list --format terse` output emitted directly by one compiled
/// libtest executable.
fn parse_libtest_terse(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<BTreeSet<String>, String> {
    parse_libtest_terse_with(success, stdout, stderr, &mut |line| eprintln!("{line}"))
}

fn parse_libtest_terse_with(
    success: bool,
    stdout: &str,
    stderr: &str,
    emit: &mut dyn FnMut(&str),
) -> Result<BTreeSet<String>, String> {
    emit_stderr(stderr, emit);
    if !success {
        return Err(format!("discovery failed:\n{stderr}"));
    }
    let mut tests = BTreeSet::new();
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        if let Some(name) = line.strip_suffix(": test") {
            tests.insert(name.to_owned());
        } else if line.strip_suffix(": benchmark").is_none() {
            return Err(format!("unrecognized libtest list line `{line}`"));
        }
    }
    Ok(tests)
}

#[derive(Debug, PartialEq, Eq)]
struct DoctestDiscovery {
    tests: BTreeSet<String>,
    blocks: Vec<(usize, usize)>,
}

fn parse_summary_count(token: &str, singular: &str, plural: &str) -> Option<usize> {
    let (count, unit) = token.trim().split_once(' ')?;
    if unit == singular || unit == plural {
        count.parse::<usize>().ok()
    } else {
        None
    }
}

fn parse_doctest_block_summary(line: &str) -> Option<(usize, usize)> {
    let (left, right) = line.split_once(", ")?;
    let tests = parse_summary_count(left, "test", "tests")?;
    let benchmarks = parse_summary_count(right, "benchmark", "benchmarks")?;
    Some((tests, benchmarks))
}

/// Parses `cargo test --doc ... -- --list` stdout/stderr across all rustdoc
/// test blocks emitted on the stream, verifying every block summary and the
/// final timing line without assuming block counts label doctest kinds.
fn parse_doctest_list(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<DoctestDiscovery, String> {
    parse_doctest_list_with(success, stdout, stderr, &mut |line| eprintln!("{line}"))
}

fn parse_doctest_list_with(
    success: bool,
    stdout: &str,
    stderr: &str,
    emit: &mut dyn FnMut(&str),
) -> Result<DoctestDiscovery, String> {
    emit_stderr(stderr, emit);
    if !success {
        return Err(format!("doctest discovery failed:\n{stderr}"));
    }
    let running = stderr
        .lines()
        .filter(|line| line.trim_start().starts_with("Running "))
        .count();
    let doc_banners = stderr
        .lines()
        .filter(|line| line.trim_start().starts_with("Doc-tests "))
        .count();
    // `--workspace` emits one `Doc-tests <crate>` banner per member library;
    // crates without doctests still count.
    if running != 0 || doc_banners == 0 {
        return Err(format!(
            "expected Doc-tests banners and no unit harness in doctest stderr \
             (running={running}, doc_banners={doc_banners}):\n{stderr}"
        ));
    }

    let mut tests = BTreeSet::new();
    let mut blocks = Vec::new();
    let mut block_tests = 0usize;
    let mut block_benchmarks = 0usize;
    // One `all doctests ran in` line terminates each doctest-bearing crate's
    // block; member crates without doctests emit their `(0, 0)` summary with
    // no timing line, so later blocks legitimately follow a final line.
    let mut finals = 0usize;

    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        if let Some((summary_tests, summary_benchmarks)) = parse_doctest_block_summary(line) {
            if (summary_tests, summary_benchmarks) != (block_tests, block_benchmarks) {
                return Err(format!(
                    "doctest block summary mismatch: header reported ({summary_tests}, \
                     {summary_benchmarks}), parsed ({block_tests}, {block_benchmarks})"
                ));
            }
            blocks.push((summary_tests, summary_benchmarks));
            block_tests = 0;
            block_benchmarks = 0;
        } else if line.starts_with("all doctests ran in ") {
            if block_tests != 0 || block_benchmarks != 0 {
                return Err(format!(
                    "unterminated doctest block before final summary ({block_tests} tests, \
                     {block_benchmarks} benchmarks)"
                ));
            }
            finals += 1;
        } else if let Some(name) = line.strip_suffix(": test") {
            block_tests += 1;
            tests.insert(name.to_owned());
        } else if line.strip_suffix(": benchmark").is_some() {
            block_benchmarks += 1;
        } else {
            return Err(format!("unrecognized doctest list line `{line}`"));
        }
    }

    if block_tests != 0 || block_benchmarks != 0 || blocks.is_empty() || finals == 0 {
        return Err(format!(
            "incomplete doctest discovery output (blocks={blocks:?}, finals={finals})"
        ));
    }
    Ok(DoctestDiscovery { tests, blocks })
}

#[derive(Debug, Default)]
struct Report {
    summary: Vec<String>,
    findings: Vec<String>,
}

#[allow(clippy::too_many_lines)] // one linear pass over the documented rules
fn check(
    steps: &[Step],
    targets: &[Target],
    members: &BTreeMap<String, BTreeSet<Kind>>,
    host: Platform,
    jobs: &[String],
    requirements: &[Requirement],
    discovery: &dyn Discovery,
) -> Report {
    let mut report = Report::default();
    let executing: Vec<&Step> = steps.iter().filter(|step| step.executes).collect();

    // Assignment and requirement placement need no discovery; every host
    // checks them for the whole workflow. FULL is the complete profile: a
    // target only FAST selects is not assigned.
    for target in targets {
        if !executing.iter().any(|step| {
            step.platform == Platform::Linux
                && step.profile == Profile::Full
                && step.targets.contains(target)
        }) {
            report.findings.push(format!(
                "{target} is not selected by any executing FULL cargo test step on linux \
                 (compile-only --no-run steps and FAST steps do not count); assign it to a lane"
            ));
        }
    }
    for requirement in requirements {
        match targets
            .iter()
            .find(|target| target.to_string() == requirement.target)
        {
            None => report.findings.push(format!(
                "required suite {} names no Cargo target",
                requirement.describe()
            )),
            Some(target) => {
                if !executing.iter().any(|step| {
                    step.platform == requirement.platform
                        && step.profile == requirement.profile
                        && step.targets.contains(target)
                }) {
                    report.findings.push(format!(
                        "required suite {} is selected by no executing cargo test step",
                        requirement.describe()
                    ));
                }
            }
        }
    }
    // Extracted member coverage: on every platform where the workflow runs
    // cargo test at all, each workspace member's targets must be selected by
    // an executing member step. A new crate no lane selects fails closed.
    for step in &executing {
        for name in &step.member_packages {
            if !members.contains_key(name) {
                report.findings.push(format!(
                    "{} names workspace member `{name}` that cargo metadata does not list",
                    step.label()
                ));
            }
        }
    }
    let test_platforms: BTreeSet<Platform> = executing.iter().map(|step| step.platform).collect();
    for (member, kinds) in members {
        for platform in &test_platforms {
            let covered: BTreeSet<Kind> = executing
                .iter()
                .filter(|step| step.platform == *platform && step.member_step())
                .filter(|step| {
                    step.member_packages.iter().any(|name| name == member)
                        || (step.scope == PackageScope::Workspace
                            && !step.member_excludes.iter().any(|name| name == member))
                })
                .flat_map(|step| step.member_kinds.iter().copied())
                .collect();
            for kind in kinds {
                if !covered.contains(kind) {
                    report.findings.push(format!(
                        "workspace member `{member}` {kind:?} tests are selected by no executing \
                         member step on {platform}; assign them to a lane"
                    ));
                }
            }
        }
    }
    for job in jobs {
        match steps.iter().find(|step| step.job == *job) {
            None => report
                .findings
                .push(format!("job `{job}` has no cargo test step")),
            Some(step) if step.platform != host => report.findings.push(format!(
                "job `{job}` runs on {}; its discovery must run natively there, not on {host}",
                step.platform
            )),
            Some(_) => {}
        }
    }

    let focus: Vec<&Step> = executing
        .iter()
        .copied()
        .filter(|step| step.platform == host && (jobs.is_empty() || jobs.contains(&step.job)))
        .collect();
    // Each profile's coverage is its own: FAST never borrows FULL's steps.
    let evaluated: BTreeSet<(Profile, &Target)> = focus
        .iter()
        .flat_map(|step| step.targets.iter().map(|target| (step.profile, target)))
        .collect();

    for (profile, target) in evaluated {
        let selecting: Vec<&Step> = executing
            .iter()
            .copied()
            .filter(|step| {
                step.platform == host && step.profile == profile && step.targets.contains(target)
            })
            .collect();
        let mut covered: BTreeMap<&[String], BTreeSet<String>> = BTreeMap::new();
        // What every run of the profile executes, whatever the change touches.
        let mut unconditional: BTreeMap<&[String], BTreeSet<String>> = BTreeMap::new();
        for step in &selecting {
            let arguments = step.libtest(&step.filters, &step.skips);
            match discovery.runnable(&step.features, target, &arguments) {
                Ok((runnable, ignored)) => {
                    report.summary.push(format!(
                        "{}: {target}: {} runnable, {ignored} ignored selected",
                        step.label(),
                        runnable.len()
                    ));
                    if runnable.is_empty() && !target.may_be_empty() {
                        report.findings.push(format!(
                            "{} selects no runnable test of {target}",
                            step.label()
                        ));
                    }
                    if !step.affected {
                        unconditional
                            .entry(step.features.as_slice())
                            .or_default()
                            .extend(runnable.iter().cloned());
                    }
                    covered
                        .entry(step.features.as_slice())
                        .or_default()
                        .extend(runnable);
                }
                Err(problem) => report.findings.push(problem),
            }
        }
        for (features, covered) in &covered {
            if host == Platform::Linux && profile == Profile::Full {
                match discovery.runnable(features, target, &[]) {
                    Ok((universe, _)) => {
                        let missing: Vec<&String> = universe.difference(covered).collect();
                        if !missing.is_empty() {
                            report.findings.push(format!(
                                "{} runnable test(s) of {target} on linux are executed by no \
                                 FULL lane, e.g. {:?}",
                                missing.len(),
                                &missing[..missing.len().min(5)]
                            ));
                        }
                    }
                    Err(problem) => report.findings.push(problem),
                }
            }
            for requirement in requirements.iter().filter(|requirement| {
                requirement.platform == host
                    && requirement.profile == profile
                    && requirement.target == target.to_string()
            }) {
                let mut filters: Vec<String> = if requirement.filter.is_empty() {
                    Vec::new()
                } else {
                    vec![requirement.filter.to_owned()]
                };
                if requirement.profile == Profile::Fast {
                    filters.extend(["--skip".to_owned(), SCALE_SUITE.to_owned()]);
                }
                match discovery.runnable(features, target, &filters) {
                    Ok((required, _)) if required.is_empty() => report.findings.push(format!(
                        "required suite {} matches no runnable test; was it renamed?",
                        requirement.describe()
                    )),
                    Ok((required, _)) => {
                        let always = unconditional.get(features).cloned().unwrap_or_default();
                        let missing: Vec<&String> = required.difference(&always).collect();
                        if !missing.is_empty() {
                            report.findings.push(format!(
                                "required suite {}: {} runnable test(s) are executed by no \
                                 step, e.g. {:?}",
                                requirement.describe(),
                                missing.len(),
                                &missing[..missing.len().min(5)]
                            ));
                        }
                    }
                    Err(problem) => report.findings.push(problem),
                }
            }
        }
    }

    for step in focus {
        for filter in &step.filters {
            let only = std::slice::from_ref(filter);
            let selected = sum(discovery, step, &step.libtest(only, &step.skips));
            let unskipped = sum(discovery, step, &step.libtest(only, &[]));
            match (selected, unskipped) {
                (Ok((0, _)), Ok((0, 0))) => report.findings.push(format!(
                    "{}: filter `{filter}` matches no test; a stale selector",
                    step.label()
                )),
                (Ok((0, _)), Ok((0, _))) => report.findings.push(format!(
                    "{}: filter `{filter}` matches only #[ignore]d tests",
                    step.label()
                )),
                (Ok((0, _)), Ok(_)) => report.findings.push(format!(
                    "{}: every runnable match of filter `{filter}` is excluded by --skip",
                    step.label()
                )),
                (Err(problem), _) | (_, Err(problem)) => report.findings.push(problem),
                _ => {}
            }
        }
        for skip in &step.skips {
            let others: Vec<String> = step.skips.iter().filter(|s| *s != skip).cloned().collect();
            let mut excluded = 0;
            for target in &step.targets {
                let with = discovery.list(
                    &step.features,
                    target,
                    &step.libtest(&step.filters, &step.skips),
                    false,
                );
                let without = discovery.list(
                    &step.features,
                    target,
                    &step.libtest(&step.filters, &others),
                    false,
                );
                match (with, without) {
                    (Ok(with), Ok(without)) => excluded += without.difference(&with).count(),
                    (Err(problem), _) | (_, Err(problem)) => report.findings.push(problem),
                }
            }
            if excluded == 0 {
                report.findings.push(format!(
                    "{}: --skip `{skip}` excludes no test; a stale selector",
                    step.label()
                ));
            }
        }
    }
    report.findings.sort();
    report.findings.dedup();
    report
}

/// Runnable and ignored counts over every target of `step`.
fn sum(
    discovery: &dyn Discovery,
    step: &Step,
    libtest: &[String],
) -> Result<(usize, usize), String> {
    let mut totals = (0, 0);
    for target in &step.targets {
        let (runnable, ignored) = discovery.runnable(&step.features, target, libtest)?;
        totals.0 += runnable.len();
        totals.1 += ignored;
    }
    Ok(totals)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A libtest stand-in: substring (or `--exact`) filters, `--skip`, and
    /// `--ignored`, over a fixed per-target inventory of `(name, ignored)`.
    struct Fake(BTreeMap<Target, Vec<(&'static str, bool)>>);

    impl Discovery for Fake {
        fn list(
            &self,
            _features: &[String],
            target: &Target,
            libtest: &[String],
            ignored: bool,
        ) -> Result<BTreeSet<String>, String> {
            let exact = libtest.iter().any(|argument| argument == "--exact");
            let matches = |name: &str, filter: &str| {
                if exact {
                    name == filter
                } else {
                    name.contains(filter)
                }
            };
            let mut filters = Vec::new();
            let mut skips = Vec::new();
            let mut arguments = libtest.iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--skip" => skips.push(arguments.next().unwrap().as_str()),
                    "--exact" => {}
                    filter => filters.push(filter),
                }
            }
            Ok(self.0[target]
                .iter()
                .filter(|(name, is_ignored)| {
                    (!ignored || *is_ignored)
                        && (filters.is_empty() || filters.iter().any(|f| matches(name, f)))
                        && !skips.iter().any(|skip| matches(name, skip))
                })
                .map(|(name, _)| (*name).to_owned())
                .collect())
        }
    }

    fn targets() -> Vec<Target> {
        vec![
            Target::new(Kind::Lib, "rustx"),
            Target::new(Kind::Doc, "rustx"),
            Target::new(Kind::Bin, "rustx"),
            Target::new(Kind::Example, "tool"),
            Target::new(Kind::Test, "durable"),
            Target::new(Kind::Test, "contracts"),
        ]
    }

    fn inventory() -> Fake {
        Fake(BTreeMap::from([
            (
                Target::new(Kind::Lib, "rustx"),
                vec![
                    ("model::unit", false),
                    ("model::measure", true),
                    ("scripted_suites::agent::order", false),
                    ("boundary_suites::durable::kill", false),
                    ("manager::tests::residency", false),
                    ("model::scale::thousand_turns", false),
                ],
            ),
            (
                Target::new(Kind::Doc, "rustx"),
                vec![
                    ("src/lib.rs - positive::Api (line 10)", false),
                    ("src/lib.rs - negative::Api (line 20)", false),
                ],
            ),
            (Target::new(Kind::Bin, "rustx"), vec![]),
            (Target::new(Kind::Example, "tool"), vec![]),
            (
                Target::new(Kind::Test, "durable"),
                vec![("recovery", false)],
            ),
            (
                Target::new(Kind::Test, "contracts"),
                vec![("fixture", false)],
            ),
        ]))
    }

    /// The repository's lane shape in miniature.
    const WORKFLOW: &str = r"
jobs:
  fast:
    runs-on: ubuntu-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['fast']
    env:
      VERIFICATION_PROFILE: fast
    steps:
      - name: Core
        run: 'cargo test --lib --all-features -- model:: --skip ::scale::'
  contracts:
    runs-on: ubuntu-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['contracts']
    env:
      VERIFICATION_PROFILE: full
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: cargo build --bins
      - name: Units
        run: 'cargo test --lib --bins --examples --all-features -- --skip boundary_suites::'
      - name: Pure
        run: cargo test --test contracts --all-features
      - name: Doctests
        run: cargo test --doc --all-features --locked
  boundaries:
    runs-on: ubuntu-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['boundaries']
    env:
      VERIFICATION_PROFILE: full
    steps:
      - name: In-crate
        run: 'cargo test --lib --all-features -- boundary_suites::'
      - name: External
        env:
          RUSTX_REQUIRE_PROVIDER_EMULATOR: '1'
        run: >-
          cargo test --all-features
          --test durable
  platform:
    runs-on: macos-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['platform']
    env:
      VERIFICATION_PROFILE: fast full
    steps:
      - name: Compile
        run: /usr/bin/time -l cargo test --lib --all-features --no-run
      - name: Native
        run: '/usr/bin/time -l cargo test --lib --all-features -- --skip scripted_suites:: --skip manager::tests::'
      - name: External
        run: cargo test --all-features --test durable
";

    const MACOS: &[Requirement] = &[
        Requirement::new(Profile::Full, Platform::Macos, "lib", "boundary_suites::"),
        Requirement::new(Profile::Full, Platform::Macos, "test durable", ""),
        Requirement::new(Profile::Fast, Platform::Macos, "test durable", ""),
        Requirement::new(Profile::Fast, Platform::Linux, "lib", "model::"),
        Requirement::new(Profile::Full, Platform::Linux, "lib", "::scale::"),
        Requirement::new(Profile::Full, Platform::Linux, "doc", ""),
        Requirement::new(Profile::Full, Platform::Linux, "doc", "negative::Api"),
    ];

    fn run(workflow: &str, host: Platform, fake: &Fake) -> Vec<String> {
        let steps = parse_workflow(workflow, &targets()).unwrap();
        check(&steps, &targets(), &BTreeMap::new(), host, &[], MACOS, fake).findings
    }

    fn findings_containing(findings: &[String], needle: &str) -> usize {
        findings.iter().filter(|f| f.contains(needle)).count()
    }

    #[test]
    fn a_test_profile_does_not_turn_other_cargo_subcommands_into_tests() {
        for command in [
            "cargo run --profile test --all-features --example check_test_lanes",
            "cargo build --profile test --bins",
            "cargo check --profile test --all-targets",
        ] {
            let workflow = WORKFLOW.replace(
                "      - name: In-crate",
                &format!(
                    "      - name: Not a test\n        run: {command}\n      - name: In-crate"
                ),
            );
            assert_eq!(
                parse_workflow(&workflow, &targets()).unwrap().len(),
                parse_workflow(WORKFLOW, &targets()).unwrap().len(),
                "{command}"
            );
        }
    }

    #[test]
    fn native_test_preparation_wrapper_is_transparent_to_lane_parsing() {
        for command in [
            "python3 test-support/prepare_native_tests.py -- cargo test --lib --all-features --no-run",
            "/usr/bin/time -l python3 test-support/prepare_native_tests.py -- cargo test --lib --all-features --no-run",
        ] {
            let workflow = format!(
                "jobs:\n  j:\n    runs-on: ubuntu-latest\n    if: fromJSON(needs.plan.outputs.run)['j']\n    env:\n      VERIFICATION_PROFILE: full\n    steps:\n      - name: prep\n        run: {}\n",
                serde_json::to_string(command).unwrap()
            );
            let steps = parse_workflow(&workflow, &targets()).unwrap();
            assert_eq!(steps.len(), 1, "{command}");
            assert!(!steps[0].executes, "{command}");
        }
    }

    #[test]
    fn the_miniature_lane_shape_is_complete_on_both_platforms() {
        assert_eq!(
            run(WORKFLOW, Platform::Linux, &inventory()),
            Vec::<String>::new()
        );
        assert_eq!(
            run(WORKFLOW, Platform::Macos, &inventory()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_unassigned_new_external_target_fails() {
        let mut targets = targets();
        targets.push(Target::new(Kind::Test, "fresh"));
        let steps = parse_workflow(WORKFLOW, &targets).unwrap();
        let findings = check(
            &steps,
            &targets,
            &BTreeMap::new(),
            Platform::Linux,
            &[],
            MACOS,
            &inventory(),
        )
        .findings;
        assert_eq!(
            findings,
            [
                "test fresh is not selected by any executing FULL cargo test step on linux \
              (compile-only --no-run steps and FAST steps do not count); assign it to a lane"
            ]
        );
    }

    #[test]
    fn a_compile_only_step_assigns_nothing() {
        let workflow = WORKFLOW.replace(
            "cargo test --test contracts --all-features",
            "cargo test --test contracts --all-features --no-run",
        );
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings_containing(&findings, "test contracts is not selected"),
            1
        );
    }

    #[test]
    fn a_stale_critical_selector_fails() {
        let workflow = WORKFLOW.replace("-- boundary_suites::'", "-- boundary_conformance::'");
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings_containing(&findings, "filter `boundary_conformance::` matches no test"),
            1
        );
        // The populated sibling lib step does not hide it: the step itself
        // selects nothing, and the old namespace now runs nowhere.
        assert_eq!(
            findings_containing(&findings, "selects no runnable test of lib"),
            1
        );
        assert_eq!(
            findings_containing(&findings, "executed by no FULL lane"),
            1
        );
    }

    #[test]
    fn a_selector_whose_matches_are_all_skipped_fails() {
        let workflow = WORKFLOW.replace(
            "-- boundary_suites::'",
            "-- boundary_suites:: --skip boundary_suites::durable'",
        );
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings_containing(
                &findings,
                "every runnable match of filter `boundary_suites::` is excluded by --skip"
            ),
            1
        );
    }

    #[test]
    fn an_ignored_only_selector_is_not_coverage() {
        let workflow = WORKFLOW.replace(
            "-- boundary_suites::'",
            "-- boundary_suites:: model::measure'",
        );
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings,
            [
                "linux FULL job `boundaries` step `In-crate`: filter `model::measure` matches \
              only #[ignore]d tests"
            ]
        );
    }

    #[test]
    fn a_stale_skip_fails() {
        let workflow =
            WORKFLOW.replace("--skip manager::tests::", "--skip session_manager::tests::");
        let findings = run(&workflow, Platform::Macos, &inventory());
        assert_eq!(
            findings,
            [
                "macos FAST job `platform` step `Native`: --skip `session_manager::tests::` \
              excludes no test; a stale selector",
                "macos FULL job `platform` step `Native`: --skip `session_manager::tests::` \
              excludes no test; a stale selector",
            ]
        );
    }

    #[test]
    fn a_legitimate_platform_exclusion_passes() {
        // `contracts` and the scripted suites never run on macOS; nothing
        // requires them there, so their absence is not a finding.
        assert!(run(WORKFLOW, Platform::Macos, &inventory()).is_empty());
        let workflow = WORKFLOW.replace("--test durable\n", "--test durable --test contracts\n");
        assert!(run(&workflow, Platform::Macos, &inventory()).is_empty());
    }

    #[test]
    fn an_intentionally_empty_binary_or_example_harness_passes() {
        let fake = inventory();
        assert!(fake.0[&Target::new(Kind::Bin, "rustx")].is_empty());
        assert!(fake.0[&Target::new(Kind::Example, "tool")].is_empty());
        assert!(run(WORKFLOW, Platform::Linux, &fake).is_empty());
        let mut fake = inventory();
        fake.0.insert(Target::new(Kind::Test, "durable"), vec![]);
        let findings = run(WORKFLOW, Platform::Linux, &fake);
        assert_eq!(
            findings_containing(&findings, "selects no runnable test of test durable"),
            1
        );
    }

    #[test]
    fn cross_platform_overlap_is_allowed() {
        // `test durable` and the boundary suites run on both platforms.
        let steps = parse_workflow(WORKFLOW, &targets()).unwrap();
        let durable = Target::new(Kind::Test, "durable");
        let platforms: BTreeSet<Platform> = steps
            .iter()
            .filter(|step| step.executes && step.targets.contains(&durable))
            .map(|step| step.platform)
            .collect();
        assert_eq!(platforms.len(), 2);
        assert!(run(WORKFLOW, Platform::Linux, &inventory()).is_empty());
        assert!(run(WORKFLOW, Platform::Macos, &inventory()).is_empty());
    }

    #[test]
    fn a_renamed_required_namespace_fails_where_it_is_required() {
        let mut fake = inventory();
        let lib = fake.0.get_mut(&Target::new(Kind::Lib, "rustx")).unwrap();
        lib[3].0 = "boundary::durable::kill";
        // Linux still executes the renamed test through the complement pair,
        // but the Linux selector and the macOS requirement both go stale.
        let linux = run(WORKFLOW, Platform::Linux, &fake);
        assert_eq!(
            findings_containing(&linux, "filter `boundary_suites::` matches no test"),
            1
        );
        let macos = run(WORKFLOW, Platform::Macos, &fake);
        assert_eq!(
            macos,
            [
                "required suite lib `boundary_suites::` on macos FULL matches no runnable test; \
              was it renamed?"
            ]
        );
        // A requirement skipped out of its platform is also reported.
        let workflow = WORKFLOW.replace(
            "--skip manager::tests::'",
            "--skip manager::tests:: --skip boundary_suites::'",
        );
        let macos = run(&workflow, Platform::Macos, &inventory());
        assert_eq!(
            findings_containing(
                &macos,
                "required suite lib `boundary_suites::` on macos FULL: 1"
            ),
            1
        );
    }

    #[test]
    fn an_unassigned_workspace_member_target_fails() {
        const MEMBER_WORKFLOW: &str = r"
jobs:
  linux-lane:
    runs-on: ubuntu-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['linux-lane']
    env:
      VERIFICATION_PROFILE: full
    steps:
      - name: Members
        run: cargo test --workspace --exclude rustx --lib --all-features --locked
  macos-lane:
    runs-on: macos-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['macos-lane']
    env:
      VERIFICATION_PROFILE: full
    steps:
      - name: Members
        run: cargo test --workspace --exclude rustx --lib --all-features --locked
";
        let members = BTreeMap::from([(
            "rustx-foo".to_owned(),
            BTreeSet::from([Kind::Lib, Kind::Test]),
        )]);
        let steps = parse_workflow(MEMBER_WORKFLOW, &targets()).unwrap();
        let findings = check(
            &steps,
            &targets(),
            &members,
            Platform::Linux,
            &[],
            MACOS,
            &inventory(),
        )
        .findings;
        // `--lib` selects the member's lib on both platforms but never its
        // integration target.
        assert!(
            !findings
                .iter()
                .any(|finding| { finding.contains("workspace member `rustx-foo` Lib") })
        );
        assert_eq!(
            findings
                .iter()
                .filter(|finding| {
                    finding.contains("workspace member `rustx-foo` Test tests are selected")
                })
                .count(),
            2
        );
    }

    #[test]
    fn a_workspace_step_excluding_rustx_covers_members() {
        const MEMBER_WORKFLOW: &str = r"
jobs:
  linux-lane:
    runs-on: ubuntu-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['linux-lane']
    env:
      VERIFICATION_PROFILE: full
    steps:
      - name: Members
        run: cargo test --workspace --exclude rustx --lib --all-features --locked
  macos-lane:
    runs-on: macos-latest
    needs: plan
    if: fromJSON(needs.plan.outputs.run)['macos-lane']
    env:
      VERIFICATION_PROFILE: full
    steps:
      - name: Members
        run: cargo test --workspace --exclude rustx --lib --all-features --locked
";
        let members = BTreeMap::from([("rustx-foo".to_owned(), BTreeSet::from([Kind::Lib]))]);
        let steps = parse_workflow(MEMBER_WORKFLOW, &targets()).unwrap();
        let findings = check(
            &steps,
            &targets(),
            &members,
            Platform::Linux,
            &[],
            MACOS,
            &inventory(),
        )
        .findings;
        assert!(
            !findings
                .iter()
                .any(|finding| { finding.contains("workspace member `rustx-foo`") })
        );
    }

    #[test]
    fn a_job_is_checked_only_on_its_native_platform() {
        let steps = parse_workflow(WORKFLOW, &targets()).unwrap();
        let findings = check(
            &steps,
            &targets(),
            &BTreeMap::new(),
            Platform::Linux,
            &["platform".into()],
            MACOS,
            &inventory(),
        )
        .findings;
        assert_eq!(
            findings,
            ["job `platform` runs on macos; its discovery must run natively there, not on linux"]
        );
    }

    #[test]
    fn uninterpretable_steps_fail_instead_of_being_ignored() {
        for (run, problem) in [
            (
                "cargo test --lib && cargo test --doc",
                "unsupported shell syntax",
            ),
            (
                "cargo test --lib -- --ignored",
                "unsupported libtest option `--ignored`",
            ),
            (
                "cargo test --frozen",
                "unsupported cargo test option `--frozen`",
            ),
            (
                "cargo test -- boundary_suites::",
                "must select its targets explicitly",
            ),
            (
                "cargo test --test missing",
                "--test missing names no Cargo target",
            ),
            (
                "nice cargo test --lib",
                "is not a plain `cargo test` command",
            ),
            (
                "cargo +nightly test --lib",
                "is not a plain `cargo test` command",
            ),
            (
                "cargo --color always test --lib",
                "is not a plain `cargo test` command",
            ),
            (
                "cargo build --bins\ncargo test --lib",
                "single command line",
            ),
        ] {
            let workflow = format!(
                "jobs:\n  j:\n    runs-on: ubuntu-latest\n    if: fromJSON(needs.plan.outputs.run)['j']\n    env:\n      VERIFICATION_PROFILE: full\n    steps:\n      - name: s\n        run: {}\n",
                serde_json::to_string(run).unwrap()
            );
            let errors = parse_workflow(&workflow, &targets()).unwrap_err();
            assert!(errors[0].contains(problem), "{run}: {errors:?}");
        }
        let conditional = "jobs:\n  j:\n    runs-on: ubuntu-latest\n    if: fromJSON(needs.plan.outputs.run)['j']\n    env:\n      VERIFICATION_PROFILE: full\n    steps:\n      - name: s\n        if: false\n        run: cargo test --lib\n";
        assert!(parse_workflow(conditional, &targets()).unwrap_err()[0].contains("`if`"));
    }

    #[test]
    fn a_test_job_must_declare_its_profile_and_plan_selection() {
        let ungated = WORKFLOW.replace("    if: fromJSON(needs.plan.outputs.run)['fast']\n", "");
        let errors = parse_workflow(&ungated, &targets()).unwrap_err();
        assert!(
            errors[0].contains(
                "must be selected by exactly `if: fromJSON(needs.plan.outputs.run)['fast']`"
            ),
            "{errors:?}"
        );
        let borrowed = WORKFLOW.replace("run)['fast']", "run)['contracts']");
        assert!(parse_workflow(&borrowed, &targets()).is_err());
        for declared in [
            "VERIFICATION_PROFILE: quick",
            "VERIFICATION_PROFILE: ''",
            "OTHER: fast",
        ] {
            let workflow = WORKFLOW.replacen("VERIFICATION_PROFILE: fast", declared, 1);
            let errors = parse_workflow(&workflow, &targets()).unwrap_err();
            assert!(
                errors[0].contains("must declare `env: VERIFICATION_PROFILE`"),
                "{declared}: {errors:?}"
            );
        }
    }

    #[test]
    fn a_job_in_both_profiles_contributes_to_each() {
        let steps = parse_workflow(WORKFLOW, &targets()).unwrap();
        let profiles: BTreeSet<Profile> = steps
            .iter()
            .filter(|step| step.job == "platform")
            .map(|step| step.profile)
            .collect();
        assert_eq!(profiles, BTreeSet::from([Profile::Fast, Profile::Full]));
    }

    #[test]
    fn fast_steps_never_count_as_full_coverage() {
        // The model tests now run only in FAST: FULL is incomplete.
        let workflow = WORKFLOW.replace(
            "-- --skip boundary_suites::'",
            "-- --skip boundary_suites:: --skip model::'",
        );
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings_containing(
                &findings,
                "test(s) of lib on linux are executed by no FULL lane"
            ),
            1
        );
        // ... including the scale suite, which FAST deliberately skips.
        assert_eq!(
            findings_containing(&findings, "required suite lib `::scale::` on linux FULL: 1"),
            1
        );
    }

    #[test]
    fn an_unassigned_scale_test_fails_full_completeness() {
        let mut fake = inventory();
        let lib = fake.0.get_mut(&Target::new(Kind::Lib, "rustx")).unwrap();
        lib.push(("boundary_suites::scale::million_rows", false));
        // FAST skips nothing of it and FULL's in-crate boundary step runs it:
        // complete. Skipping it from that step leaves it unassigned.
        assert!(run(WORKFLOW, Platform::Linux, &fake).is_empty());
        let workflow = WORKFLOW.replace(
            "-- boundary_suites::'",
            "-- boundary_suites:: --skip boundary_suites::scale::'",
        );
        let findings = run(&workflow, Platform::Linux, &fake);
        assert_eq!(
            findings,
            [
                "1 runnable test(s) of lib on linux are executed by no FULL lane, e.g. \
              [\"boundary_suites::scale::million_rows\"]",
                "required suite lib `::scale::` on linux FULL: 1 runnable test(s) are executed \
              by no step, e.g. [\"boundary_suites::scale::million_rows\"]"
            ]
        );
    }

    #[test]
    fn a_fast_requirement_is_not_satisfied_by_full() {
        // The FAST lane loses its core selector; FULL still runs the tests.
        let workflow = WORKFLOW.replace("-- model:: --skip ::scale::'", "-- boundary_suites::'");
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings_containing(&findings, "required suite lib `model::` on linux FAST: 1"),
            1
        );
    }

    #[test]
    fn a_critical_fast_selector_that_executes_nothing_fails() {
        let mut fake = inventory();
        let lib = fake.0.get_mut(&Target::new(Kind::Lib, "rustx")).unwrap();
        lib.retain(|(name, _)| !name.starts_with("model::") || name.starts_with("model::scale::"));
        let findings = run(WORKFLOW, Platform::Linux, &fake);
        assert_eq!(
            findings_containing(
                &findings,
                "FAST job `fast` step `Core`: every runnable match of filter `model::` is excluded by --skip"
            ),
            1
        );
        assert_eq!(
            findings_containing(
                &findings,
                "required suite lib `model::` on linux FAST matches no runnable test"
            ),
            1
        );
    }

    #[test]
    fn an_affected_step_is_feedback_but_never_a_fast_requirement() {
        let affected = WORKFLOW.replace(
            "      - name: Core\n        run: 'cargo test --lib --all-features -- model:: --skip ::scale::'\n",
            "      - name: Core\n        run: 'cargo test --lib --all-features -- scripted_suites::'\n      - name: Model\n        if: fromJSON(needs.plan.outputs.steps)['fast:model']\n        run: 'cargo test --lib --all-features -- model:: --skip ::scale::'\n",
        );
        let steps = parse_workflow(&affected, &targets()).unwrap();
        assert!(
            steps
                .iter()
                .any(|step| step.name == "Model" && step.affected)
        );
        let findings = run(&affected, Platform::Linux, &inventory());
        assert_eq!(
            findings,
            [
                "required suite lib `model::` on linux FAST: 1 runnable test(s) are executed by no \
              step, e.g. [\"model::unit\"]"
            ]
        );
        for (condition, problem) in [
            (
                "fromJSON(needs.plan.outputs.steps)['other:model']",
                "understands only",
            ),
            ("github.event_name == 'push'", "understands only"),
        ] {
            let workflow = affected.replace(
                "fromJSON(needs.plan.outputs.steps)['fast:model']",
                condition,
            );
            let errors = parse_workflow(&workflow, &targets()).unwrap_err();
            assert!(errors[0].contains(problem), "{condition}: {errors:?}");
        }
        let full = WORKFLOW.replace(
            "      - name: Pure\n",
            "      - name: Pure\n        if: fromJSON(needs.plan.outputs.steps)['contracts:pure']\n",
        );
        let errors = parse_workflow(&full, &targets()).unwrap_err();
        assert!(errors[0].contains("FULL steps always run"), "{errors:?}");
    }

    #[test]
    fn all_targets_without_doc_does_not_cover_doc() {
        let workflow = WORKFLOW.replace(
            "      - name: Doctests\n        run: cargo test --doc --all-features --locked\n",
            "      - name: AllTargets\n        run: cargo test --all-targets --all-features --locked\n",
        );
        let findings = run(&workflow, Platform::Linux, &inventory());
        assert_eq!(
            findings_containing(&findings, "doc is not selected by any executing FULL"),
            1
        );
        assert_eq!(
            findings_containing(
                &findings,
                "required suite doc on linux FULL is selected by no executing cargo test step"
            ),
            1
        );
    }

    #[test]
    fn malformed_discovery_output_fails_instead_of_being_empty() {
        assert_eq!(
            parse_libtest_terse(true, "a::b: test\n\n", "").unwrap(),
            BTreeSet::from(["a::b".to_owned()])
        );
        assert!(parse_libtest_terse(true, "", "").unwrap().is_empty());
        assert!(parse_libtest_terse(false, "", "error[E0425]").is_err());
        assert!(parse_libtest_terse(true, "2 tests, 0 benchmarks\n", "").is_err());
    }

    #[test]
    fn doctest_block_parser_handles_all_blocks_and_fails_closed() {
        let banner = "    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.14s\n   Doc-tests rustx\n";
        let single =
            "src/a.rs - a::Pos (line 10): test\n1 test, 0 benchmarks\nall doctests ran in 0.20s\n";
        let single_parsed = parse_doctest_list(true, single, banner).unwrap();
        assert_eq!(single_parsed.blocks, vec![(1, 0)]);
        assert_eq!(
            single_parsed.tests,
            BTreeSet::from(["src/a.rs - a::Pos (line 10)".to_owned()])
        );

        let stdout = "\
src/a.rs - a::Pos (line 10): test

1 test, 0 benchmarks
src/b.rs - b::Neg (line 20): test
src/b.rs - b::Neg2 (line 25): test

2 tests, 0 benchmarks
all doctests ran in 1.40s; merged doctests compilation took 1.38s
";
        let parsed = parse_doctest_list(true, stdout, banner).unwrap();
        assert_eq!(parsed.blocks, vec![(1, 0), (2, 0)]);
        assert_eq!(
            parsed.tests,
            BTreeSet::from([
                "src/a.rs - a::Pos (line 10)".to_owned(),
                "src/b.rs - b::Neg (line 20)".to_owned(),
                "src/b.rs - b::Neg2 (line 25)".to_owned(),
            ])
        );
        // Zero matches across multiple blocks (`--ignored`) still produces valid summaries.
        let zero = "0 tests, 0 benchmarks\n0 tests, 0 benchmarks\nall doctests ran in 0.10s\n";
        assert_eq!(
            parse_doctest_list(true, zero, banner).unwrap().blocks,
            vec![(0, 0), (0, 0)]
        );
        // `--workspace`: every member library prints a `Doc-tests` banner;
        // crates carrying doctests end with a timing line while empty crates
        // only emit their `(0, 0)` summary between them.
        let workspace_stdout = "\
src/a.rs - a::Pos (line 10): test
1 test, 0 benchmarks
all doctests ran in 0.20s; merged doctests compilation took 0.20s
0 tests, 0 benchmarks
0 tests, 0 benchmarks
src/b.rs - b::Neg (line 20): test
1 test, 0 benchmarks
all doctests ran in 0.10s; merged doctests compilation took 0.10s
";
        let workspace_banner = "   Doc-tests rustx\n   Doc-tests rustx_types\n";
        let workspace_parsed =
            parse_doctest_list(true, workspace_stdout, workspace_banner).unwrap();
        assert_eq!(
            workspace_parsed.tests,
            BTreeSet::from([
                "src/a.rs - a::Pos (line 10)".to_owned(),
                "src/b.rs - b::Neg (line 20)".to_owned(),
            ])
        );
        // Unit harness stdout/stderr (`Running ...`), missing timing summary, count mismatch,
        // or garbage lines must fail closed.
        assert!(parse_doctest_list(true, zero, "   Running unittests src/lib.rs\n").is_err());
        assert!(parse_doctest_list(true, zero, "").is_err());
        assert!(
            parse_doctest_list(
                true,
                "src/a.rs - a::Pos (line 10): test\n1 test, 0 benchmarks\n",
                banner
            )
            .is_err()
        );
        assert!(parse_doctest_list(true, "src/a.rs - a::Pos (line 10): test\n2 tests, 0 benchmarks\nall doctests ran in 1s\n", banner).is_err());
        assert!(parse_doctest_list(false, "", "error[E0425]").is_err());
    }

    #[test]
    fn artifact_build_verification_binds_identity_and_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"x\"\nversion = \"0.1.0\"\n").unwrap();
        let canonical_manifest = std::fs::canonicalize(&manifest).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let src_lib = dir.path().join("src/lib.rs");
        let src_other = dir.path().join("src/other.rs");
        std::fs::write(&src_lib, "").unwrap();
        std::fs::write(&src_other, "").unwrap();
        let canonical_src = std::fs::canonicalize(&src_lib).unwrap();
        let exe = dir.path().join("fake-test-harness");
        let exe_dup = dir.path().join("fake-test-harness-2");
        std::fs::write(&exe, b"bin").unwrap();
        std::fs::write(&exe_dup, b"bin2").unwrap();

        let target = Target::new(Kind::Lib, "x");
        let pkg_id = "path+file:///tmp/x#x@0.1.0";
        let identity = ArtifactIdentity {
            package_id: pkg_id,
            canonical_manifest: &canonical_manifest,
            canonical_src: &canonical_src,
            target: &target,
        };

        let artifact = |pkg: &str,
                        man: &Path,
                        src: &Path,
                        name: &str,
                        kind: &str,
                        test_profile: bool,
                        exe_path: Option<&Path>| {
            serde_json::json!({
                "reason": "compiler-artifact",
                "package_id": pkg,
                "manifest_path": man,
                "target": {"name": name, "kind": [kind], "src_path": src},
                "profile": {"test": test_profile},
                "executable": exe_path,
            })
            .to_string()
        };
        let valid_json = format!(
            "{}\n{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                false,
                None
            ),
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe)
            ),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        assert_eq!(
            parse_artifact_build(true, &valid_json, "", &identity).unwrap(),
            exe
        );

        // Wrong role (`profile.test == false` only, no libtest harness built) fails closed.
        let non_test_only = format!(
            "{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                false,
                Some(&exe)
            ),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        assert!(parse_artifact_build(true, &non_test_only, "", &identity).is_err());

        // Wrong package ID fails closed even when manifest/target match.
        let wrong_pkg = format!(
            "{}\n{}\n",
            artifact(
                "path+file:///tmp/other#other@0.1.0",
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe),
            ),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        assert!(parse_artifact_build(true, &wrong_pkg, "", &identity).is_err());

        // Wrong target source path fails closed even when package/manifest/name match.
        let wrong_src = format!(
            "{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &src_other,
                "x",
                "lib",
                true,
                Some(&exe)
            ),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        let src_err = parse_artifact_build(true, &wrong_src, "", &identity).unwrap_err();
        assert!(
            src_err.contains("does not match metadata source"),
            "{src_err}"
        );

        // Incomplete stream (`build-finished` missing), failed build-finished, or missing executable fails closed.
        let incomplete = format!(
            "{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe)
            )
        );
        assert!(parse_artifact_build(true, &incomplete, "", &identity).is_err());
        let failed_build = format!(
            "{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe)
            ),
            serde_json::json!({"reason": "build-finished", "success": false}),
        );
        assert!(parse_artifact_build(true, &failed_build, "", &identity).is_err());
        let missing_file = dir.path().join("missing-harness");
        let missing_exe_json = format!(
            "{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&missing_file)
            ),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        assert!(parse_artifact_build(true, &missing_exe_json, "", &identity).is_err());

        // Duplicate matching test artifacts fail closed.
        let duplicate = format!(
            "{}\n{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe)
            ),
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe_dup)
            ),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        let dup_err = parse_artifact_build(true, &duplicate, "", &identity).unwrap_err();
        assert!(dup_err.contains("found 2"), "{dup_err}");

        // Duplicate `build-finished` records (including `false` followed by `true`) fail closed.
        let duplicate_finish = format!(
            "{}\n{}\n{}\n",
            artifact(
                pkg_id,
                &canonical_manifest,
                &canonical_src,
                "x",
                "lib",
                true,
                Some(&exe)
            ),
            serde_json::json!({"reason": "build-finished", "success": false}),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        let dup_finish_err =
            parse_artifact_build(true, &duplicate_finish, "", &identity).unwrap_err();
        assert!(
            dup_finish_err.contains("duplicate build-finished"),
            "{dup_finish_err}"
        );

        // Mismatched manifest fails closed.
        let other = dir.path().join("other/Cargo.toml");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "[package]\nname = \"x\"\nversion = \"0.1.0\"\n").unwrap();
        let mismatched = format!(
            "{}\n{}\n",
            artifact(pkg_id, &other, &canonical_src, "x", "lib", true, Some(&exe)),
            serde_json::json!({"reason": "build-finished", "success": true}),
        );
        assert!(parse_artifact_build(true, &mismatched, "", &identity).is_err());
    }

    #[test]
    fn diagnostics_and_stderr_relay_on_success_and_before_late_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        let src_lib = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(src_lib.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "[package]\nname = \"x\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(&src_lib, "").unwrap();
        let canonical_manifest = std::fs::canonicalize(&manifest).unwrap();
        let canonical_src = std::fs::canonicalize(&src_lib).unwrap();
        let exe = dir.path().join("fake-harness");
        std::fs::write(&exe, b"bin").unwrap();
        let target = Target::new(Kind::Lib, "x");
        let pkg_id = "path+file:///tmp/x#x@0.1.0";
        let identity = ArtifactIdentity {
            package_id: pkg_id,
            canonical_manifest: &canonical_manifest,
            canonical_src: &canonical_src,
            target: &target,
        };
        let artifact_line = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": pkg_id,
            "manifest_path": &canonical_manifest,
            "target": {"name": "x", "kind": ["lib"], "src_path": &canonical_src},
            "profile": {"test": true},
            "executable": &exe,
        })
        .to_string();
        let warn_line = serde_json::json!({
            "reason": "compiler-message",
            "message": {"rendered": "warning: unused variable `w`\n"}
        })
        .to_string();

        // Successful build relays warning + Cargo stderr without dumping compiler-artifact JSON.
        let ok_json = format!(
            "{artifact_line}\n{warn_line}\n{}\n",
            serde_json::json!({"reason": "build-finished", "success": true})
        );
        let mut emitted = Vec::new();
        let built = parse_artifact_build_with(
            true,
            &ok_json,
            "Compiling x v0.1.0\n",
            &identity,
            &mut |line| emitted.push(line.to_owned()),
        )
        .unwrap();
        assert_eq!(built, exe);
        assert_eq!(
            emitted,
            vec![
                "Compiling x v0.1.0".to_owned(),
                "warning: unused variable `w`".to_owned(),
            ]
        );
        assert!(
            emitted
                .iter()
                .all(|line| !line.contains("compiler-artifact"))
        );

        // Warning followed by a late malformed JSON record still emits both stderr and warning.
        let late_parse_err_json = format!("{warn_line}\n{{not-json\n");
        let mut partial = Vec::new();
        let err = parse_artifact_build_with(
            false,
            &late_parse_err_json,
            "cargo stderr note\n",
            &identity,
            &mut |line| partial.push(line.to_owned()),
        )
        .unwrap_err();
        assert!(err.contains("malformed cargo json record"), "{err}");
        assert_eq!(
            partial,
            vec![
                "cargo stderr note".to_owned(),
                "warning: unused variable `w`".to_owned(),
            ]
        );

        // Successful native libtest binary stderr and doctest warning stderr are also relayed.
        let mut libtest_emitted = Vec::new();
        let listed = parse_libtest_terse_with(
            true,
            "a::b: test\n",
            "native harness notice\n",
            &mut |line| libtest_emitted.push(line.to_owned()),
        )
        .unwrap();
        assert_eq!(listed, BTreeSet::from(["a::b".to_owned()]));
        assert_eq!(libtest_emitted, vec!["native harness notice".to_owned()]);

        let mut doctest_emitted = Vec::new();
        let doc_parsed = parse_doctest_list_with(
            true,
            "src/lib.rs - f (line 1): test\n1 test, 0 benchmarks\nall doctests ran in 0.1s\n",
            "warning: doc lint\n   Doc-tests x\n",
            &mut |line| doctest_emitted.push(line.to_owned()),
        )
        .unwrap();
        assert_eq!(doc_parsed.blocks, vec![(1, 0)]);
        assert_eq!(
            doctest_emitted,
            vec!["warning: doc lint\n   Doc-tests x".to_owned()]
        );
    }

    #[test]
    fn metadata_targets_are_classified_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("examples")).unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let lib_rs = dir.path().join("src/lib.rs");
        let main_rs = dir.path().join("src/main.rs");
        let tool_rs = dir.path().join("examples/tool.rs");
        let durable_rs = dir.path().join("tests/durable.rs");
        for path in [&manifest, &lib_rs, &main_rs, &tool_rs, &durable_rs] {
            std::fs::write(path, "").unwrap();
        }
        let metadata = serde_json::json!({"packages": [{
            "id": "path+file:///tmp/rustx#rustx@0.1.0",
            "manifest_path": &manifest,
            "targets": [
                {"name": "rustx", "kind": ["lib"], "doctest": true, "src_path": &lib_rs},
                {"name": "rustx", "kind": ["bin"], "src_path": &main_rs},
                {"name": "tool", "kind": ["example"], "src_path": &tool_rs},
                {"name": "durable", "kind": ["test"], "src_path": &durable_rs},
                {"name": "build-script-build", "kind": ["custom-build"], "src_path": &lib_rs}
            ]
        }]});
        let parsed = parse_metadata(&metadata).unwrap();
        assert_eq!(
            parsed.targets,
            [
                Target::new(Kind::Lib, "rustx"),
                Target::new(Kind::Bin, "rustx"),
                Target::new(Kind::Example, "tool"),
                Target::new(Kind::Test, "durable"),
                Target::new(Kind::Doc, "rustx"),
            ]
        );
        assert_eq!(
            parsed.sources.get(&Target::new(Kind::Lib, "rustx")),
            Some(&std::fs::canonicalize(&lib_rs).unwrap())
        );

        let no_doctest = serde_json::json!({"packages": [{
            "id": "path+file:///tmp/rustx#rustx@0.1.0",
            "manifest_path": &manifest,
            "targets": [
                {"name": "rustx", "kind": ["lib"], "doctest": false, "src_path": &lib_rs}
            ]
        }]});
        assert_eq!(
            parse_metadata(&no_doctest).unwrap().targets,
            [Target::new(Kind::Lib, "rustx")]
        );
        let proc_macro = serde_json::json!({"packages": [{
            "id": "path+file:///tmp/rustx#rustx@0.1.0",
            "manifest_path": &manifest,
            "targets": [
                {"name": "m", "kind": ["proc-macro"], "src_path": &lib_rs}
            ]
        }]});
        assert!(parse_metadata(&proc_macro).is_err());
    }

    /// Selects [`real_cargo_discovery_child`] and names the fixture package
    /// it discovers.
    const FIXTURE_ENV: &str = "RUSTX_LANE_CHECK_FIXTURE";

    /// A dependency-free package with one ordinary lib test pair, one ignored
    /// test, one integration test and deliberately empty bin and example
    /// harnesses. Every test body leaves an `executed` marker.
    const FIXTURE: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            r#"[package]
name = "lane_fixture"
version = "0.0.0"
edition = "2024"

[workspace]
"#,
        ),
        (
            "src/lib.rs",
            r#"/// ```rust
/// assert_eq!(lane_fixture::api_value(), 7);
/// ```
///
/// ```compile_fail
/// let _ = lane_fixture::hidden_value();
/// ```
pub fn api_value() -> u32 {
    hidden_value()
}

fn hidden_value() -> u32 {
    7
}

#[cfg(test)]
mod tests {
    fn executed() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/executed"), "").unwrap();
    }

    #[test]
    fn alpha_runs() {
        executed();
    }

    #[test]
    fn beta_runs() {
        executed();
    }

    #[test]
    #[ignore]
    fn alpha_measured() {
        executed();
    }
}
"#,
        ),
        (
            "tests/flow.rs",
            r#"#[test]
fn flow_runs() {
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/executed"), "").unwrap();
}
"#,
        ),
        ("src/main.rs", "fn main() {}\n"),
        ("examples/empty.rs", "fn main() {}\n"),
    ];

    type Probes = BTreeMap<String, Result<BTreeSet<String>, String>>;

    /// Fixture child entry point of
    /// [`real_cargo_discovery_is_independent_of_inherited_color`]. It runs
    /// the production [`CargoDiscovery`] over the fixture under
    /// the environment its parent chose, and writes what it discovered to
    /// `probes.json` there. It never launches itself.
    #[test]
    #[ignore = "fixture subprocess, invoked by its parent test"]
    fn real_cargo_discovery_child() {
        let root =
            std::env::var_os(FIXTURE_ENV).expect("fixture parent must provide its environment");
        let root = std::path::PathBuf::from(root);
        let discovery = CargoDiscovery::new(cargo_program(), &root).unwrap();
        let features = ["--all-features".to_owned()];
        let lib = Target::new(Kind::Lib, "lane_fixture");
        let doc = Target::new(Kind::Doc, "lane_fixture");
        let list = |target: &Target, libtest: &[&str], ignored: bool| {
            let libtest: Vec<String> = libtest.iter().map(|&a| a.to_owned()).collect();
            discovery.list(&features, target, &libtest, ignored)
        };
        let runnable = |target: &Target, libtest: &[&str]| {
            let libtest: Vec<String> = libtest.iter().map(|&a| a.to_owned()).collect();
            discovery
                .runnable(&features, target, &libtest)
                .map(|(runnable, _)| runnable)
        };
        let probes: Probes = [
            ("lib listed", list(&lib, &[], false)),
            ("lib ignored", list(&lib, &[], true)),
            ("lib runnable", runnable(&lib, &[])),
            ("lib `alpha` runnable", runnable(&lib, &["alpha"])),
            ("lib `measured` runnable", runnable(&lib, &["measured"])),
            ("lib `measured` ignored", list(&lib, &["measured"], true)),
            ("lib --skip `alpha`", runnable(&lib, &["--skip", "alpha"])),
            (
                "lib `tests::alpha` --exact",
                runnable(&lib, &["tests::alpha", "--exact"]),
            ),
            (
                "lib `tests::alpha_runs` --exact",
                runnable(&lib, &["tests::alpha_runs", "--exact"]),
            ),
            ("doc runnable", runnable(&doc, &[])),
            ("doc `api_value` runnable", runnable(&doc, &["api_value"])),
            ("doc ignored", list(&doc, &[], true)),
            (
                "test flow",
                list(&Target::new(Kind::Test, "flow"), &[], false),
            ),
            (
                "bin lane_fixture",
                list(&Target::new(Kind::Bin, "lane_fixture"), &[], false),
            ),
            (
                "example empty",
                list(&Target::new(Kind::Example, "empty"), &[], false),
            ),
        ]
        .into_iter()
        .map(|(probe, result)| (probe.to_owned(), result))
        .collect();
        // Repeated queries for the same `(features, target)` share one acquired
        // harness binary rather than spawning Cargo per query.
        assert_eq!(discovery.acquire_count.get(), 4);
        assert_eq!(discovery.harnesses.borrow().len(), 4);
        // Distinct feature argument sets trigger separate harness acquisitions,
        // while another query with that same feature set reuses the cached binary.
        assert!(discovery.runnable(&[], &lib, &[]).is_ok());
        assert_eq!(discovery.acquire_count.get(), 5);
        assert_eq!(discovery.harnesses.borrow().len(), 5);
        assert!(discovery.runnable(&[], &lib, &["alpha".to_owned()]).is_ok());
        assert_eq!(discovery.acquire_count.get(), 5);
        std::fs::write(
            root.join("probes.json"),
            serde_json::to_string(&probes).unwrap(),
        )
        .unwrap();
    }

    /// The hosted toolchain action exports `CARGO_TERM_COLOR=always`; it
    /// must change neither harness recognition nor any discovered inventory.
    /// Real Cargo and libtest run over a fixture package with its own target
    /// directory, and each color mode is set only in a re-executed child's
    /// environment.
    #[test]
    fn real_cargo_discovery_is_independent_of_inherited_color() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        for (path, text) in FIXTURE {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let isolated = |command: &mut Command| {
            command
                .current_dir(root)
                .env("CARGO_TARGET_DIR", root.join("target"))
                .env("CARGO_NET_OFFLINE", "true");
        };
        let mut lock = Command::new(cargo_program());
        lock.arg("generate-lockfile");
        isolated(&mut lock);
        assert!(lock.status().unwrap().success());

        let names = |names: &[&str]| -> Result<BTreeSet<String>, String> {
            Ok(names.iter().map(|&name| name.to_owned()).collect())
        };
        let expected: Probes = [
            (
                "lib listed",
                names(&[
                    "tests::alpha_measured",
                    "tests::alpha_runs",
                    "tests::beta_runs",
                ]),
            ),
            ("lib ignored", names(&["tests::alpha_measured"])),
            (
                "lib runnable",
                names(&["tests::alpha_runs", "tests::beta_runs"]),
            ),
            ("lib `alpha` runnable", names(&["tests::alpha_runs"])),
            ("lib `measured` runnable", names(&[])),
            ("lib `measured` ignored", names(&["tests::alpha_measured"])),
            ("lib --skip `alpha`", names(&["tests::beta_runs"])),
            ("lib `tests::alpha` --exact", names(&[])),
            (
                "lib `tests::alpha_runs` --exact",
                names(&["tests::alpha_runs"]),
            ),
            (
                "doc runnable",
                names(&[
                    "src/lib.rs - api_value (line 1)",
                    "src/lib.rs - api_value (line 5)",
                ]),
            ),
            (
                "doc `api_value` runnable",
                names(&[
                    "src/lib.rs - api_value (line 1)",
                    "src/lib.rs - api_value (line 5)",
                ]),
            ),
            ("doc ignored", names(&[])),
            ("test flow", names(&["flow_runs"])),
            ("bin lane_fixture", names(&[])),
            ("example empty", names(&[])),
        ]
        .into_iter()
        .map(|(probe, result)| (probe.to_owned(), result))
        .collect();

        for color in ["always", "never"] {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args(["tests::real_cargo_discovery_child", "--exact", "--ignored"])
                .env(FIXTURE_ENV, root)
                .env("CARGO_TERM_COLOR", color);
            isolated(&mut child);
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "CARGO_TERM_COLOR={color}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let written = root.join("probes.json");
            let probes: Probes =
                serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
            std::fs::remove_file(written).unwrap();
            assert_eq!(probes, expected, "CARGO_TERM_COLOR={color}");
        }

        // Listing ran no test body; running one does leave the marker.
        let marker = root.join("executed");
        assert!(!marker.exists());
        let mut run = Command::new(cargo_program());
        run.args([
            "test",
            "--locked",
            "--lib",
            "--",
            "tests::alpha_runs",
            "--exact",
        ]);
        isolated(&mut run);
        assert!(run.output().unwrap().status.success());
        assert!(marker.exists());
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// Test-local `cargo` wrapper that isolates `CARGO_TARGET_DIR`, logs exact
    /// NUL-delimited `argv` records, and optionally injects stderr warnings or
    /// failures.
    struct CargoWrapper {
        _dir: tempfile::TempDir,
        script_path: PathBuf,
        log_path: PathBuf,
        fail_path: PathBuf,
        warn_path: PathBuf,
    }

    impl CargoWrapper {
        fn new(target_dir: &Path) -> Self {
            use std::io::Write;
            use std::process::Stdio;

            let dir = tempfile::tempdir().unwrap();
            let script_path = dir.path().join("cargo");
            let log_path = dir.path().join("calls.log");
            let fail_path = dir.path().join("fail.msg");
            let warn_path = dir.path().join("warn.msg");
            let real_cargo = cargo_program();
            let script = format!(
                "#!/bin/sh\n\
                 set -eu\n\
                 export CARGO_TARGET_DIR={target}\n\
                 export CARGO_NET_OFFLINE=true\n\
                 printf '%s\\0' \"$#\" \"$@\" >> {log}\n\
                 if [ -f {fail} ]; then\n\
                   cat {fail} >&2\n\
                   exit 1\n\
                 fi\n\
                 if [ -f {warn} ]; then\n\
                   cat {warn} >&2\n\
                 fi\n\
                 exec {cargo} \"$@\"\n",
                target = shell_quote(&target_dir.display().to_string()),
                log = shell_quote(&log_path.display().to_string()),
                fail = shell_quote(&fail_path.display().to_string()),
                warn = shell_quote(&warn_path.display().to_string()),
                cargo = shell_quote(&real_cargo),
            );
            // Write and chmod the executable in a child `sh` process so the
            // parent test runner never holds a writable FD on `script_path`
            // across parallel test `fork()`s (`ETXTBSY`).
            let mut writer = Command::new("/bin/sh")
                .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
                .arg(&script_path)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            writer
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            assert!(writer.wait().unwrap().success());
            Self {
                _dir: dir,
                script_path,
                log_path,
                fail_path,
                warn_path,
            }
        }

        fn cargo(&self) -> String {
            self.script_path.display().to_string()
        }

        fn set_fail(&self, message: Option<&str>) {
            match message {
                Some(text) => std::fs::write(&self.fail_path, text).unwrap(),
                None if self.fail_path.exists() => std::fs::remove_file(&self.fail_path).unwrap(),
                None => {}
            }
        }

        fn set_warn(&self, message: Option<&str>) {
            match message {
                Some(text) => std::fs::write(&self.warn_path, text).unwrap(),
                None if self.warn_path.exists() => std::fs::remove_file(&self.warn_path).unwrap(),
                None => {}
            }
        }

        fn take_calls(&self) -> Vec<Vec<String>> {
            let Ok(bytes) = std::fs::read(&self.log_path) else {
                return Vec::new();
            };
            std::fs::write(&self.log_path, b"").unwrap();
            if bytes.is_empty() {
                return Vec::new();
            }
            let fields: Vec<&[u8]> = bytes
                .strip_suffix(b"\0")
                .unwrap_or(&bytes)
                .split(|&byte| byte == 0)
                .collect();
            let mut calls = Vec::new();
            let mut index = 0;
            while index < fields.len() {
                let argc: usize = std::str::from_utf8(fields[index]).unwrap().parse().unwrap();
                index += 1;
                let end = index + argc;
                let args = fields[index..end]
                    .iter()
                    .map(|slice| String::from_utf8(slice.to_vec()).unwrap())
                    .collect();
                calls.push(args);
                index = end;
            }
            calls
        }
    }

    fn create_fixture(ignored_doctest: bool) -> tempfile::TempDir {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        for (path, text) in FIXTURE {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        if ignored_doctest {
            let lib_rs = root.join("src/lib.rs");
            let mut content = std::fs::read_to_string(&lib_rs).unwrap();
            content.push_str(
                "\n/// ```rust,ignore\n\
                 /// assert_eq!(lane_fixture::ignored_doc_api(), 7);\n\
                 /// ```\n\
                 pub fn ignored_doc_api() -> u32 {\n\
                     hidden_value()\n\
                 }\n",
            );
            std::fs::write(lib_rs, content).unwrap();
        }
        let mut lock = Command::new(cargo_program());
        lock.arg("generate-lockfile")
            .current_dir(root)
            .env("CARGO_TARGET_DIR", root.join("target"))
            .env("CARGO_NET_OFFLINE", "true");
        assert!(lock.status().unwrap().success());
        fixture
    }

    fn to_strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|&item| item.to_owned()).collect()
    }

    fn expected_doc_args(features: &[&str], libtest: &[&str], ignored: bool) -> Vec<String> {
        let mut args = to_strings(&["test", "--doc", "--locked", "--color", "never"]);
        args.extend(features.iter().map(|&item| item.to_owned()));
        args.push("--".to_owned());
        args.extend(libtest.iter().map(|&item| item.to_owned()));
        args.push("--list".to_owned());
        if ignored {
            args.push("--ignored".to_owned());
        }
        args
    }

    fn normalize_error_diagnostic(message: &str) -> String {
        let tail = message.split_once(": ").map_or(message, |(_, tail)| tail);
        tail.lines()
            .filter_map(|line| {
                if line
                    .trim_start()
                    .starts_with("Blocking waiting for file lock on ")
                {
                    return None;
                }
                if line.trim_start().starts_with("Finished ")
                    && let Some((prefix, elapsed)) = line.split_once(" target(s) in ")
                    && let Some(secs) = elapsed.trim_end().strip_suffix('s')
                    && !secs.is_empty()
                    && secs.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                {
                    Some(format!("{prefix} target(s) in <elapsed>"))
                } else {
                    Some(line.to_owned())
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_same_native_outcome(
        label: &str,
        actual: &Result<BTreeSet<String>, String>,
        native: &Result<BTreeSet<String>, String>,
    ) {
        match (actual, native) {
            (Ok(left), Ok(right)) => assert_eq!(left, right, "{label}"),
            (Err(left), Err(right)) => {
                assert_eq!(
                    normalize_error_diagnostic(left),
                    normalize_error_diagnostic(right),
                    "{label}"
                );
            }
            _ => panic!("{label}: outcome mismatch: actual={actual:?}, native={native:?}"),
        }
    }

    #[test]
    fn empty_global_ignored_shortcut_reuses_single_atom_and_keeps_positive_native() {
        let fixture = create_fixture(false);
        let root = fixture.path();
        let wrapper = CargoWrapper::new(&root.join("target"));
        let discovery = CargoDiscovery::new(wrapper.cargo(), root).unwrap();
        let _ = wrapper.take_calls();

        let features = to_strings(&["--all-features"]);
        let doc = Target::new(Kind::Doc, "lane_fixture");

        // Populate global doc inventory (`ignored=false` and `ignored=true`, where ignored is empty).
        let (all_runnable, all_ignored) = discovery.runnable(&features, &doc, &[]).unwrap();
        assert_eq!(all_runnable.len(), 2);
        assert_eq!(all_ignored, 0);
        assert_eq!(
            wrapper.take_calls(),
            vec![
                expected_doc_args(&["--all-features"], &[], false),
                expected_doc_args(&["--all-features"], &[], true),
            ]
        );

        // Single plain ASCII identifier filter: positive query runs natively, ignored reuses empty global.
        let filtered = discovery
            .runnable(&features, &doc, &to_strings(&["api_value"]))
            .unwrap();
        assert_eq!(
            wrapper.take_calls(),
            vec![expected_doc_args(
                &["--all-features"],
                &["api_value"],
                false
            )]
        );

        // Must match an uncached fresh native CargoDiscovery exactly.
        let native_wrapper = CargoWrapper::new(&root.join("target"));
        let uncached = CargoDiscovery::new(native_wrapper.cargo(), root).unwrap();
        let expected = uncached
            .runnable(&features, &doc, &to_strings(&["api_value"]))
            .unwrap();
        assert_eq!(filtered, expected);

        // Single `::`-separated owner symbol atom also reuses empty global ignored with 0 spawns.
        assert_eq!(
            discovery
                .list(
                    &features,
                    &doc,
                    &to_strings(&["context::status::AgentStatusSurfaceView"]),
                    true,
                )
                .unwrap(),
            BTreeSet::new()
        );
        assert!(wrapper.take_calls().is_empty());
    }

    #[test]
    fn absent_or_nonempty_global_ignored_runs_native_without_prefetch() {
        // 1. Absent global cache -> native filtered query with NO speculative global prefetch.
        let empty_doc_fixture = create_fixture(false);
        let empty_root = empty_doc_fixture.path();
        let wrapper = CargoWrapper::new(&empty_root.join("target"));
        let absent_discovery = CargoDiscovery::new(wrapper.cargo(), empty_root).unwrap();
        let _ = wrapper.take_calls();

        let features = to_strings(&["--all-features"]);
        let doc = Target::new(Kind::Doc, "lane_fixture");
        let lib = Target::new(Kind::Lib, "lane_fixture");

        assert_eq!(
            absent_discovery
                .list(&features, &doc, &to_strings(&["api_value"]), true)
                .unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            wrapper.take_calls(),
            vec![expected_doc_args(&["--all-features"], &["api_value"], true)]
        );

        // 2. Non-empty global ignored on `lib` -> native filtered queries (matching & non-matching).
        assert_eq!(
            absent_discovery.list(&features, &lib, &[], true).unwrap(),
            BTreeSet::from(["tests::alpha_measured".to_owned()])
        );
        assert_eq!(
            absent_discovery
                .list(&features, &lib, &to_strings(&["measured"]), true)
                .unwrap(),
            BTreeSet::from(["tests::alpha_measured".to_owned()])
        );
        assert_eq!(
            absent_discovery
                .list(&features, &lib, &to_strings(&["alpha_runs"]), true)
                .unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            absent_discovery
                .runnable(&features, &lib, &to_strings(&["alpha_runs"]))
                .unwrap(),
            (BTreeSet::from(["tests::alpha_runs".to_owned()]), 0)
        );

        // 3. Non-empty global ignored on `doc` (fixture created BEFORE fresh discovery owner).
        let ignored_doc_fixture = create_fixture(true);
        let ignored_root = ignored_doc_fixture.path();
        let doc_wrapper = CargoWrapper::new(&ignored_root.join("target"));
        let doc_discovery = CargoDiscovery::new(doc_wrapper.cargo(), ignored_root).unwrap();
        let _ = doc_wrapper.take_calls();

        let global_ignored = doc_discovery.list(&features, &doc, &[], true).unwrap();
        assert_eq!(global_ignored.len(), 1);
        assert!(
            global_ignored
                .iter()
                .all(|name| name.contains("ignored_doc_api"))
        );
        let _ = doc_wrapper.take_calls();

        assert_eq!(
            doc_discovery
                .list(&features, &doc, &to_strings(&["ignored_doc_api"]), true)
                .unwrap(),
            global_ignored
        );
        assert_eq!(
            doc_wrapper.take_calls(),
            vec![expected_doc_args(
                &["--all-features"],
                &["ignored_doc_api"],
                true
            )]
        );

        assert_eq!(
            doc_discovery
                .list(&features, &doc, &to_strings(&["api_value"]), true)
                .unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            doc_wrapper.take_calls(),
            vec![expected_doc_args(&["--all-features"], &["api_value"], true)]
        );
    }

    #[test]
    fn empty_global_ignored_requires_exact_features_and_target_and_success() {
        let fixture = create_fixture(false);
        let root = fixture.path();
        let wrapper = CargoWrapper::new(&root.join("target"));
        let discovery = CargoDiscovery::new(wrapper.cargo(), root).unwrap();
        let _ = wrapper.take_calls();

        let all_features = to_strings(&["--all-features"]);
        let doc = Target::new(Kind::Doc, "lane_fixture");
        let bin = Target::new(Kind::Bin, "lane_fixture");
        let lib = Target::new(Kind::Lib, "lane_fixture");

        // Populate empty global ignored for `(&["--all-features"], &doc)` and `(&["--all-features"], &bin)`.
        assert_eq!(
            discovery.list(&all_features, &doc, &[], true).unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            discovery.list(&all_features, &bin, &[], true).unwrap(),
            BTreeSet::new()
        );
        let _ = wrapper.take_calls();

        // Different features vectors never borrow `(&["--all-features"], &doc)`.
        assert_eq!(
            discovery
                .list(&[], &doc, &to_strings(&["api_value"]), true)
                .unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            wrapper.take_calls(),
            vec![expected_doc_args(&[], &["api_value"], true)]
        );

        let no_default = to_strings(&["--no-default-features"]);
        assert_eq!(
            discovery
                .list(&no_default, &doc, &to_strings(&["api_value"]), true)
                .unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            wrapper.take_calls(),
            vec![expected_doc_args(
                &["--no-default-features"],
                &["api_value"],
                true
            )]
        );

        // Different target with same features never borrows `doc` or `bin` empty sets.
        assert_eq!(
            discovery
                .list(&all_features, &lib, &to_strings(&["measured"]), true)
                .unwrap(),
            BTreeSet::from(["tests::alpha_measured".to_owned()])
        );

        // Failed global producer never inserts empty into cache.
        let fail_wrapper = CargoWrapper::new(&root.join("target"));
        let fail_discovery = CargoDiscovery::new(fail_wrapper.cargo(), root).unwrap();
        let _ = fail_wrapper.take_calls();

        fail_wrapper.set_fail(Some("error: simulated global failure\n"));
        let err = fail_discovery
            .list(&all_features, &doc, &[], true)
            .unwrap_err();
        assert!(err.contains("error: simulated global failure"), "{err}");
        fail_wrapper.set_fail(None);
        let _ = fail_wrapper.take_calls();

        assert_eq!(
            fail_discovery
                .list(&all_features, &doc, &to_strings(&["api_value"]), true)
                .unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            fail_wrapper.take_calls(),
            vec![expected_doc_args(&["--all-features"], &["api_value"], true)]
        );
    }

    const DIAG_ENV: &str = "RUSTX_LANE_CHECK_DIAG_FIXTURE";

    #[test]
    #[ignore = "fixture subprocess, invoked by diagnostics preservation test"]
    fn diagnostics_preservation_child() {
        let root =
            std::env::var_os(DIAG_ENV).expect("diagnostics parent must provide fixture root");
        let root = PathBuf::from(root);
        let wrapper = CargoWrapper::new(&root.join("target"));
        let discovery = CargoDiscovery::new(wrapper.cargo(), &root).unwrap();
        let _ = wrapper.take_calls();

        let features = to_strings(&["--all-features"]);
        let doc = Target::new(Kind::Doc, "lane_fixture");

        // 1. Successful global ignored query relays warning stderr.
        wrapper.set_warn(Some("warning: global-ignored-diag\n"));
        assert_eq!(
            discovery.list(&features, &doc, &[], true).unwrap(),
            BTreeSet::new()
        );
        let _ = wrapper.take_calls();

        // 2. Positive query still runs natively and relays warning stderr, while ignored reuses cache.
        wrapper.set_warn(Some("warning: positive-query-diag\n"));
        let (runnable, ignored) = discovery
            .runnable(&features, &doc, &to_strings(&["api_value"]))
            .unwrap();
        assert_eq!(runnable.len(), 2);
        assert_eq!(ignored, 0);
        assert_eq!(
            wrapper.take_calls(),
            vec![expected_doc_args(
                &["--all-features"],
                &["api_value"],
                false
            )]
        );
        wrapper.set_warn(None);

        // 3. Failed global query relays diagnostic stderr both in Err and process stderr.
        let failing = CargoDiscovery::new(wrapper.cargo(), &root).unwrap();
        wrapper.set_fail(Some("error: global-failure-diag\n"));
        let err = failing.list(&features, &doc, &[], true).unwrap_err();
        assert!(err.contains("error: global-failure-diag"), "{err}");
    }

    #[test]
    fn diagnostics_and_warnings_are_preserved_across_global_positive_and_failed_queries() {
        let fixture = create_fixture(false);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "tests::diagnostics_preservation_child",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env(DIAG_ENV, fixture.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "child failed:\nstdout:\n{}\nstderr:\n{stderr}",
            String::from_utf8_lossy(&output.stdout),
        );
        assert!(stderr.contains("warning: global-ignored-diag"), "{stderr}");
        assert!(stderr.contains("warning: positive-query-diag"), "{stderr}");
        assert!(stderr.contains("error: global-failure-diag"), "{stderr}");

        // Diagnostic normalization strips only command identity, Cargo's
        // informational lock-wait progress lines, and Cargo's volatile
        // `Finished ... target(s) in <secs>s` elapsed field, preserving all
        // real error/warning text.
        let fast = "\"/tmp/a/cargo\" \"test\": doctest discovery failed:\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.00s\nwarning: doc lint\nerror: Unrecognized option: 'bogus'";
        let slow_with_lock_wait = "\"/tmp/b/cargo\" \"test\": doctest discovery failed:\n    Blocking waiting for file lock on package cache\n    Blocking waiting for file lock on build directory\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.12s\nwarning: doc lint\nerror: Unrecognized option: 'bogus'";
        let diff_warning = "\"/tmp/b/cargo\" \"test\": doctest discovery failed:\n    Blocking waiting for file lock on package cache\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.12s\nwarning: different doc lint\nerror: Unrecognized option: 'bogus'";
        let diff_error = "\"/tmp/b/cargo\" \"test\": doctest discovery failed:\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.12s\nwarning: doc lint\nerror: Unrecognized option: 'other'";
        assert_eq!(
            normalize_error_diagnostic(fast),
            normalize_error_diagnostic(slow_with_lock_wait)
        );
        assert_ne!(
            normalize_error_diagnostic(fast),
            normalize_error_diagnostic(diff_warning)
        );
        assert_ne!(
            normalize_error_diagnostic(fast),
            normalize_error_diagnostic(diff_error)
        );
    }

    #[test]
    fn excluded_argument_shapes_delegate_and_match_uncached_native_outcomes() {
        let fixture = create_fixture(false);
        let root = fixture.path();
        let wrapper = CargoWrapper::new(&root.join("target"));
        let metadata = metadata_targets(&wrapper.cargo(), root).unwrap();
        let cached = CargoDiscovery::from_metadata(wrapper.cargo(), root, metadata.clone());
        let native_wrapper = CargoWrapper::new(&root.join("target"));

        let features = to_strings(&["--all-features"]);
        let doc = Target::new(Kind::Doc, "lane_fixture");
        let flow = Target::new(Kind::Test, "flow");

        // Populate empty global ignored for both `doc` and `flow`.
        assert_eq!(
            cached.list(&features, &doc, &[], true).unwrap(),
            BTreeSet::new()
        );
        assert_eq!(
            cached.list(&features, &flow, &[], true).unwrap(),
            BTreeSet::new()
        );
        let _ = wrapper.take_calls();

        let doc_shapes: &[&[&str]] = &[
            &["api_value", "--exact"],
            &["--skip", "other"],
            &["api_value", "other"],
            &["--bogus"],
            &["--skip"],
            &["--exact", "--exact"],
            &["--skip", "--exact"],
            &[""],
            &["foo --bogus"],
            &["api_value --include-ignored"],
            &["api_value "],
            &["\"api_value\""],
            &["api.value"],
            &["api\\value"],
            &["-api_value"],
            &["alpha\0beta"],
        ];

        for shape in doc_shapes {
            let args = to_strings(shape);
            let actual = cached.list(&features, &doc, &args, true);
            let uncached =
                CargoDiscovery::from_metadata(native_wrapper.cargo(), root, metadata.clone());
            let native = uncached.list(&features, &doc, &args, true);
            assert_same_native_outcome(&format!("doc {shape:?}"), &actual, &native);

            let calls = wrapper.take_calls();
            if shape.iter().any(|item| item.contains('\0')) {
                assert!(actual.is_err(), "doc NUL shape must error: {shape:?}");
                assert!(
                    calls.is_empty(),
                    "doc NUL shape errors before wrapper exec: {calls:?}"
                );
            } else {
                assert_eq!(
                    calls,
                    vec![expected_doc_args(&["--all-features"], shape, true)],
                    "doc shape must delegate natively: {shape:?}"
                );
            }
        }

        let flow_shapes: &[&[&str]] = &[
            &["flow_runs", "--exact"],
            &["--skip", "other"],
            &["flow_runs", "other"],
            &["--bogus"],
            &["--skip"],
            &["--exact", "--exact"],
            &[""],
            &["foo --bogus"],
            &["\"flow_runs\""],
            &["alpha\0beta"],
        ];
        for shape in flow_shapes {
            let args = to_strings(shape);
            let actual = cached.list(&features, &flow, &args, true);
            let uncached =
                CargoDiscovery::from_metadata(native_wrapper.cargo(), root, metadata.clone());
            let native = uncached.list(&features, &flow, &args, true);
            assert_same_native_outcome(&format!("flow {shape:?}"), &actual, &native);
        }
    }
}
