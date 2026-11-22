# Lira

Lira runs AI agents on the issues of your GitHub repositories. You write an
issue and set a label. An agent does the work in its own worktree and opens a
pull request. Lira runs the check of the repository, a Reviewer reads the
change, and the pull request waits for your review, or Lira merges it.

**Status: in development.** There is no release yet. [SPEC.md](SPEC.md) says
what the first version does.

## How it works

- The first version is a server and a command line on one host.
- GitHub is the only screen. You read issues and pull requests there.
- Lira uses your GitHub login, through the GitHub command line, `gh`.
- Lira runs on Linux, and needs `bwrap` (bubblewrap). Each agent works in a
  sandbox, where it can change its own worktree, and not your other files.
  The check of its change runs in the same sandbox.
- A repository sets itself up for Lira with one file, `lira.toml`. It names
  the check: the one command that says pass or fail for a change.
- The check can print scores. A change that makes a score worse waits for
  you.

## Labels

You decide how far an agent can go with a label on the issue.

| Label | Meaning |
|---|---|
| `auto:ready` | An agent can work until the pull request is ready for your review. |
| `auto:merge` | An agent can work until the pull request is merged. |
| `needs:answer` | An agent asked a question. The work waits for your answer. |

An issue with no `auto:` label does not start.

## Commands

| Command | What it does |
|---|---|
| `lira init` | Makes a Lira file for the repository, and opens the setup pull request. |
| `lira run "<request>"` | Runs an agent on a request, from the request to a pull request that is ready for review. |
| `lira continue <number>` | Continues the work on a pull request of `lira run` after your answer in a comment. |
| `lira check` | Runs the check command of the Lira file, as the server runs it. |
| `lira inbox` | Lists each issue and chat that waits for you, with the reason. |
| `lira merge <number>` | Puts the change of a pull request in the merge queue. |
| `lira answer <number> "<text>"` | Writes your answer to the question of an agent. |
| `lira later <number>` | Moves an issue or chat to Later. |

## Documents

- [SPEC.md](SPEC.md) is the contract for Lira.
- [WORDS.md](WORDS.md) defines the words that Lira uses.
- [AGENTS.md](AGENTS.md) is the rules for agents that change Lira.

## Licence

Lira has the [Apache License, Version 2.0](LICENSE).
