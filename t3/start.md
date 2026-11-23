# Start

These instructions are for a T3 Code thread that runs on a schedule. It starts
one agent for each issue that can start, then it stops. It changes no file,
and it does no issue itself. Nobody reads this thread while it runs, so ask no
question.

The rules are those of SPEC.md, "Issues".

Agent limit: 5

Model of the agent: `claude-opus-5-5`, at high effort

1. Run `df --output=avail -BG .`. If it shows less than 20G, say so and stop.
2. Read the open issues and the open pull requests:

   ```sh
   gh issue list --state open --limit 1000 \
     --json number,title,labels,assignees,body,createdAt
   gh pr list --state open --limit 1000 \
     --json number,headRefName,isDraft,labels
   ```

   The pull request of issue `<number>` is the one whose branch starts with
   `lira/<number>-`.
3. Count the issues that an agent works on: the open issues that have an
   `auto:` label and an assignee, except those whose pull request is ready
   for review. If the count is the agent limit or more, say so and stop.
4. Find the issues that can start. An issue can start when all of these are
   true:

   - It has `auto:ready` or `auto:merge`.
   - It has no assignee.
   - It has no `needs:answer`, and its pull request has none.
   - It is not a parent issue:
     `gh api repos/{owner}/{repo}/issues/<number> --jq .sub_issues_summary.total`
     prints `0`.
   - Each issue in its `Blocked by` line is closed.

5. Take the oldest issues first, as many as the agent limit minus the count
   of step 3. For each one, do steps 6 and 7.
6. Assign the issue: `gh issue edit <number> --add-assignee @me`.
7. Load `mcp__t3-code__t3_thread_launch` with ToolSearch, and start one thread
   with only these fields. Without the other fields, the thread gets the
   project and the runtime mode of this one.

   `title` is `Issue <number>`.

   `modelSelection`, with the model of the agent from above, at high
   effort:

   ```json
   {"instanceId": "claudeAgent", "model": "<model>",
    "options": [{"id": "effort", "value": "high"}]}
   ```

   `clientRequestId` is `issue-<number>-<time>`. `<time>` is what
   `date -u +%Y%m%dT%H%MZ` prints before the first call for the issue. So
   each start of an issue has a new `clientRequestId`, also a second start on
   one day. T3 Code reads the same `clientRequestId` as a retry, and gives
   back the thread of the call before it.

   `workspaceStrategy`, when `git worktree list` shows a worktree whose
   branch starts with `lira/<number>-`:

   ```json
   {"type": "existing_worktree", "worktreePath": "<path>"}
   ```

   Otherwise, when the issue has a pull request, or
   `git ls-remote --heads origin 'lira/<number>-*'` finds a branch:

   ```json
   {"type": "worktree", "baseRef": "origin/<branch>",
    "startFromOrigin": true, "branch": "<branch>"}
   ```

   Otherwise, make `<words>` from two or three words of the title, in
   lowercase with hyphens between them:

   ```json
   {"type": "worktree", "baseRef": "origin/main",
    "startFromOrigin": true, "branch": "lira/<number>-<words>"}
   ```

   `message` is:

   ```text
   Your issue is #<number>. Run `git fetch origin main`, read your
   instructions with `git show origin/main:t3/agent.md`, and follow them.
   ```

   If the call fails, make it one more time with the same fields, and the
   same `clientRequestId`: do not run `date` again. If it fails again,
   remove the assignee with
   `gh issue edit <number> --remove-assignee @me`, so that the issue can
   start at the next run.

8. List the threads that you started, then stop.

Start each issue one time. Do not look at an agent after you start it, and do
not wait for one.

## What the user does

- To let an issue start, set `auto:ready` or `auto:merge` on it.
- To answer a question, write a comment and remove `needs:answer`.
- To send a pull request back, write a comment on it, and remove the assignee
  from its issue.
- To start an issue again after its thread stopped, remove its assignee.
