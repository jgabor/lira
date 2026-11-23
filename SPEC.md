# Lira specification

This document is the contract for Lira. [WORDS.md](WORDS.md) defines the words
it uses.

A sentence here is a promise, and each promise has a test that quotes it. A
paragraph, list item or table row that starts with *Proposed* is not decided
yet. One that starts with **Planned.** is for a later version. Neither is a
promise.

## Overview

The first version is a server and a command line on one host.

- The server does the steps for the issues of one or more repositories.
- GitHub is the only screen: you read issues and pull requests there.
- Lira uses your GitHub login.
- Each run and each chat has a git worktree on the host.

## Format of the Lira file

The Lira file is `lira.toml` at the root of a repository. It is the only
place where a repository sets something for Lira.

| Key | Type | Meaning |
|---|---|---|
| `check` | Text | The check command. It is required. |
| `setup` | Text | A command that Lira runs in each new worktree. |
| `protected` | List of text | The protected paths. |
| `agents` | Number | The agent limit for this repository. |
| `models.agent` | Text | The AI model for the agent. |
| `models.planner` | Text | The AI model for the Planner. |
| `models.reviewer` | Text | The AI model for the Reviewer. |
| `harness.program` | Text | The program of the harness. |
| `reviewer.harnesses` | List of text | The harnesses for the Reviewer, in order of preference. |
| `score` | List of tables | The scores. Each has a `name` and a `better`. |
| `planner.on` | True or false | If the Planner runs. |
| `planner.instructions` | Text | The path of the Planner's instructions. |

- Lira reads these forms of TOML in the Lira file: a key with a text, a number, `true` or `false`, or a list; a table; and a list of tables.
- Another form of TOML in the Lira file is an error.
- A Lira file with an unknown key is an error.
- A Lira file with no `check` is an error.
- The value of `better` is `higher` or `lower`.
- Two scores with the same name are an error.
- The value of `harness.program` is the name of a program, or an absolute path.
- A path in `protected` that ends with `/` is a folder, and it covers all that is in it.
- A path in `protected` is relative to the root of the repository.
- The agent limit of a repository can't be larger than the agent limit of the host.

Lira reads the Lira file from the main branch. It does not read it from the
branch of a change.

## Server

- The server is one program, and it is the same file as the command line.
- One server runs on a host.
- *Proposed:* The server listens on the socket file `$XDG_RUNTIME_DIR/lira/server.sock`.
- The server does not listen on the network.
- The data folder is `$XDG_DATA_HOME/lira`.
- When `XDG_DATA_HOME` is not set, the data folder is `~/.local/share/lira`.
- An `XDG_DATA_HOME` that is empty, or that is not an absolute path, counts as not set.
- The server keeps a numbered log of all messages and changes of state in one database file.
- Each entry in the log has a number that is one more than the number before it.
- The server keeps the worktrees in its data folder.
- When the server starts again, it continues from GitHub and from its log.
- *Proposed:* The agent limit of the host is 3.

## GitHub

- Lira uses the token of the GitHub command line, `gh`.
- Lira stops with an error that names `gh auth login` when `gh` has no login.
- An agent and a Reviewer have no login for GitHub.
- Lira asks GitHub for changes at a fixed interval, and after each of its own writes.
- Lira writes nothing to a repository that has no Lira file on its main branch, except the setup pull request.

## Issues

- An issue with no `auto:` label does not start.
- An issue with an open issue in its `Blocked by` line does not start.
- A parent issue does not start.
- An issue has at most one open pull request.
- When Lira starts an issue, it assigns the issue to its own GitHub login.
- Lira starts the oldest issue first.
- Lira does not start an issue while the number of agents that work is the agent limit.

## Pull requests

- Lira opens the pull request of an agent as a draft, at the first commit of the agent.
- The description of a draft pull request is the progress note.
- The progress note has the headings `Done`, `Next` and `Decisions`.
- The description of a pull request for an issue ends with the line `Closes #<number>.`.
- Lira marks a pull request ready for review only after the check and the Reviewer pass it.
- When a pull request is ready for review, its description has the headings `Problem`, `Change` and `Decisions`.
- *Proposed:* The branch of an issue is `lira/<number>-<two or three words of the title>`.

## States

