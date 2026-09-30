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
//! - a Cargo test target (lib, bin, example, integration test, bench) is not
//!   selected by any executing Linux step — `--no-run` compilation does not
//!   count;
//! - a Linux-discovered runnable test is executed by no Linux step;
//! - an executing step selects no runnable test of a lib, integration-test or
//!   bench target (bin and example harnesses may be deliberately empty);
//! - a positive filter selects no runnable test, including when every match
//!   is `--skip`ped or `#[ignore]`d;
//! - a `--skip` excludes nothing, so the name it once excluded has moved;
//! - a [`REQUIREMENTS`] suite is absent, or not entirely executed, on its
//!   platform;
//! - a workflow step or libtest output cannot be interpreted exactly.
//!
//! Discovery is native: each platform is checked by its own CI job on its own
//! runner. `--job <id>` restricts discovery to the targets that job already
//! built; every step on the same platform that selects one of those targets
//! is evaluated, so a target's Linux coverage is complete across lanes.
//!
//! ```text
//! cargo run --example check_test_lanes -- --job rust-contracts
//! cargo run --example check_test_lanes            # every target on this host
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::process::Command;

/// Suites that must run in full on a platform beyond Linux completeness.
///
/// Linux is the complete platform: every runnable test there must execute,
/// so it needs no entries. macOS deliberately runs only the platform-sensitive
/// classes; these entries name the ones whose loss would silently remove
/// native process, signal, socket and filesystem proof. The deterministic
/// contract suites and the pure `contracts`/`provider` targets are Linux-only
/// by design and are intentionally absent.
const REQUIREMENTS: &[Requirement] = &[
    Requirement::new(Platform::Macos, "lib", "boundary_suites::"),
    Requirement::new(Platform::Macos, "test durable", ""),
    Requirement::new(Platform::Macos, "test process", ""),
    Requirement::new(Platform::Macos, "test subagent", ""),
    Requirement::new(Platform::Macos, "test tools", ""),
    Requirement::new(Platform::Macos, "test conformance", ""),
    Requirement::new(Platform::Macos, "test cfg3_catalog", ""),
    Requirement::new(Platform::Macos, "test cfg3_managed_output", ""),
];

/// Command prefixes that only measure the Cargo process they wrap.
const TRANSPARENT_WRAPPERS: &[&[&str]] = &[&["/usr/bin/time", "-l"]];

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
    let targets = metadata_targets(&cargo, root).unwrap_or_else(|error| fail(&[error]));
    let workflow = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))
        .unwrap_or_else(|error| fail(&[format!("cannot read the CI workflow: {error}")]));
    let steps = parse_workflow(&workflow, &targets).unwrap_or_else(|errors| fail(&errors));
    let discovery = CargoDiscovery {
        cargo,
        root,
        cache: std::cell::RefCell::default(),
    };
    let report = check(&steps, &targets, host, &jobs, REQUIREMENTS, &discovery);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Lib,
    Bin,
    Example,
    Test,
    Bench,
}

/// One Cargo target that `cargo test` can build as a libtest harness.
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
        }
    }
}

/// A suite that must execute entirely on `platform`: every runnable test of
/// `target` matching the libtest `filter` (all of them when empty).
#[derive(Clone, Copy, Debug)]
struct Requirement {
    platform: Platform,
    target: &'static str,
    filter: &'static str,
}

impl Requirement {
    const fn new(platform: Platform, target: &'static str, filter: &'static str) -> Self {
        Self {
            platform,
            target,
            filter,
        }
    }

    fn describe(&self) -> String {
        if self.filter.is_empty() {
            format!("{} on {}", self.target, self.platform)
        } else {
            format!("{} `{}` on {}", self.target, self.filter, self.platform)
        }
    }
}

/// One `cargo test` step of the workflow.
#[derive(Clone, Debug)]
struct Step {
    job: String,
    name: String,
    platform: Platform,
    /// `--no-run` compiles without executing; it assigns nothing.
    executes: bool,
    features: Vec<String>,
    targets: Vec<Target>,
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
}

