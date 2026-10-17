"""FAST and FULL verification profiles for CI (see tests/README.md).

`plan` decides, from the event and the pull request's changed paths, which
profile runs and which jobs it selects; every gated job in ci.yml runs only
when `run[<job id>]` is true, so this module is the single owner of job
selection. `gate` is the one verdict branch protection should require: it
passes only when every selected job succeeded, nothing unselected ran, a
high-risk change was verified by FULL, and the verified pull request head is
still the current one.
"""

import fnmatch
import json
import os
import sys
import urllib.request

# A label on the pull request promotes its current and every later head to
# FULL. Adding it starts a run (the `labeled` event); removing it does not.
PROMOTION_LABEL = "full-verification"

# Paths whose change can alter runtime, persistence, transport, process,
# protocol, build, provenance or verification semantics. Their pull requests
# need FULL on the exact head before merge. First matching rule wins; a path
# no rule names is unknown and therefore high risk.
HIGH_RISK = (
    ".github/**",
    "src/**",
    "tests/**",
    "examples/**",
    "benches/**",
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "schemas/**",
    "protocol/**",
    "test-support/**",
    "dev/**",
    "web-console/host/**",
    "web-console/shared/**",
    "web-console/src/client/**",
    "web-console/src/bindings/**",
    "web-console/src/carrier/**",
    "web-console/scripts/**",
    "web-console/test/e2e/*.ts",
    "web-console/test/e2e/*.py",
    "web-console/test/e2e/*.yaml",
    "web-console/test/fixture*.ts",
    "web-console/test/setup.ts",
    "web-console/playwright.config.ts",
    "web-console/vite.config.ts",
    "web-console/vite.performance.config.ts",
    "web-console/package.json",
    "web-console/pnpm-lock.yaml",
    "web-console/pnpm-workspace.yaml",
    "tui/src/app-server/**",
    "tui/src/protocol/**",
    "tui/package.json",
    "tui/pnpm-lock.yaml",
    "tui/pnpm-workspace.yaml",
)
# Documentation that no build, test or runtime reads. Markdown elsewhere is
# not assumed inert: examples/local-runtime holds Agent, Skill and Workflow
# definitions written in Markdown.
DOCUMENTATION = ("*.md", "docs/**", "web-console/*.md", "tui/*.md", "tests/README.md", "LICENSE")
# Presentation and leaf tests: FAST, with the affected surface's suites
# selected, is their merge evidence. Browser specs sit beside the shared e2e
# helpers that HIGH_RISK names.
LOW_RISK = (
    "web-console/test/e2e/*.spec.ts",
    "web-console/test/e2e/*.spec.ts-snapshots/**",
    "web-console/src/**",
    "web-console/test/**",
    "web-console/public/**",
    "web-console/licenses/**",
    "web-console/index.html",
    "web-console/source-inventory.json",
    "web-console/browser-session.ts",
    "web-console/tsconfig.json",
    "tui/src/**",
    "tui/test/**",
    "tui/tsconfig.json",
)
# Which suites a changed path affects. Documentation selects only the
# always-on jobs; any other path in no surface affects every suite.
SURFACES = {
    "rust": ("src/**", "tests/**", "examples/**", "benches/**", "build.rs", "Cargo.toml",
             "Cargo.lock", "rust-toolchain.toml", "schemas/**", "test-support/**"),
    "protocol": ("protocol/**",),
    "web": ("web-console/**",),
    "tui": ("tui/**",),
    "dev": ("dev/**",),
}


def matches(path, patterns):
    """Glob match where `**` spans directories and `*` stays within one."""
    for pattern in patterns:
        if "**" in pattern:
            prefix, _, suffix = pattern.partition("**")
            if path.startswith(prefix) and fnmatch.fnmatchcase(path[len(prefix):], "*" + suffix):
                return True
        elif fnmatch.fnmatchcase(path, pattern) and path.count("/") == pattern.count("/"):
            return True
    return False


def risk_of(path):
    """`high` or `low` for one path; unknown paths are high."""
    if matches(path, DOCUMENTATION) or matches(path, ("web-console/test/e2e/*.spec.ts",)):
        return "low"
    if matches(path, HIGH_RISK):
        return "high"
    if matches(path, LOW_RISK):
        return "low"
    return "high"


def surfaces_of(path):
    if matches(path, DOCUMENTATION):
        return set()
    # CI definitions and unknown paths affect every suite.
    return {name for name, patterns in SURFACES.items() if matches(path, patterns)} or set(SURFACES)


def classify(paths):
    """Risk, affected surfaces and the reasons, for a pull request's paths."""
    if not paths:
        # An empty or unreadable diff cannot be classified confidently.
        return {"risk": "high", "surfaces": sorted(SURFACES), "reasons": ["no changed paths could be read"]}
    reasons = [f"{path}: high risk" for path in paths if risk_of(path) == "high"]
    surfaces = set().union(*(surfaces_of(path) for path in paths))
    return {"risk": "high" if reasons else "low", "surfaces": sorted(surfaces), "reasons": reasons}


# Every gated job and when it runs. FULL runs everything once: the complete
# Rust lanes replace the FAST Rust lane, and macOS core runs in both, with
# macOS full beside it in FULL.
def selected_jobs(profile, surfaces):
    full = profile == "full"
    s = set(surfaces)
    rust = "rust" in s or "protocol" in s
    return {
        "quality": full or rust,
        "rust-fast": not full and rust,
        "rust-contracts": full,
        "rust-boundaries": full,
        "rust-platform-core": full or rust,
        "rust-platform-boundaries": full,
        "tui": full or bool(s & {"tui", "rust", "protocol"}),
        "app-server-protocol": full or rust,
        "desktop-macos": full or bool(s & {"web", "tui"}),
        "browser-prepare": full or bool(s & {"web", "dev", "rust", "protocol"}),
        "web-client": full or bool(s & {"web", "dev", "rust", "protocol"}),
        "browser-shard": full or "web" in s,
        "browser-report": full or "web" in s,
    }


