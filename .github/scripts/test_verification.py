import pathlib
import re
import unittest

import verification as v

WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / "workflows" / "ci.yml"


def passing(planned):
    results = {job: "success" if selected else "skipped" for job, selected in planned["run"].items()}
    return {**results, "notifications": "success"}


class Globs(unittest.TestCase):
    def test_double_star_spans_directories_and_single_star_one_segment(self):
        self.assertTrue(v.matches("web-console/src/a/b/c.css", ["web-console/src/**.css"]))
        self.assertTrue(v.matches("web-console/test/e2e/a.spec.ts-snapshots/x/y.png", ["web-console/test/e2e/*.spec.ts-snapshots/**"]))
        self.assertFalse(v.matches("web-console/test/e2e/sub/a.spec.ts", ["web-console/test/e2e/*.spec.ts"]))
        self.assertFalse(v.matches("web-console/src/locale/dictionaries/x/y.ts", ["web-console/src/locale/dictionaries/*.ts"]))
        self.assertTrue(v.matches("README.md", ["*.md"]))
        self.assertFalse(v.matches("examples/AGENTS.md", ["*.md"]))


class Classification(unittest.TestCase):
    def test_runtime_ci_test_infrastructure_and_unknown_paths_are_high_risk(self):
        for path in [
            "src/runtime/agent_loop.rs",
            "src/durable/sqlite.rs",
            "tests/support/app_server_driver.rs",
            ".github/workflows/ci.yml",
            ".github/scripts/verification.py",
            "examples/check_test_lanes.rs",
            "examples/local-runtime/AGENTS.md",
            "protocol/app-server/schema.json",
            "test-support/fake-provider/server.py",
            "web-console/host/product-host.ts",
            "web-console/src/client/app-server.ts",
            "web-console/test/e2e/dogfood-server.ts",
            "web-console/scripts/browser-artifact.ts",
            "web-console/playwright.config.ts",
            "tui/src/protocol/decoder.ts",
            "dev/src/process.ts",
            "web-console/src/workspaces/authority.ts",
            "web-console/src/workspaces/session-admission.ts",
            "web-console/src/connection/controller.ts",
            "web-console/src/app/agent/AgentComposer.tsx",
            "web-console/src/locale/controller.ts",
            "web-console/test/fixture.ts",
            "web-console/test/setup.ts",
            "tui/src/commands/dispatcher.ts",
            "tui/src/presentation/state.ts",
            "tui/src/ui/app.ts",
            "tui/test/support.ts",
            "Cargo.lock",
            "an/unknown/path.txt",
            "Makefile",
        ]:
            with self.subTest(path=path):
                self.assertEqual(v.risk_of(path), "high")

    def test_presentation_documentation_and_leaf_tests_are_low_risk(self):
        for path in [
            "README.md",
            "docs/architecture.md",
            "tests/README.md",
            "web-console/CHAT.md",
            "web-console/src/presentation/agent/Message.tsx",
            "web-console/src/locale/dictionaries/agent.ts",
            "web-console/src/app/agent/AgentComposer.module.css",
            "web-console/test/model-retry.test.tsx",
            "web-console/test/e2e/chat.spec.ts",
            "web-console/test/e2e/agent.spec.ts-snapshots/settled.png",
            "tui/test/deliveries.test.ts",
        ]:
            with self.subTest(path=path):
                self.assertEqual(v.risk_of(path), "low")

    def test_one_high_risk_path_makes_the_change_high_risk(self):
        classified = v.classify(["web-console/src/presentation/a.tsx", "src/lib.rs"])
        self.assertEqual(classified["risk"], "high")
        self.assertEqual(classified["reasons"], ["src/lib.rs: high risk"])
        self.assertEqual(classified["surfaces"], ["rust", "web"])

    def test_unknown_paths_and_an_unreadable_diff_select_every_surface(self):
        for paths in (["an/unknown/path.txt"], []):
            with self.subTest(paths=paths):
                classified = v.classify(paths)
                self.assertEqual(classified["risk"], "high")
                self.assertEqual(classified["surfaces"], sorted(v.SURFACES))

    def test_documentation_selects_no_suite(self):
        self.assertEqual(v.classify(["README.md", "docs/a.md"]), {"risk": "low", "surfaces": [], "reasons": []})