impl Step {
    fn label(&self) -> String {
        format!("{} job `{}` step `{}`", self.platform, self.job, self.name)
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

fn metadata_targets(cargo: &str, root: &Path) -> Result<Vec<Target>, String> {
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
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

fn parse_metadata(metadata: &serde_json::Value) -> Result<Vec<Target>, String> {
    let packages = metadata["packages"]
        .as_array()
        .filter(|packages| packages.len() == 1)
        .ok_or("cargo metadata must describe exactly the one rustx package")?;
    let mut targets = Vec::new();
    for target in packages[0]["targets"]
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
                Kind::Lib
            }
            other => return Err(format!("target {name} has unsupported kinds {other:?}")),
        };
        targets.push(Target::new(kind, name));
    }
    targets.sort();
    Ok(targets)
}

fn parse_workflow(text: &str, targets: &[Target]) -> Result<Vec<Step>, Vec<String>> {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|error| vec![format!("malformed workflow: {error}")])?;
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
            for (key, value) in [
                ("if", &job["if"]),
                ("strategy", &job["strategy"]),
                ("continue-on-error", &job["continue-on-error"]),
                ("defaults", &job["defaults"]),
                ("if", &step["if"]),
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
                Ok(command) => steps.push(Step {
                    job: id.into(),
                    name,
                    platform,
                    ..command
                }),
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

fn platform_of(job: &serde_yaml::Value) -> Result<Platform, String> {
    match job["runs-on"].as_str() {
        Some(runner) if runner.starts_with("ubuntu-") => Ok(Platform::Linux),
        Some(runner) if runner.starts_with("macos-") => Ok(Platform::Macos),
        _ => Err(format!("unsupported runs-on {:?}", job["runs-on"])),
    }
}

fn mentions_cargo_test(run: &str) -> bool {
    run.lines().any(|line| {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        tokens
            .iter()
            .position(|token| *token == "cargo")
            .is_some_and(|cargo| tokens[cargo..].contains(&"test"))
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
    if let Some(wrapper) = TRANSPARENT_WRAPPERS
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
    };
    let of = |kind: Kind| targets.iter().filter(move |target| target.kind == kind);
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
            "--bins" => step.targets.extend(of(Kind::Bin).cloned()),
            "--examples" => step.targets.extend(of(Kind::Example).cloned()),
            "--all-targets" => step.targets.extend(targets.iter().cloned()),
            "--bin" => step.targets.push(named(Kind::Bin)?),
            "--example" => step.targets.push(named(Kind::Example)?),
            "--test" => step.targets.push(named(Kind::Test)?),
            "--bench" => step.targets.push(named(Kind::Bench)?),
            "--all-features" | "--no-default-features" => step.features.push((*argument).into()),
            "--features" | "-F" => {
                let features = cargo
                    .next()
                    .ok_or(format!("{argument} requires a feature list"))?;
                step.features
                    .extend([(*argument).into(), (*features).into()]);
            }
            "--no-run" => step.executes = false,
            "--locked" | "--timings" => {}
            other => return Err(format!("unsupported cargo test option `{other}`")),
        }
    }
    if step.targets.is_empty() {
        return Err("a cargo test step must select its targets explicitly".into());
    }
    step.targets.sort();
    step.targets.dedup();
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
    Ok(step)
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

struct CargoDiscovery<'a> {
    cargo: String,
    root: &'a Path,
    cache: std::cell::RefCell<BTreeMap<ListKey, BTreeSet<String>>>,
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
        // The adapter owns the format it parses: Cargo's own `--color` (before
        // `--`) keeps its status lines plain whatever color mode the caller
        // inherited, e.g. `CARGO_TERM_COLOR=always` on hosted runners.
        let mut command = Command::new(&self.cargo);
        command
            .args(["test", "--locked", "--color", "never"])
            .args(features)
            .args(target.selector())
            .arg("--")
            .args(libtest)
            .args(["--list", "--format", "terse"]);
        if ignored {
            command.arg("--ignored");
        }
        let rendered = format!("{command:?}");
        let output = command
            .current_dir(self.root)
            .output()
            .map_err(|error| format!("cannot run {rendered}: {error}"))?;
        let listed = parse_list(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
        .map_err(|problem| format!("{rendered}: {problem}"))?;
        self.cache.borrow_mut().insert(key, listed.clone());
        Ok(listed)
    }
}

