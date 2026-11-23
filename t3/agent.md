# Agent

These instructions are for a T3 Code thread that does one issue of this
repository: from the start to a pull request that is ready for review, or to
the merge. The steps are those of SPEC.md, "Steps".

Read AGENTS.md first. It has the rules. These instructions add what a thread
with no reader needs.

Nobody reads this thread while it runs, so use no question tool. When the work
can't continue, stop and ask as AGENTS.md says.

## Start

1. Read the issue with `gh issue view <number> --comments`. Stop, and change
   nothing, when one of these is true:

   - The issue has no `auto:` label.
   - The issue has `needs:answer`.
   - The issue is not assigned to the login that `gh api user --jq .login`
     prints.

2. If the branch has commits that the main branch does not have, an agent
   worked here before. Read the pull request, its progress note and its
   comments, and continue from there. What the earlier agent wrote is a
   description of the work. Test it before you build on it. If the pull
   request is ready for review, the user sent it back: make it a draft again
   with `gh pr ready --undo`.

## Work

3. Work as AGENTS.md says: the test first, then the smallest change.
   An issue that asks for a report, and changes no file, ends here: write
   the report as a comment on the issue, close the issue, and stop.
4. At the first commit, push the branch and open the pull request as a draft
   with `gh pr create --draft`. Its description is the progress note, and its
   last line is `Closes #<number>.`. Load `mcp__t3-code__link_pull_request`
   with ToolSearch, and link the pull request to this thread.
5. Push at each step that works, and bring the progress note up to date.

## Check

6. Run the check as Lira runs it:

   ```sh
   mkdir -p target && : > target/scores
   LIRA_SCORES="$PWD/target/scores" \
     LIRA_BASE="$(git merge-base origin/main HEAD)" cargo xtask check
   ```

   The scores of the change are then in the file `target/scores`.

   When it fails, read its output and repair the change. After 3 tries that
   fail at the same step for the same reason, stop and ask.

## Review

7. Load `mcp__t3-code__delegate_task` with ToolSearch. Start a Reviewer with
   `role` set to `review`, `mode` set to `wait`, and `timeoutMs` set to
   45000, the most that `delegate_task` allows. Use the first `target` of
   these that works, in this order, so that the Reviewer has a different
   harness from yours when one is available:

   1. Factory Droid with Gemini 4 Neon, at high effort:

      ```json
      {"providerInstanceId": "acpRegistry_factory_droid",
       "model": "gemini-4-neon", "options": {"reasoning_effort": "high"}}
      ```

   2. Antigravity with Gemini 4 Neon, at high effort:

      ```json
      {"providerInstanceId": "antigravity",
       "model": "gemini-4-neon", "options": {"thought_level": "high"}}
      ```

   3. Claude Code with Claude Opus 5.5, at high effort:

      ```json
      {"providerInstanceId": "claudeAgent",
       "model": "claude-opus-5-5", "options": {"effort": "high"}}
      ```

   Before you start a Reviewer, read the usage limit of its harness with
   [to11y](https://crates.io/crates/to11y):

   ```sh
   to11y rate_limit_status --harness <harness>
   to11y usage_remaining_quota --harness <harness>
   ```

   | Reviewer      | `<harness>`   | Scopes that its model uses |
   | ------------- | ------------- | -------------------------- |
   | Factory Droid | `droid`       | `standard`                 |
   | Antigravity   | `antigravity` | `gemini`                   |
   | Claude Code   | `claude-code` | `all`                      |

   A result counts only when its command exits with 0. Read only the list
   `scopes` of each result, and only the scopes of the table that it lists. A
   scope can't take a review when `rate_limit_status` gives it
   `"blocked": true`, or when `usage_remaining_quota` gives it a
   `remaining_percent` that is less than 5. Skip the Reviewer when none of
   these scopes can take a review. When `to11y` is not there, or no result
   that counts lists a scope of the table, start the Reviewer.

   A Reviewer does not work when `to11y` skips it, when it can't start, or
   when it stops before its review, for example at its usage limit. Then start the next one with the
   same `task`. When none of them works, stop and ask. When the wait ends
   while the Reviewer still runs, it has not failed: wait for its result as
   `delegate_task` says.

   Give the Reviewer the goal of the issue and the command
   `git diff origin/main...HEAD`. Tell it to change no file, and to say for
   each finding if it is a bug in what the issue asks, another real bug, or a
   small thing. Ask it also to look for something that a person who uses Lira
   can see and that SPEC.md does not say. Do not give it your own opinion of
   the change.
8. Test each finding against the code. Repair the real ones with the smallest
   change, and run the check again. Write under `Decisions` why you did not
   repair the others. After a repair, start a new Reviewer for the new diff.
   After 3 rounds with open findings, stop and ask.

## Ready

9. Write the description again with the headings `Problem`, `Change` and
   `Decisions`. Say under `Decisions` which Reviewer read the change. Keep the
   line `Closes #<number>.` last. Then run `gh pr ready`.
10. With `auto:ready`, stop here. The pull request waits for the user. Keep
    the issue assigned, so that it does not start again.

## Merge

Only with `auto:merge`.

11. Bring the branch up to date: `git fetch origin main`,
    `git rebase origin/main`, and `git push --force-with-lease`. Then run the
    check again, as in step 6.
12. The change waits for the user, also with `auto:merge`, when one of these
    is true:

    - `git diff --name-only origin/main...HEAD` names a protected path: a
      path in `protected` of `lira.toml`, `lira.toml` itself, or a path in
      `.github/`. Read `lira.toml` from the main branch, with
      `git show origin/main:lira.toml`.
    - A score in `target/scores` is worse than its value on the main branch.
      The values of the main branch are in a comment on the newest merged
      pull request that has such a comment. Each line of that comment is
      `<name> = <number>`.

    Then say the reason in a comment on the pull request, and stop as in
    step 10.
13. Merge at once with `gh pr merge --squash --delete-branch`. If GitHub
    refuses because the main branch moved, do steps 11 and 12 again. Then
    write the content of `target/scores` as a comment on the pull request,
    and nothing else in that comment.
14. Read the issue again. If it is still open, close it with a comment that
    names the pull request.