class Listing(unittest.TestCase):
    def test_a_possibly_truncated_file_listing_is_unreadable(self):
        pages = {1: [{"filename": f"web-console/src/presentation/f{index}.tsx"} for index in range(100)]}
        original = v.github
        try:
            v.github = lambda path: pages.get(int(path.rsplit("=", 1)[1]), [])
            self.assertEqual(len(v.changed_paths(1)), 100)
            for page in range(2, 31):
                pages[page] = pages[1]
            self.assertEqual(v.changed_paths(1), [])
        finally:
            v.github = original


class Planning(unittest.TestCase):
    def test_main_schedule_and_dispatch_run_full(self):
        for event in ("push", "schedule", "workflow_dispatch"):
            with self.subTest(event=event):
                planned = v.plan(event, [], [])
                self.assertEqual(planned["profile"], "full")
                self.assertTrue(all(planned["run"][job] for job in v.FULL_JOBS))
                self.assertFalse(planned["run"]["rust-fast"])
                self.assertFalse(planned["run"]["rust-platform-core"])
                self.assertTrue(planned["run"]["rust-platform-boundaries"])

    def test_a_pull_request_runs_fast_until_promoted(self):
        self.assertEqual(v.plan("pull_request", [], ["src/lib.rs"])["profile"], "fast")
        self.assertEqual(v.plan("pull_request", ["bug", v.PROMOTION_LABEL], ["src/lib.rs"])["profile"], "full")

    def test_fast_selects_the_affected_surfaces_only(self):
        run = v.plan("pull_request", [], ["web-console/src/presentation/a.tsx"])["run"]
        self.assertEqual({job for job, selected in run.items() if selected},
                         {"desktop-macos", "browser-prepare", "web-client", "browser-shard", "browser-report"})
        self.assertEqual(v.plan("pull_request", [], ["web-console/src/presentation/a.tsx"])["risk"], "low")
        run = v.plan("pull_request", [], ["src/lib.rs"])["run"]
        self.assertTrue(run["rust-fast"] and run["rust-platform-core"] and run["web-client"])
        self.assertFalse(run["rust-contracts"] or run["rust-platform-boundaries"])
        run = v.plan("pull_request", [], ["README.md"])["run"]
        self.assertFalse(any(run.values()))