| State | An issue is in it when | A chat is in it when |
|---|---|---|
| Inbox | Its pull request is ready for review, or it has `needs:answer`, or it has no `auto:` label | Its agent asked a question, or its agent stopped |
| Next | It has an `auto:` label, no pull request, and no open issue blocks it | Never |
| Now | An agent works on it, or its change is in the merge queue | Its agent works, or its change is in the merge queue |
| Later | An open issue blocks it, or you moved it to Later | You moved it to Later |
| Done | Its pull request is merged, or the issue is closed | You marked it done |

- An issue or a chat has one state.
- What you moved to Later returns to its state when its time comes, or when it changes.

## Steps

Lira does these steps for each issue, in this order.

1. Start.
2. Work.
3. Check.
4. Review.
5. Ready.
6. Merge.

- No AI model decides which issue starts.
- An agent works only in its own worktree.
- An agent can change no file of its harness on the host, other than the login of the harness.
- The harness of each agent keeps its conversations in a folder of its own, and Lira removes the folder when the agent ends.
- When Lira ends before it can remove that folder, Lira removes it when the next agent on the same host starts.
- When Lira can't lock the folder of a harness, it runs no agent.
- The harness of an agent reads no memory file of the host.
- In the sandbox of an agent, `~/.ssh`, `~/.gnupg`, `~/.netrc` and `~/.git-credentials` are empty, with no login for GitHub or for another site.
- In the sandbox, `SSH_AUTH_SOCK` and `DBUS_SESSION_BUS_ADDRESS` are not set, and a socket file that they name is not a socket there.
- In the sandbox, the folder of `XDG_RUNTIME_DIR` has no file of the host, other than the paths that Lira gives the sandbox.
- An `XDG_RUNTIME_DIR` that is not an absolute path counts as not set.
- When the host has no `bwrap`, Lira runs no agent, and the error names `bwrap`.
- When `bwrap` makes no sandbox, Lira runs no agent, and the error shows what `bwrap` wrote.
- When Lira can't start a `sh` that stops the harness if Lira ends, Lira starts no harness, and the error names `sh`.
- Lira runs the `setup` command in a new worktree before the agent starts.
- Lira runs the `setup` command with `sh -c`.
- When the `setup` command fails, Lira removes the new worktree and its branch, and the error shows what the command wrote.
- A new worktree starts from the newest main branch.
- A new worktree is the folder `worktrees/<owner>/<repository>/<branch>` in the data folder, except as `lira continue` says.
- When the check fails, Lira gives the output of the check to the agent.
- After 3 failed tries at the same check, the agent stops and asks.
- A Reviewer reads the change after the check passes.
- A Reviewer can't change a file.
- A Reviewer uses a different harness from the agent when the Lira file names one that is available.
- After 3 rounds of review with open findings, the agent stops and asks.
- When an agent stops and asks, Lira adds `needs:answer` to the pull request, or to the issue if there is no pull request.
- When you answer, Lira removes `needs:answer` and the work continues.
- With `auto:merge`, a change that is ready enters the merge queue.
- With `auto:ready`, a change that is ready waits for you.

## Check

- Lira runs the check command in the worktree of the change.
- Lira runs the check command with `sh -c`.
- Lira runs the check of an agent in the sandbox of the agent.
- The check passes when its exit status is 0.
- The check fails when its exit status is not 0.
- Lira sets `LIRA_SCORES` to the path of an empty file before it runs the check.
- Lira sets `LIRA_BASE` to the commit of the main branch that the change is compared with.
- *Proposed:* A check that runs longer than 30 minutes fails.

## Scores

- The check writes one line for each score to the file that `LIRA_SCORES` names.
- A score line is `<name> = <number>`.
- The number of a score is a decimal number, such as `12`, `-3` or `0.25`.
- A score line with a name that the Lira file does not have is an error.
- Two score lines with the same name are an error.
- A score that the check did not write has no value, and it is not compared.
- A score got worse when its value moved against its `better` from the value on the main branch.
- At merge, Lira writes the scores in a comment on the pull request.
- Lira reads the scores of the main branch from the comment on the last merged pull request.

## Protected paths

- A change that touches a protected path waits for you, also with `auto:merge`.
- A change that made a score worse waits for you, also with `auto:merge`.
- `lira.toml` is always a protected path.
- `.github/` is always a protected path.
- A pull request that waits for one of these reasons says the reason in a comment.

## Merge queue