# Heavy Rust contract groups that FAST runs only for the change's affected
# components (FULL always runs them), as `<job>:<component>` step keys.
COMPONENTS = {
    "rust-fast:durable": ("src/durable/**",),
    "rust-fast:app-server": ("src/app_server/**", "src/local_runtime/**", "tests/scripted/app_server/**",
                             "tests/support/**"),
    "rust-fast:runtime-client": ("src/runtime_client/**",),
}


def selected_steps(profile, paths):
    # FULL has no conditional steps; an unreadable diff affects everything.
    return {key: profile == "fast" and (not paths or any(matches(path, patterns) for path in paths))
            for key, patterns in COMPONENTS.items()}


# Jobs every run requires besides the selected ones.
ALWAYS = ("notifications",)
FULL_JOBS = tuple(job for job, selected in selected_jobs("full", SURFACES).items() if selected)


def plan(event_name, labels, paths):
    """The profile and job selection for one run.

    main, the nightly schedule and manual dispatch run FULL. A pull request
    runs FAST unless it carries the promotion label.
    """
    if event_name != "pull_request":
        profile, classified = "full", {"risk": "high", "surfaces": sorted(SURFACES), "reasons": [f"{event_name} runs FULL"]}
    else:
        classified = classify(paths)
        profile = "full" if PROMOTION_LABEL in labels else "fast"
    return {**classified, "profile": profile, "run": selected_jobs(profile, classified["surfaces"]),
            "steps": selected_steps(profile, paths)}


def gate(planned, results, tested_head, current_head):
    """Merge verdict: the reasons this run cannot verify its head (empty = pass).

    `results` maps a job id to its GitHub result (success, failure,
    cancelled, skipped); `tested_head` is the pull request head this run
    checked out and `current_head` the pull request's head now (both None
    outside pull requests).
    """
    errors = [f"{job}: required by every run but {results.get(job) or 'missing'}"
              for job in ALWAYS if results.get(job) != "success"]
    for job, selected in planned["run"].items():
        result = results.get(job)
        if selected and result != "success":
            errors.append(f"{job}: selected for {planned['profile'].upper()} but {result or 'missing'}")
        if not selected and result not in (None, "skipped"):
            errors.append(f"{job}: not selected for {planned['profile'].upper()} but {result}")
    if planned["risk"] == "high" and planned["profile"] != "full":
        errors.append(
            "high-risk change verified only by FAST; FULL is required on the exact head "
            f"before merge (add the `{PROMOTION_LABEL}` label): " + "; ".join(planned["reasons"][:5])
        )
    if tested_head != current_head:
        errors.append(f"this run verified {tested_head}, but the pull request head is now {current_head}: superseded")
    return errors


def github(path):
    request = urllib.request.Request(
        f"{os.environ['GITHUB_API_URL']}/repos/{os.environ['GITHUB_REPOSITORY']}/{path}",
        headers={"Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}", "Accept": "application/vnd.github+json"},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def changed_paths(number):
    """Every path the pull request changes (renames contribute both names)."""
    paths, page = [], 1
    while True:
        files = github(f"pulls/{number}/files?per_page=100&page={page}")
        for item in files:
            paths.append(item["filename"])
            if item.get("previous_filename"):
                paths.append(item["previous_filename"])
        if len(files) < 100:
            return sorted(set(paths))
        page += 1


def output(name, value):
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(f"{name}={value}\n")


def summary(text):
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write(text + "\n")


def main(command):
    event = json.load(open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8"))
    pull = event.get("pull_request")
    if command == "plan":
        labels = [label["name"] for label in pull["labels"]] if pull else []
        paths = changed_paths(pull["number"]) if pull else []
        planned = plan(os.environ["GITHUB_EVENT_NAME"], labels, paths)
        output("plan", json.dumps(planned, separators=(",", ":")))
        output("run", json.dumps(planned["run"], separators=(",", ":")))
        output("steps", json.dumps(planned["steps"], separators=(",", ":")))
        summary(f"### {planned['profile'].upper()} verification, {planned['risk']} risk\n\n"
                f"Surfaces: {', '.join(planned['surfaces']) or 'none'}\n\n"
                + "\n".join(f"- {reason}" for reason in planned["reasons"][:20]))
        print(json.dumps(planned, indent=2))
    elif command == "gate":
        planned = json.loads(os.environ["PLAN"])
        results = {job: need["result"] for job, need in json.loads(os.environ["NEEDS"]).items()}
        tested = pull["head"]["sha"] if pull else None
        current = github(f"pulls/{pull['number']}")["head"]["sha"] if pull else None
        errors = gate(planned, results, tested, current)
        summary(f"### Merge verification: {'FAILED' if errors else 'passed'} ({planned['profile'].upper()}, {planned['risk']} risk)\n\n"
                + "\n".join(f"- {error}" for error in errors))
        for error in errors:
            print(f"::error::{error}")
        if errors:
            sys.exit(1)
        print(f"{planned['profile'].upper()} verification holds for {tested or os.environ['GITHUB_SHA']}")
    else:
        sys.exit("usage: verification.py plan|gate")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "")