class AffectedSteps(unittest.TestCase):
    def test_fast_runs_a_heavy_contract_group_only_when_its_component_changed(self):
        steps = v.plan("pull_request", [], ["src/durable/sqlite.rs", "README.md"])["steps"]
        self.assertEqual(steps, {"rust-fast:durable": True, "rust-fast:app-server": False,
                                 "rust-fast:runtime-client": False})
        steps = v.plan("pull_request", [], ["tests/support/app_server_driver.rs"])["steps"]
        self.assertTrue(steps["rust-fast:app-server"])

    def test_full_has_no_conditional_steps_and_an_unreadable_diff_runs_them_all(self):
        self.assertFalse(any(v.plan("push", [], [])["steps"].values()))
        self.assertTrue(all(v.plan("pull_request", [], [])["steps"].values()))

    def test_every_workflow_step_condition_names_a_planned_step(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        used = set(re.findall(r"fromJSON\(needs\.plan\.outputs\.steps\)\['([^']+)'\]", text))
        self.assertEqual(used, set(v.COMPONENTS))


class BrowserSelection(unittest.TestCase):
    def test_full_runs_the_complete_inventory_in_two_shards(self):
        self.assertEqual(v.plan("push", [], [])["browser"],
                         {"selection": "full", "shards": [1, 2], "specs": [], "screenshots": False})

    def test_fast_is_one_shard_of_smoke_and_changed_specs(self):
        browser = v.plan("pull_request", [], ["web-console/test/e2e/chat.spec.ts", "README.md"])["browser"]
        self.assertEqual(browser, {"selection": "fast", "shards": [1], "specs": ["chat.spec.ts"], "screenshots": False})

    def test_a_changed_screenshot_reference_selects_its_spec(self):
        browser = v.plan("pull_request", [], ["web-console/test/e2e/agent.spec.ts-snapshots/a.png"])["browser"]
        self.assertEqual(browser["specs"], ["agent.spec.ts"])

    def test_a_low_risk_presentation_change_runs_the_browser_and_its_screenshot_specs(self):
        planned = v.plan("pull_request", [], ["web-console/src/presentation/agent/Message.tsx"])
        self.assertEqual(planned["risk"], "low")
        self.assertTrue(planned["run"]["browser-shard"] and planned["run"]["browser-report"])
        self.assertTrue(planned["browser"]["screenshots"])
        self.assertEqual(planned["browser"]["shards"], [1, 2])

    def test_a_high_risk_web_change_runs_smoke_and_changed_specs_on_one_shard(self):
        browser = v.plan("pull_request", [], ["web-console/src/connection/controller.ts"])["browser"]
        self.assertEqual(browser, {"selection": "fast", "shards": [1], "specs": [], "screenshots": False})

    def test_an_unreadable_diff_runs_every_screenshot_spec(self):
        self.assertTrue(v.plan("pull_request", [], [])["browser"]["screenshots"])

    def test_a_rust_change_gets_browser_smoke_feedback(self):
        planned = v.plan("pull_request", [], ["src/lib.rs"])
        self.assertTrue(planned["run"]["browser-shard"])
        self.assertEqual(planned["browser"]["selection"], "fast")


class Gate(unittest.TestCase):
    def test_a_low_risk_fast_run_on_the_current_head_passes(self):
        planned = v.plan("pull_request", [], ["web-console/src/presentation/a.tsx"])
        self.assertEqual(v.gate(planned, passing(planned), "abc", "abc"), [])

    def test_a_high_risk_change_verified_only_by_fast_fails_closed(self):
        planned = v.plan("pull_request", [], ["src/runtime/agent_loop.rs"])
        errors = v.gate(planned, passing(planned), "abc", "abc")
        self.assertEqual(len(errors), 1)
        self.assertIn("high-risk change verified only by FAST", errors[0])
        self.assertIn("src/runtime/agent_loop.rs: high risk", errors[0])

    def test_an_unknown_path_cannot_merge_on_fast(self):
        planned = v.plan("pull_request", [], ["web-console/src/presentation/a.tsx", "new-tool/config.toml"])
        self.assertIn("high-risk change verified only by FAST", v.gate(planned, passing(planned), "abc", "abc")[0])

    def test_a_promoted_high_risk_change_passes_with_every_full_job(self):
        planned = v.plan("pull_request", [v.PROMOTION_LABEL], ["src/lib.rs"])
        self.assertEqual(v.gate(planned, passing(planned), "abc", "abc"), [])

    def test_full_success_for_a_previous_head_does_not_verify_the_current_one(self):
        planned = v.plan("pull_request", [v.PROMOTION_LABEL], ["src/lib.rs"])
        self.assertEqual(v.gate(planned, passing(planned), "old", "new"),
                         ["this run verified old, but the pull request head is now new: superseded"])

    def test_a_tested_merge_commit_must_join_the_event_base_and_head(self):
        planned = v.plan("pull_request", [], ["web-console/src/presentation/a.tsx"])
        good = {"merge": "m", "parents": ["base", "abc"], "base": "base"}
        self.assertEqual(v.gate(planned, passing(planned), "abc", "abc", good), [])
        stale = {"merge": "m", "parents": ["base", "older"], "base": "base"}
        self.assertEqual(v.gate(planned, passing(planned), "abc", "abc", stale),
                         ["the tested merge commit m joins ['base', 'older'], not base base and head abc"])

    def test_a_superseded_fast_run_is_not_current_verification(self):
        planned = v.plan("pull_request", [], ["web-console/src/presentation/a.tsx"])
        self.assertIn("superseded", v.gate(planned, passing(planned), "old", "new")[0])

    def test_full_without_macos_full_fails(self):
        planned = v.plan("push", [], [])
        for job in ("rust-platform-boundaries",):
            for result in ("skipped", "cancelled", "failure", None):
                with self.subTest(job=job, result=result):
                    results = passing(planned)
                    if result is None:
                        del results[job]
                    else:
                        results[job] = result
                    self.assertEqual(v.gate(planned, results, None, None),
                                     [f"{job}: selected for FULL but {result or 'missing'}"])

    def test_a_promoted_pull_request_cannot_pass_on_macos_core_alone(self):
        planned = v.plan("pull_request", [v.PROMOTION_LABEL], ["src/runtime/process_runner.rs"])
        results = {**passing(planned), "rust-platform-core": "success", "rust-platform-boundaries": "skipped"}
        self.assertEqual(v.gate(planned, results, "abc", "abc"),
                         ["rust-platform-core: not selected for FULL but success",
                          "rust-platform-boundaries: selected for FULL but skipped"])

    def test_a_job_that_ran_without_being_selected_fails(self):
        planned = v.plan("pull_request", [], ["web-console/src/presentation/a.tsx"])
        results = {**passing(planned), "rust-contracts": "success"}
        self.assertEqual(v.gate(planned, results, "abc", "abc"),
                         ["rust-contracts: not selected for FAST but success"])


class Workflow(unittest.TestCase):
    """ci.yml selects each gated job exactly as `selected_jobs` names it."""

    def jobs(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        body = text[text.index("\njobs:\n"):]
        found = {}
        for match in re.finditer(r"^  ([a-z0-9-]+):\n((?:    .*\n|\s*\n)*)", body, re.M):
            found[match.group(1)] = match.group(2)
        return found

    def test_every_selected_job_exists_and_is_gated_by_its_own_plan_entry(self):
        jobs = self.jobs()
        planned = v.plan("push", [], [])
        for job in planned["run"]:
            with self.subTest(job=job):
                self.assertIn(job, jobs)
                condition = re.search(r"^    if: (.+)$", jobs[job], re.M)
                self.assertIsNotNone(condition, f"{job} has no if")
                self.assertIn(f"fromJSON(needs.plan.outputs.run)['{job}']", condition.group(1))
                self.assertIsNotNone(re.search(r"^    needs: \[?plan\b", jobs[job], re.M), f"{job} must need plan")

    def test_every_other_job_is_the_plan_the_gate_or_always_on(self):
        jobs = self.jobs()
        planned = v.plan("push", [], [])
        self.assertEqual(set(jobs) - set(planned["run"]), {"plan", "notifications", "verification"})
        gate = jobs["verification"]
        for job in planned["run"]:
            self.assertIn(job, gate, f"the gate must need {job}")
        self.assertIn("if: always()", gate)

    def test_rust_profiles_match_the_lane_check_declarations(self):
        jobs = self.jobs()
        self.assertIn("VERIFICATION_PROFILE: fast\n", jobs["rust-fast"])
        self.assertIn("VERIFICATION_PROFILE: fast\n", jobs["rust-platform-core"])
        for job in ("rust-contracts", "rust-boundaries", "rust-platform-boundaries"):
            self.assertIn("VERIFICATION_PROFILE: full", jobs[job])


if __name__ == "__main__":
    unittest.main()