- A change enters the merge queue when you decide the merge, or with `auto:merge` when it is ready.
- Lira merges one set of changes at a time.
- Lira puts all changes that wait in the queue on the newest main branch together, and runs the check once.
- When that check passes, Lira merges each change as one commit.
- When that check fails, Lira finds the change that breaks it, and merges the others.
- Lira gives a change that breaks the check back to its agent.
- When a change has a conflict with the main branch, Lira gives it back to its agent.
- A change that an agent had to edit after your decision waits for you again.
- A change that Lira only brought up to date does not wait for you again.

## Commands

Each command is one word after `lira`, and the words are fixed. Exit status 0
is success, 1 is a failure, and 2 is bad usage.

### lira --version

- It prints the version, and exits with status 0.

### lira init

- It makes a Lira file for the repository in the current folder, and opens the setup pull request.
- It proposes the check command from the files that it finds in the repository.
- It does not change the main branch.
- It is an error when the repository has a Lira file.

### lira server

*Proposed:* The commands for the server are a group.

- *Proposed:* `lira server install` installs the server as a service of your user. It does not start it.
- *Proposed:* `lira server install --run` installs the server and starts it.
- *Proposed:* `lira server up` starts the server.
- *Proposed:* `lira server down` stops the server.
- *Proposed:* `lira server status` says if the server runs, and names its socket file.
- *Proposed:* `lira server uninstall` removes the service. It keeps the data.

### lira run

- `lira run "<request>"` runs an agent on the request, in a new worktree of the repository in the current folder.
- The request needs no issue.
- The new worktree starts on a branch `lira/run-<8 letters or digits>`.
- The instructions tell the agent to give the branch a name that says what the work is, such as `fix/login-timeout`.
- When the remote has a branch with the name that the agent chose, Lira tells the agent to give its branch another name.
- *Proposed:* After 3 such messages with no new name, `lira run` fails.
- Lira opens a draft pull request at the first commit of the agent.
- The title of the pull request is the subject of the first commit.
- When the agent makes no commit, Lira opens no pull request, and `lira run` fails.
- Lira runs the check and a Reviewer on the change, and marks the pull request ready for review.
- When the agent stops and asks, Lira writes what stops the work as a comment on the pull request: what the last check wrote, or what the last Reviewer wrote.
- Then `lira run` fails, and says why Lira stopped.
- A change from `lira run` always waits for you.
- It prints the address of the pull request.

### lira continue

- `lira run` writes the request as the first comment of the draft pull request.
- `lira run` fails before an agent starts when the request is too long for a comment on GitHub.
- `lira continue <number>` starts a new agent on the branch of that pull request. The agent gets the request, what stopped the work, and your answer.
- Your answer is each comment that the login of `gh` wrote after the last comment of Lira, and that Lira did not write.
- A review with a text, and a comment on a line of the change, are comments too, in the order of their time.
- A comment on a line in a review has the time when the review was sent.
- A review that is not sent yet is not part of your answer.
- With a comment on a line, the agent gets its file and its lines, or only its file when the comment is on the whole file.
- When the lines are in an older version of the file, the agent is told so.
- With a reply on a line, the agent also gets the comment that it replies to, and each reply to that comment before it.
- When you wrote no answer, the agent is told so, and is told to read the newest commits of the branch.
- When you wrote no answer after a stop, the agent is told to try again.
- A comment from someone else is data, and the agent is told so.
- The command removes `needs:answer` when the pull request has it. It does not need the label: you can remove it yourself on GitHub.
- When the work of the new agent fails, the pull request gets `needs:answer` again.
- When the check and a Reviewer pass the change of the new agent, Lira writes a comment that says so.
- After a `lira continue` that passed, the next one gets only the answer that you wrote after it.
- After a `lira continue` that passed, the agent is told that the check and a Reviewer passed the change, and that you sent it back.
- After a `lira continue` that fails with no question, the next one gets the same question and the same answer again.
- When Lira can't write the comment that the work passed, `lira continue` still passes, and says so.
- When you wrote a comment while the new agent worked, Lira writes no comment that the work passed, and says so.
- When the pull request is ready for review, it becomes a draft again.
- The agent works in the old worktree when it is still there, and in a new one from the branch of the remote when it is not.
- When something that git does not list as a worktree is in the folder of the branch, the new worktree is that folder with `~2` at the end of its name, or with `~3` when that is there too, and so on.
- A commit that is on the remote and not in the worktree is taken first.
- A commit that is in the old worktree and not on the remote is pushed with the work of the new agent.
- When the old worktree is on another branch, or on no branch, Lira puts it back on the branch of the pull request.
- When the branch is in no worktree whose folder is there, Lira removes from `git worktree list` each worktree of Lira whose folder is gone.
- A worktree of yours stays in `git worktree list`, also when its folder is gone.
- The new agent has a new conversation, and it runs under the same rules as an agent of `lira run`.
- Under `Decisions`, the progress note has only what the new agent wrote.
- Lira then runs the check and a Reviewer as `lira run` does, with both limits at 0 again. The work can stop and ask again.
- The Reviewer gets the request and your answer.
- The Reviewer also gets each comment that you wrote after the request and before your answer.
- It is an error when the pull request is not open, or has no comment with the request.
- It is an error when the branch of the pull request is in another repository, or is checked out in a folder that is not a worktree of Lira.
- It is an error when the worktree and the remote both have commits on the branch that the other does not have.
- It is an error when the remote no longer has the branch of the pull request.
- It fails at once when an agent of Lira works on the pull request, and says so.
- Then the pull request, its labels and the worktree stay as they are.
- It does not wait for that agent, and it does not stop it.
- When Lira is killed while its agent works, a later `lira continue` starts a new agent.
- It prints the address of the pull request.

