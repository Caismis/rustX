# Feishu/Lark GitHub notifications

The `Lark notifications` workflow sends signed text messages to the team's
custom bot. It uses the repository secrets `LARK_WEBHOOK` and `LARK_SIGN_SECRET`.
No application code, server, or additional Python packages are involved.

## Events

- Non-draft PR opened or marked ready for review: title, number, author, and link.
- Submitted changes-requested review: reviewer, PR details, and review link.
- Non-draft PR head updated: a reminder to review again.
- PR merged: target branch and PR link. Unmerged closures are quiet.
- Issue assigned: assignee and issue link.
- `CI` or `Workspace Host platform conformance` failed or timed out: branch,
  failing jobs, job links, and workflow run link. Reruns report the current
  attempt only. Successful and cancelled runs are quiet.

Ordinary pushes, successful checks, approvals, and draft updates do not send
messages. Add any future CI workflow name to both the trigger list in
`lark-notify.yml` and the allowlist in `lark_notify.py`.

## Security

`pull_request_target` handles PR lifecycle metadata, including fork PRs.
Every notification job checks out only `.github/scripts` from the repository's
default branch, with persisted Git credentials disabled. It never checks out
PR code, executes a PR's scripts, or restores its caches or artifacts. Event
content is parsed as JSON data rather than interpolated into shell commands.
The token has only contents, Actions, and pull request read permissions.

Fork `pull_request_review` workflows cannot access repository secrets.
`PR review signal` runs without secrets or token permissions and carries only
the PR and review identifiers in its run name. A subsequent `workflow_run`
notification validates those identifiers and retrieves the review from
GitHub's API, checking the requested-changes state and reviewer identity.
GitHub may require maintainer approval for workflows from first-time fork
contributors; the review notification waits until the signal workflow runs.

The bot signs the empty message using HMAC-SHA256 with
`timestamp + "\n" + secret` as the key, then Base64 encodes the result.
Transport errors and bot rejection both fail the workflow. Logs omit request
data, response bodies, credentials, and webhook URLs; HTTP redirects are
disabled. A bot's business-level error fails even when HTTP returns 200.

References: [GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows)
and [Feishu custom bot guide](https://open.feishu.cn/document/client-docs/bot-v3/add-custom-bot).

## Verify delivery

These workflows must be merged into the default branch before GitHub enables
their production events or manual dispatch. The two repository secrets must
already exist, and the bot's signature verification must be enabled.

1. Open **Actions → Lark notifications → Run workflow** and select `main`.
2. Verify that the workflow succeeds and the GitHub chat receives the signed
   test notification. A successful job means the bot API accepted the message;
   confirm its appearance in the intended chat.
3. Exercise a non-draft PR, a changes-requested review, a PR update, an issue
   assignment, and a merge. A genuinely failed CI run should list its failed
   jobs; successful CI runs should remain quiet.

The command-line equivalent of step 1 is:

```bash
gh workflow run lark-notify.yml --ref main
```

Run the deterministic checks locally without secrets or outbound requests:

```bash
python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v
```