/// Parses `--list --format terse` output of exactly one harness, as
/// [`CargoDiscovery`] produces it: uncolored. Anything else — a failed build,
/// no or a second harness, an unknown line — is an error, never an empty
/// inventory.
fn parse_list(success: bool, stdout: &str, stderr: &str) -> Result<BTreeSet<String>, String> {
    if !success {
        return Err(format!("discovery failed:\n{stderr}"));
    }
    let harnesses = stderr
        .lines()
        .filter(|line| line.trim_start().starts_with("Running "))
        .count();
    if harnesses != 1 {
        return Err(format!(
            "expected exactly one test harness, Cargo ran {harnesses}:\n{stderr}"
        ));
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

#[derive(Debug, Default)]
struct Report {
    summary: Vec<String>,
    findings: Vec<String>,
}

#[allow(clippy::too_many_lines)] // one linear pass over the documented rules
fn check(
    steps: &[Step],
    targets: &[Target],
    host: Platform,
    jobs: &[String],
    requirements: &[Requirement],
    discovery: &dyn Discovery,
) -> Report {
    let mut report = Report::default();
    let executing: Vec<&Step> = steps.iter().filter(|step| step.executes).collect();

    // Assignment and requirement placement need no discovery; every host
    // checks them for the whole workflow.
    for target in targets {
        if !executing
            .iter()
            .any(|step| step.platform == Platform::Linux && step.targets.contains(target))
        {
            report.findings.push(format!(
                "{target} is not selected by any executing cargo test step on linux \
                 (compile-only --no-run steps do not count); assign it to a lane"
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
                    step.platform == requirement.platform && step.targets.contains(target)
                }) {
                    report.findings.push(format!(
                        "required suite {} is selected by no executing cargo test step",
                        requirement.describe()
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
    let evaluated: BTreeSet<&Target> = focus.iter().flat_map(|step| &step.targets).collect();

    for target in evaluated {
        let selecting: Vec<&Step> = executing
            .iter()
            .copied()
            .filter(|step| step.platform == host && step.targets.contains(target))
            .collect();
        let mut covered: BTreeMap<&[String], BTreeSet<String>> = BTreeMap::new();
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
                    covered
                        .entry(step.features.as_slice())
                        .or_default()
                        .extend(runnable);
                }
                Err(problem) => report.findings.push(problem),
            }
        }
        for (features, covered) in &covered {
            if host == Platform::Linux {
                match discovery.runnable(features, target, &[]) {
                    Ok((universe, _)) => {
                        let missing: Vec<&String> = universe.difference(covered).collect();
                        if !missing.is_empty() {
                            report.findings.push(format!(
                                "{} runnable test(s) of {target} on linux are executed by no \
                                 lane, e.g. {:?}",
                                missing.len(),
                                &missing[..missing.len().min(5)]
                            ));
                        }
                    }
                    Err(problem) => report.findings.push(problem),
                }
            }
            for requirement in requirements.iter().filter(|requirement| {
                requirement.platform == host && requirement.target == target.to_string()
            }) {
                let filters: &[String] = if requirement.filter.is_empty() {
                    &[]
                } else {
                    &[requirement.filter.to_owned()]
                };
                match discovery.runnable(features, target, filters) {
                    Ok((required, _)) if required.is_empty() => report.findings.push(format!(
                        "required suite {} matches no runnable test; was it renamed?",
                        requirement.describe()
                    )),
                    Ok((required, _)) => {
                        let missing: Vec<&String> = required.difference(covered).collect();
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
  contracts:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: cargo build --bins
      - name: Units
        run: 'cargo test --lib --bins --examples --all-features -- --skip boundary_suites::'
      - name: Pure
        run: cargo test --test contracts --all-features
  boundaries:
    runs-on: ubuntu-latest
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
    steps:
      - name: Compile
        run: /usr/bin/time -l cargo test --lib --all-features --no-run
      - name: Native
        run: '/usr/bin/time -l cargo test --lib --all-features -- --skip scripted_suites:: --skip manager::tests::'
      - name: External
        run: cargo test --all-features --test durable
";

    const MACOS: &[Requirement] = &[
        Requirement::new(Platform::Macos, "lib", "boundary_suites::"),
        Requirement::new(Platform::Macos, "test durable", ""),
    ];

    fn run(workflow: &str, host: Platform, fake: &Fake) -> Vec<String> {
        let steps = parse_workflow(workflow, &targets()).unwrap();
        check(&steps, &targets(), host, &[], MACOS, fake).findings
    }

    fn findings_containing(findings: &[String], needle: &str) -> usize {
        findings.iter().filter(|f| f.contains(needle)).count()
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
        let findings = check(&steps, &targets, Platform::Linux, &[], MACOS, &inventory()).findings;
        assert_eq!(
            findings,
            [
                "test fresh is not selected by any executing cargo test step on linux \
              (compile-only --no-run steps do not count); assign it to a lane"
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
        assert_eq!(findings_containing(&findings, "executed by no lane"), 1);
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
                "linux job `boundaries` step `In-crate`: filter `model::measure` matches only \
              #[ignore]d tests"
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
                "macos job `platform` step `Native`: --skip `session_manager::tests::` excludes no \
              test; a stale selector"
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
                "required suite lib `boundary_suites::` on macos matches no runnable test; \
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
            findings_containing(&macos, "required suite lib `boundary_suites::` on macos: 1"),
            1
        );
    }

    #[test]
    fn a_job_is_checked_only_on_its_native_platform() {
        let steps = parse_workflow(WORKFLOW, &targets()).unwrap();
        let findings = check(
            &steps,
            &targets(),
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
                "cargo test --workspace",
                "unsupported cargo test option `--workspace`",
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
                "cargo build --bins\ncargo test --lib",
                "single command line",
            ),
        ] {
            let workflow = format!(
                "jobs:\n  j:\n    runs-on: ubuntu-latest\n    steps:\n      - name: s\n        run: {}\n",
                serde_json::to_string(run).unwrap()
            );
            let errors = parse_workflow(&workflow, &targets()).unwrap_err();
            assert!(errors[0].contains(problem), "{run}: {errors:?}");
        }
        let conditional = "jobs:\n  j:\n    runs-on: ubuntu-latest\n    steps:\n      - name: s\n        if: false\n        run: cargo test --lib\n";
        assert!(parse_workflow(conditional, &targets()).unwrap_err()[0].contains("`if`"));
    }

    #[test]
    fn malformed_discovery_output_fails_instead_of_being_empty() {
        let running = "     Running unittests src/lib.rs (target/debug/deps/rustx-0)\n";
        assert_eq!(
            parse_list(true, "a::b: test\n\n", running).unwrap(),
            BTreeSet::from(["a::b".to_owned()])
        );
        assert!(parse_list(true, "", running).unwrap().is_empty());
        assert!(parse_list(false, "", "error[E0425]").is_err());
        assert!(parse_list(true, "a::b: test\n", "").is_err());
        // An unrecognized harness is not an empty one.
        assert!(parse_list(true, "", "").is_err());
        assert!(parse_list(true, "", &running.repeat(2)).is_err());
        assert!(parse_list(true, "2 tests, 0 benchmarks\n", running).is_err());
    }

    #[test]
    fn metadata_targets_are_classified_by_kind() {
        let metadata = serde_json::json!({"packages": [{"targets": [
            {"name": "rustx", "kind": ["lib"]},
            {"name": "rustx", "kind": ["bin"]},
            {"name": "tool", "kind": ["example"]},
            {"name": "durable", "kind": ["test"]},
            {"name": "build-script-build", "kind": ["custom-build"]}
        ]}]});
        assert_eq!(
            parse_metadata(&metadata).unwrap(),
            [
                Target::new(Kind::Lib, "rustx"),
                Target::new(Kind::Bin, "rustx"),
                Target::new(Kind::Example, "tool"),
                Target::new(Kind::Test, "durable"),
            ]
        );
        let proc_macro = serde_json::json!({"packages": [{"targets": [
            {"name": "m", "kind": ["proc-macro"]}
        ]}]});
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
edition = "2021"

[workspace]
"#,
        ),
        (
            "src/lib.rs",
            r#"#[cfg(test)]
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
    /// [`real_cargo_discovery_is_independent_of_inherited_color`]. Without
    /// [`FIXTURE_ENV`] it returns at once and proves nothing. Re-executed with
    /// it, it runs the production [`CargoDiscovery`] over the fixture under
    /// the environment its parent chose, and writes what it discovered to
    /// `probes.json` there. It never launches itself.
    #[test]
    fn real_cargo_discovery_child() {
        let Some(root) = std::env::var_os(FIXTURE_ENV) else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let discovery = CargoDiscovery {
            cargo: cargo_program(),
            root: &root,
            cache: std::cell::RefCell::default(),
        };
        let features = ["--all-features".to_owned()];
        let lib = Target::new(Kind::Lib, "lane_fixture");
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
                .args(["tests::real_cargo_discovery_child", "--exact"])
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
}
