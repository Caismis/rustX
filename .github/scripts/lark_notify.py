"""Send GitHub metadata to a signed Feishu/Lark custom bot using the stdlib."""

import base64
import hashlib
import hmac
import json
import os
import re
import sys
import time
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Neither bot credentials nor the GitHub token may follow redirects.
        return None


def request_json(request):
    with urllib.request.build_opener(NoRedirect).open(request, timeout=20) as response:
        return json.load(response)


def github_get(path):
    request = urllib.request.Request(
        f"{os.environ['GITHUB_API_URL']}/repos/{os.environ['GITHUB_REPOSITORY']}/{path}",
        headers={
            "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    return request_json(request)


def pr_message(pr, status, details, url=None):
    return (
        f"{status}: PR #{pr['number']} {pr['title']}\n"
        f"Author: {pr['user']['login']}\n{details}\n{url or pr['html_url']}"
    )


def build_message(event_name, event, api=github_get):
    if event_name == "workflow_dispatch":
        return "Test notification: signed GitHub Actions delivery to Feishu/Lark is working."

    if event_name == "pull_request_target":
        pr = event["pull_request"]
        action = event["action"]
        if action == "closed":
            if not pr["merged"]:
                return None
            return pr_message(pr, "PR merged", f"Merged into: {pr['base']['ref']}")
        if pr["draft"]:
            return None
        status = {
            "opened": "PR opened",
            "ready_for_review": "PR ready for review",
            "synchronize": "PR updated; please review again",
        }.get(action)
        if status:
            return pr_message(pr, status, f"Branch: {pr['head']['ref']}")

    if event_name == "issues" and event["action"] == "assigned":
        issue = event["issue"]
        return (
            f"Issue assigned: #{issue['number']} {issue['title']}\n"
            f"Assignee: {event['assignee']['login']}\n{issue['html_url']}"
        )

    if event_name != "workflow_run":
        return None
    run = event["workflow_run"]
    if run["name"] == "PR review signal":
        if (
            run["event"] != "pull_request_review"
            or run["conclusion"] != "success"
            or run["path"] != ".github/workflows/pr-review-signal.yml"
        ):
            return None
        match = re.fullmatch(r"PR review #([1-9][0-9]*) / ([1-9][0-9]*)", run["display_title"])
        if not match:
            raise ValueError("Invalid review signal identifiers")
        number, review_id = match.groups()
        # Run metadata is untrusted data. Query canonical GitHub records rather
        # than trusting titles, review state, or URLs from a PR-produced artifact.
        review = api(f"pulls/{number}/reviews/{review_id}")
        if review["state"] != "CHANGES_REQUESTED" or review["user"]["id"] != run["actor"]["id"]:
            return None
        pr = api(f"pulls/{number}")
        return pr_message(
            pr, "PR changes requested", f"Reviewer: {review['user']['login']}", review["html_url"]
        )

    if run["name"] not in {"CI", "Workspace Host platform conformance"}:
        return None
    if run["conclusion"] not in {"failure", "timed_out"}:
        return None
    failures = []
    page = 1
    while True:
        # An attempt-specific endpoint avoids mixing previous reruns' failures.
        jobs = api(
            f"actions/runs/{int(run['id'])}/attempts/{int(run['run_attempt'])}"
            f"/jobs?per_page=100&page={page}"
        )["jobs"]
        failures.extend(
            f"- {job['name']} ({job['conclusion']})\n  {job['html_url']}"
            for job in jobs if job["conclusion"] in {"failure", "timed_out"}
        )
        if len(jobs) < 100:
            break
        page += 1
    return (
        f"CI failed: {run['name']}\nBranch: {run['head_branch']}\n"
        + "\n".join(failures)
        + f"\n{run['html_url']}"
    )


def signed_payload(text, secret, timestamp):
    # A PR/Issue title may contain bot mention markup. Render it literally.
    text = text.replace("<", "＜")
    key = f"{timestamp}\n{secret}".encode()
    sign = base64.b64encode(hmac.new(key, b"", hashlib.sha256).digest()).decode()
    return {
        "timestamp": str(timestamp),
        "sign": sign,
        "msg_type": "text",
        # Keep large job lists below the bot's 20 KB request limit, including
        # JSON escaping. Preserve the run URL at the end of truncated messages.
        "content": {
            "text": text if len(text) <= 2500 else text[:2200] + "\n...\n" + text.splitlines()[-1][-300:]
        },
    }


def send_notification(text):
    webhook = os.environ.get("LARK_WEBHOOK", "")
    secret = os.environ.get("LARK_SIGN_SECRET", "")
    if not webhook or not secret:
        raise ValueError("LARK_WEBHOOK and LARK_SIGN_SECRET must both be configured")
    if not webhook.startswith("https://"):
        raise ValueError("LARK_WEBHOOK must use HTTPS")
    payload = signed_payload(text, secret, int(time.time()))
    request = urllib.request.Request(
        webhook,
        data=json.dumps(payload, ensure_ascii=False).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    result = request_json(request)
    if result.get("code") != 0:
        # Do not log the response body: it may echo request data or credentials.
        raise ValueError("Bot rejected the notification; check signature and bot security settings")


def main():
    try:
        with open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8") as source:
            event = json.load(source)
        message = build_message(os.environ["GITHUB_EVENT_NAME"], event)
        if message is None:
            print("No notification needed.")
            return 0
        text = f"[{os.environ['GITHUB_REPOSITORY']}]\n{message}"
        send_notification(text)
        print("Notification accepted by Feishu/Lark.")
        return 0
    except Exception:
        # urllib errors contain the secret webhook URL. Never print exceptions,
        # event payloads, requests, or response bodies in public Actions logs.
        print("Notification failed. Check repository secrets, bot settings, and service availability.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
