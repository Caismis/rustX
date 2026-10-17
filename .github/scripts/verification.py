"""FAST and FULL verification profiles for CI (see tests/README.md).

`plan` decides, from the event and the pull request's changed paths, which
profile runs and which jobs it selects; every gated job in ci.yml runs only
when `run[<job id>]` is true, so this module is the single owner of job
selection. `gate` is the one verdict branch protection should require: it
passes only when every selected job succeeded, nothing unselected ran, a
high-risk change was verified by FULL, and the verified pull request head is
still the current one.
"""

import json
import os
import re
import subprocess
import sys
import urllib.request

# A label on the pull request promotes its current and every later head to
# FULL. Adding it starts a run (the `labeled` event); removing it does not.
PROMOTION_LABEL = "full-verification"

# Risk is an explicit allowlist. Only the paths below are low risk: changes
# that cannot alter runtime, persistence, Source, recovery, authorization,
# admission, transport, connection or client ownership, CI, test
# infrastructure or build semantics. Everything else, including every path
# no rule names, is high risk and needs FULL on the exact head before merge.
#
# Documentation that no build, test or runtime reads. Markdown elsewhere is
# not assumed inert: examples/local-runtime holds Agent, Skill and Workflow
# definitions written in Markdown.
DOCUMENTATION = ("*.md", "docs/**", "web-console/*.md", "tui/*.md", "tests/README.md", "LICENSE")
# Web presentation: what a screenshot reference can show changing.
PRESENTATION = (
    # The presentation layer. scripts/provenance.ts fails the Web check if a
    # file here imports a client, protocol, binding, storage, fetch or
    # WebSocket authority, so it can only render what it is given.
    "web-console/src/presentation/**",
    # Pure translations and stylesheets.
    "web-console/src/locale/dictionaries/*.ts",
    "web-console/src/**.css",
    "web-console/public/**",
)
LOW_RISK = DOCUMENTATION + (
    *PRESENTATION,
    # Leaf tests. Each runs in FAST when changed: the Web and TUI unit suites
    # run whole, and a changed browser spec or screenshot reference selects
    # that spec (browser_selection below). Shared fixtures and helpers are
    # test infrastructure and stay high risk.
    "web-console/test/*.test.ts",
    "web-console/test/*.test.tsx",
    "web-console/test/e2e/*.spec.ts",
    "web-console/test/e2e/*.spec.ts-snapshots/**",
    "tui/test/*.test.ts",
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


def glob(pattern):
    """A path glob: `**` spans directories, `*` and `?` stay within one."""
    parts = re.split(r"(\*\*|\*|\?)", pattern)
    tokens = {"**": ".*", "*": "[^/]*", "?": "[^/]"}
    return re.compile("".join(tokens.get(part, re.escape(part)) for part in parts) + r"\Z")


def matches(path, patterns):
    return any(glob(pattern).match(path) for pattern in patterns)


def risk_of(path):
    """`low` only for an allowlisted path; anything else, unknown paths
    included, is `high`."""
    return "low" if matches(path, LOW_RISK) else "high"


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
# Rust lanes replace the FAST Rust lane and macOS full replaces macOS core.
def selected_jobs(profile, surfaces):
    full = profile == "full"
    s = set(surfaces)
    rust = "rust" in s or "protocol" in s
    return {
        "quality": full or rust,
        "rust-fast": not full and rust,
        "rust-contracts": full,
        "rust-boundaries": full,
        "rust-platform-core": not full and rust,
        "rust-platform-boundaries": full,
        "tui": full or bool(s & {"tui", "rust", "protocol"}),
        "app-server-protocol": full or rust,
        "desktop-macos": full or bool(s & {"web", "tui"}),
        "browser-prepare": full or bool(s & {"web", "dev", "rust", "protocol"}),
        "web-client": full or bool(s & {"web", "dev", "rust", "protocol"}),
        "browser-shard": full or bool(s & {"web", "dev", "rust", "protocol"}),
        "browser-report": full or bool(s & {"web", "dev", "rust", "protocol"}),
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


# Browser acceptance in two selections over one discovered inventory
# (web-console/scripts/browser-shards.ts). FULL runs every discovered test in
# two shards. FAST runs one shard: the tests tagged @smoke, every test of a
# changed spec (or of a spec whose screenshot references changed), and, when
# Web source changed, every spec that holds screenshot references, since a
# presentation change can move pixels.
BROWSER_SPEC = "web-console/test/e2e/"


def browser_selection(profile, paths):
    if profile == "full":
        return {"selection": "full", "shards": [1, 2], "specs": [], "screenshots": False}
    specs = set()
    for path in paths:
        if path.startswith(BROWSER_SPEC):
            name = path[len(BROWSER_SPEC):].split("/")[0]
            if name.endswith(".spec.ts-snapshots"):
                name = name[: -len("-snapshots")]
            if name.endswith(".spec.ts"):
                specs.add(name)
    # A low-risk presentation change merges on FAST, so FAST must also hold
    # it to the screenshot references (and spreads them over both shards). A
    # high-risk Web change needs FULL anyway; its FAST run is smoke feedback.
    # An unreadable diff cannot name its specs: it runs every screenshot spec.
    screenshots = not paths or any(matches(path, PRESENTATION) for path in paths)
    return {"selection": "fast", "shards": [1, 2] if screenshots else [1], "specs": sorted(specs),
            "screenshots": screenshots}


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
            "steps": selected_steps(profile, paths), "browser": browser_selection(profile, paths)}


def gate(planned, results, tested_head, current_head, tested=None):
    """Merge verdict: the reasons this run cannot verify its head (empty = pass).

    `results` maps a job id to its GitHub result (success, failure,
    cancelled, skipped); `tested_head` is the pull request head this run's
    event named and `current_head` the pull request's head now (both None
    outside pull requests). `tested` describes the synthetic merge commit a
    pull request run checked out: {"merge", "parents", "base"}; its parents
    must be exactly the event's base and head.
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
    if tested is not None and tested["parents"] != [tested["base"], tested_head]:
        errors.append(f"the tested merge commit {tested['merge']} joins {tested['parents']}, "
                      f"not base {tested['base']} and head {tested_head}")
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


# GitHub lists at most this many files of a pull request.
LISTED_FILES_LIMIT = 3000


def changed_paths(number):
    """Every path the pull request changes (renames contribute both names),
    or none when GitHub's listing may be truncated: an unreadable diff is
    classified as high risk affecting every surface."""
    paths, page, listed = [], 1, 0
    while True:
        files = github(f"pulls/{number}/files?per_page=100&page={page}")
        listed += len(files)
        for item in files:
            paths.append(item["filename"])
            if item.get("previous_filename"):
                paths.append(item["previous_filename"])
        if len(files) < 100:
            return [] if listed >= LISTED_FILES_LIMIT else sorted(set(paths))
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
        output("browser", json.dumps(planned["browser"], separators=(",", ":")))
        summary(f"### {planned['profile'].upper()} verification, {planned['risk']} risk\n\n"
                f"Surfaces: {', '.join(planned['surfaces']) or 'none'}\n\n"
                + "\n".join(f"- {reason}" for reason in planned["reasons"][:20]))
        print(json.dumps(planned, indent=2))
    elif command == "gate":
        planned = json.loads(os.environ["PLAN"])
        results = {job: need["result"] for job, need in json.loads(os.environ["NEEDS"]).items()}
        head = pull["head"]["sha"] if pull else None
        current = github(f"pulls/{pull['number']}")["head"]["sha"] if pull else None
        merge = None
        if pull:
            parents = subprocess.run(["git", "rev-list", "--parents", "-n", "1", "HEAD"], check=True,
                                     capture_output=True, text=True).stdout.split()
            merge = {"merge": parents[0], "parents": parents[1:], "base": pull["base"]["sha"]}
        errors = gate(planned, results, head, current, merge)
        identity = (f"PR head `{head}`, base `{merge['base']}`, tested merge `{merge['merge']}`"
                    if merge else f"commit `{os.environ['GITHUB_SHA']}`")
        summary(f"### Merge verification: {'FAILED' if errors else 'passed'} ({planned['profile'].upper()}, {planned['risk']} risk)\n\n"
                + identity + "\n\n" + "\n".join(f"- {error}" for error in errors))
        for error in errors:
            print(f"::error::{error}")
        if errors:
            sys.exit(1)
        print(f"{planned['profile'].upper()} verification holds for {identity}")
    else:
        sys.exit("usage: verification.py plan|gate")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "")