### lira check

- It runs the check command of the Lira file in the current folder, as the server runs it.
- It prints each score with its value, and its value on the main branch.
- Its exit status is 0 when the check passes, and 1 when it fails.

### lira inbox

- It lists each issue and chat that waits for you, with the reason.
- It prints nothing, and exits with status 0, when nothing waits.

### lira merge

- `lira merge <number>` puts the change of that pull request in the merge queue.
- It is an error when the pull request is a draft.

### lira answer

- `lira answer <number> "<text>"` writes your answer as a comment, and removes `needs:answer`.

### lira later

- `lira later <number>` moves that issue or chat to Later until it changes.
- `lira later <number> --until <time>` moves it to Later until that time.

### lira note

*Proposed:* An agent calls this command.

- *Proposed:* `lira note` makes the text on its input the progress note of the agent's pull request.
- *Proposed:* It pushes the branch first. It opens the draft pull request if there is none.

### lira ask

*Proposed:* An agent calls this command.

- *Proposed:* `lira ask "<question>"` writes the question as a comment, adds `needs:answer`, and stops the agent.

### lira submit

*Proposed:* An agent calls this command.

- *Proposed:* `lira submit` tells Lira that the agent's work is complete. Lira then runs the check and a Reviewer.

## Harnesses

- Lira runs an agent through the Agent Client Protocol (ACP).
- The harness is the program that `LIRA_HARNESS` names.
- When `LIRA_HARNESS` is not set, the harness is the program that `harness.program` in the Lira file names.
- When `LIRA_HARNESS` is not set and the Lira file has no `harness.program`, the harness is the program `claude-agent-acp`.
- Lira reads a login token for the harness from the file `lira/auth/claude-oauth-token` in `XDG_CONFIG_HOME`, or in `~/.config` when `XDG_CONFIG_HOME` is not set.
- An `XDG_CONFIG_HOME` that is empty, or that is not an absolute path, counts as not set.
- With that token, the harness gets the token in `CLAUDE_CODE_OAUTH_TOKEN`, and its data folder has no `.credentials.json` of the host.
- With that token, the `.credentials.json` of the host is empty in the sandbox.
- With no such file, the harness can renew the login in `.credentials.json` of the host.
- When Lira can't read a token from that file, Lira starts no harness, and the error names the file.
- In the sandbox, the file of the token is empty, in `XDG_CONFIG_HOME` and in `~/.config`.
- Lira gives the token to the harness, and not to the check.
- Lira writes the token in no file, no comment and no description, and it prints the token nowhere, other than in what an agent wrote.
- *Proposed:* The first harness is Claude Code.
- When a harness stops at a usage limit, Lira keeps the work in Now, and continues when the limit ends.

## Later versions

- **Planned.** Lira has its own GitHub App, and acts as that App.
- **Planned.** A desktop app shows the Inbox, the overview, issue pages, chats and the conversation with Lira.
- **Planned.** A phone app acts on the Inbox.
- **Planned.** The Planner writes issues at fixed times.
- **Planned.** You talk to Lira in one conversation for each repository.
- **Planned.** A chat has checkpoints, and you can restore one.
- **Planned.** You can send a message to an agent that works: now, or after it stops.
- **Planned.** Each agent runs in an Agent+ virtual machine.
- **Planned.** Lira Cloud runs the server for you.
