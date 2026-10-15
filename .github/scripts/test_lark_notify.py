"""Deterministic notification coverage; no real secrets or network requests."""

import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

import lark_notify


PR = {
    "number": 42,
    "title": "Fix $(touch /tmp/untrusted) `whoami` \"quotes\"\nUnicode: 修复",
    "user": {"login": "developer"},
    "html_url": "https://github.com/Caismis/rustX/pull/42",
    "draft": False,
    "merged": False,
    "base": {"ref": "main"},
    "head": {"ref": "fix-parser"},
}
REVIEW = {
    "state": "CHANGES_REQUESTED",
    "user": {"id": 7, "login": "reviewer"},
    "html_url": PR["html_url"] + "#pullrequestreview-123",
}
SIGNAL = {
    "name": "PR review signal",
    "path": ".github/workflows/pr-review-signal.yml",
    "event": "pull_request_review",
    "conclusion": "success",
    "display_title": "PR review #42 / 123",
    "actor": {"id": 7},
}


class NotificationTests(unittest.TestCase):
    def test_pr_lifecycle_and_untrusted_titles_remain_data(self):
        for action, status in [
            ("opened", "PR opened"),
            ("ready_for_review", "PR ready for review"),
            ("synchronize", "please review again"),
        ]:
            with self.subTest(action=action):
                message = lark_notify.build_message(
                    "pull_request_target", {"action": action, "pull_request": PR}
                )
                for expected in [status, PR["title"], "developer", PR["html_url"]]:
                    self.assertIn(expected, message)

    def test_drafts_and_unmerged_closures_are_quiet(self):
        for action in ["opened", "synchronize"]:
            self.assertIsNone(lark_notify.build_message(
                "pull_request_target", {"action": action, "pull_request": {**PR, "draft": True}}
            ))
        self.assertIsNone(lark_notify.build_message(
            "pull_request_target", {"action": "closed", "pull_request": PR}
        ))
        self.assertIn("Merged into: main", lark_notify.build_message(
            "pull_request_target", {"action": "closed", "pull_request": {**PR, "merged": True}}
        ))

    def test_issue_assignment(self):
        message = lark_notify.build_message("issues", {
            "action": "assigned",
            "issue": {"number": 9, "title": "New task", "html_url": "https://github.com/Caismis/rustX/issues/9"},
            "assignee": {"login": "tester"},
        })
        self.assertIn("Assignee: tester", message)
        self.assertIn("#9 New task", message)

    def test_fork_review_is_loaded_from_canonical_api_records(self):
        calls = []

        def api(path):
            calls.append(path)
            return {"pulls/42/reviews/123": REVIEW, "pulls/42": PR}[path]

        message = lark_notify.build_message("workflow_run", {"workflow_run": SIGNAL}, api)
        self.assertEqual(calls, ["pulls/42/reviews/123", "pulls/42"])
        self.assertIn("PR changes requested", message)
        self.assertIn("Reviewer: reviewer", message)
        self.assertIn(REVIEW["html_url"], message)

    def test_forged_or_failed_review_signal_is_rejected_before_api_access(self):
        for key, value in [
            ("path", ".github/workflows/attacker.yml"),
            ("event", "push"),
            ("conclusion", "failure"),
        ]:
            with self.subTest(key=key):
                self.assertIsNone(lark_notify.build_message(
                    "workflow_run", {"workflow_run": {**SIGNAL, key: value}},
                    lambda _: self.fail("Unexpected API call"),
                ))
        for title in ["PR review #../../secrets / 123", "PR review #42 / 123; whoami", "PR review #0 / 123"]:
            with self.subTest(title=title), self.assertRaises(ValueError):
                lark_notify.build_message(
                    "workflow_run", {"workflow_run": {**SIGNAL, "display_title": title}},
                    lambda _: self.fail("Unexpected API call"),
                )

    def test_approvals_dismissals_and_wrong_reviewers_are_quiet(self):
        for review in [
            {**REVIEW, "state": "APPROVED"},
            {**REVIEW, "state": "DISMISSED"},
            {**REVIEW, "user": {"id": 99}},
        ]:
            self.assertIsNone(lark_notify.build_message(
                "workflow_run", {"workflow_run": SIGNAL}, lambda _: review
            ))

    def test_ci_failure_paginates_jobs_for_the_exact_attempt(self):
        run = {
            "name": "CI", "conclusion": "failure", "id": 88, "run_attempt": 2,
            "head_branch": "fix-parser", "html_url": "https://github.com/Caismis/rustX/actions/runs/88",
        }
        calls = []

        def api(path):
            calls.append(path)
            if path.endswith("&page=1"):
                return {"jobs": [{"name": "Passed", "conclusion": "success"}] * 99 + [
                    {"name": "Clippy", "conclusion": "failure", "html_url": "https://github.com/job/1"}
                ]}
            return {"jobs": [
                {"name": "Tests", "conclusion": "timed_out", "html_url": "https://github.com/job/2"}
            ]}

        message = lark_notify.build_message("workflow_run", {"workflow_run": run}, api)
        self.assertEqual(calls, [
            "actions/runs/88/attempts/2/jobs?per_page=100&page=1",
            "actions/runs/88/attempts/2/jobs?per_page=100&page=2",
        ])
        for expected in ["Branch: fix-parser", "Clippy", "Tests", run["html_url"]]:
            self.assertIn(expected, message)
        self.assertNotIn("Passed", message)

    def test_ci_success_cancel_and_unknown_workflows_are_quiet(self):
        for run in [
            {"name": "CI", "conclusion": "success"},
            {"name": "CI", "conclusion": "cancelled"},
            {"name": "Lark notifications", "conclusion": "failure"},
        ]:
            self.assertIsNone(lark_notify.build_message(
                "workflow_run", {"workflow_run": run}, lambda _: self.fail("Unexpected API call")
            ))

    def test_official_signing_algorithm_known_vector(self):
        payload = lark_notify.signed_payload("rustX test", "demo", 1599360473)
        self.assertEqual(payload["timestamp"], "1599360473")
        self.assertEqual(payload["sign"], "l1N0gAcBjdwBvGm1xMjOF0XSyaLRpR7tuO5dHfhAYc8=")
        self.assertEqual(payload["msg_type"], "text")

    def test_large_messages_fit_bot_limit_and_keep_run_link(self):
        url = "https://github.com/Caismis/rustX/actions/runs/88"
        for text in ["修复" * 10000 + "\n" + url, "\x00" * 10000]:
            payload = lark_notify.signed_payload(text, "demo", 1599360473)
            self.assertLess(len(json.dumps(payload, ensure_ascii=False).encode()), 20_000)
        self.assertTrue(lark_notify.signed_payload("x" * 10000 + "\n" + url, "demo", 1)["content"]["text"].endswith(url))

    def test_titles_cannot_inject_bot_mentions(self):
        payload = lark_notify.signed_payload('<at user_id="all">everyone</at>', "demo", 1)
        self.assertNotIn("<at", payload["content"]["text"])

    def test_signed_post_and_bot_business_error(self):
        env = {"LARK_WEBHOOK": "https://open.feishu.cn/open-apis/bot/v2/hook/test", "LARK_SIGN_SECRET": "demo"}
        with patch.dict(os.environ, env), patch.object(lark_notify, "request_json", return_value={"code": 0}) as send:
            lark_notify.send_notification("Test")
            request = send.call_args.args[0]
            self.assertEqual(request.get_method(), "POST")
            self.assertIn("sign", json.loads(request.data))
            self.assertNotIn("Authorization", request.headers)
        with patch.dict(os.environ, env), patch.object(lark_notify, "request_json", return_value={"code": 19021}):
            with self.assertRaises(ValueError):
                lark_notify.send_notification("Test")

    def test_missing_secrets_fail_closed(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(lark_notify, "request_json") as send:
            with self.assertRaises(ValueError):
                lark_notify.send_notification("Test")
            send.assert_not_called()

    def test_http_redirects_do_not_forward_credentials(self):
        self.assertIsNone(lark_notify.NoRedirect().redirect_request(
            None, None, 307, "Redirect", {}, "https://attacker.invalid"
        ))

    def test_manual_smoke_test_and_errors_do_not_log_secrets(self):
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            event_path.write_text("{}")
            env = {
                "GITHUB_EVENT_PATH": str(event_path), "GITHUB_EVENT_NAME": "workflow_dispatch",
                "GITHUB_REPOSITORY": "Caismis/rustX",
            }
            with patch.dict(os.environ, env), patch.object(lark_notify, "send_notification") as send:
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(lark_notify.main(), 0)
                self.assertIn("[Caismis/rustX]", send.call_args.args[0])
                self.assertIn("Test notification", send.call_args.args[0])
            output = io.StringIO()
            error = urllib.error.URLError("https://secret-webhook.invalid/secret-token")
            with patch.dict(os.environ, env), patch.object(lark_notify, "send_notification", side_effect=error):
                with contextlib.redirect_stderr(output):
                    self.assertEqual(lark_notify.main(), 1)
            self.assertNotIn("secret-token", output.getvalue())
            self.assertNotIn("secret-webhook", output.getvalue())


if __name__ == "__main__":
    unittest.main()
