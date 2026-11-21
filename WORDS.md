# Words

Each thing has one word, and each word has one meaning. Use these words in the
code, in the documents, on the screens, and in text for agents.

## Rules

1. One thing has one word. One word means one thing.
2. A GitHub or git thing keeps its GitHub or git word: repository, issue, pull
   request, label, draft, merge, worktree.
3. A name that people in this field already use is better than a new plain
   phrase: harness, host, checkpoint, subagent, inbox.
4. No new figures of speech. A new word says what the thing is or does.
5. A state is one short word. A button is a verb, or a verb and a noun.
6. The person is "you". An agent with a fixed job has the name of that job.
7. A label is `group:value`. Each group answers one question.
8. Sentences are short and active.

## Things

| Word | Meaning |
|---|---|
| Lira | The whole product: the server and its apps. Also the one conversation for each repository where you talk to Lira. |
| Repository | A GitHub repository that you added to Lira. |
| Issue | A request that is written down so that it can wait. It is a GitHub issue. |
| Parent issue | An issue that has sub-issues. It has no pull request of its own. |
| Pull request | One change. An issue has at most one open pull request. |
| Chat | A conversation between you and an agent that has no issue. |
| Run | One period of work by an agent on an issue. Runs have numbers: run 1, run 2. |
| Check | The one command that a repository supplies. It says pass or fail for a change. |
| Score | A number with a name that the check prints. A repository says if higher or lower is better. |
| Score history | The scores after each merge. Lira writes them on the pull request. |
| Protected path | A file or folder that the Lira file names. A change to it always needs your review. |
| Merge queue | The changes that wait to be merged, in order. Lira brings them up to date, checks them and merges them without you. |
| Progress note | The description of a draft pull request. It says what is done, what is next, and what the agent decided. |
| Checkpoint | The files of a worktree as they were after one agent reply. You can restore it. |
| Worktree | A separate set of the repository files. Each run and each chat has its own. |
| Lira file | The file `lira.toml` in a repository. It sets the repository up for Lira. |
| Setup pull request | The pull request that adds the Lira file to a repository. |
| Instructions | The text that tells an agent how to work. It is part of Lira. |
| Rules | The text that a repository adds for agents, in its `AGENTS.md`. |
| Agent limit | The largest number of agents that can work at one time. |
| Usage limit | The point where a harness stops because your plan has no more use left. |
| Overview | The part of the repository screen that shows all issues and chats in five columns, one for each state. |
| Path | The line from a repository to what you look at: repository, parent issue, issue, run, subagent. |
| Review guide | Notes on a pull request that say where to look closely and which questions to ask. |

## People and programs

| Word | Meaning |
|---|---|
| You | The person who uses Lira. Instructions for agents say "the user". |
| Agent | A program that does work with an AI model. With no other name, it is the agent that does an issue or works with you in a chat. |
| Planner | The agent that writes issues for the work that is not done. |
| Reviewer | The agent that reads a change and says if it is good. |
| Subagent | An agent that another agent starts for a part of its work. |
| Harness | The software that runs an agent, for example Claude Code. |
| Server | The program that runs the agents and holds the repository files. |
| Host | The one computer that runs the server. |
| App | What you use: the desktop app, the phone app, or the command line. |

## States

| Word | Meaning |
|---|---|
| Inbox | It waits for you. |
| Next | The issue can start. No agent works on it yet. |
| Now | An agent works on it, or it is in the merge queue. |
| Later | It comes back without you: you moved it there until a time, or another issue blocks it. |
| Done | The pull request is merged, the issue is closed, or you marked the chat done. |

A state belongs to an issue or to a chat. There is no one word for the two:
write "issue or chat".

## Actions

| Word | Meaning |
|---|---|
| Start | Lira gives an issue to an agent, and assigns the issue to itself on GitHub. |
| Stop and ask | An agent cannot continue. It writes its question and stops. |
| Answer | You reply to a question. The work continues. |
| Review | Read a change and decide if it is good. A Reviewer does it first, then you. |
| Merge | You decide that a change goes in. It enters the merge queue, and Lira does the rest. |
| Send back | Return a pull request to the agent with a comment. It becomes a draft again. |
| Move to Later | Hide an issue or a chat from the Inbox until a time, or until it changes. |
| Mark done | Close a chat. |
| Create issue | Write a request from a chat so that an agent can do it later. |
| Create pull request | Run the check on the change of a chat, have a Reviewer read it, and open its pull request. |
| Restore checkpoint | Return a chat and its files to a checkpoint. |
| Send now | Stop the agent and give it your message. |
| Send after | Give your message to the agent when it stops. |

## Labels

| Label | Meaning |
|---|---|
| `auto:ready` | An agent can work until the pull request is ready for your review. |
| `auto:merge` | An agent can work until the pull request is merged. |
| `needs:answer` | An agent asked a question. The work waits for your answer. |

An issue with no `auto:` label does not start. Only you set an `auto:` label.
"Draft" and "ready for review" are GitHub's own states for a pull request, so
they need no label.

## Words we do not use

| Do not write | Write |
|---|---|
| Owner, user (to the person) | You |
| Project | Repository |
| Work item, task, item | Issue, or "issue or chat" |
| Thread | Chat |
| Attempt, session | Run |
| Worker, builder | Agent |
| Helper | Subagent |
| Gate | Check |
| Hill | Score |
| Ledger | Score history |
| Yours, needs you | Inbox |
| Todo, doing | Next, Now |
| Lane, board | State, overview |
| Snooze | Move to Later |
| Settle | Mark done |
| Park | Stop and ask |
| Land | Merge |
| Ship | Create pull request |
| Promote | Create issue |
| Dispatch, claim | Start |
| Slot | Agent limit |
| Brief | Instructions |
| Repo file, project file | Lira file |
| Bootstrap PR | Setup pull request |
| Onboarding | Setup |
| Main computer, machine, environment | Host |
| Client | App |
| CLI | Command line |
| Provider, runtime, agent program | Harness |
| Saved point | Checkpoint |
| Queue, steer (for messages) | Send after, send now |
| Home, project chat | Lira |
| Lineage | Path |
| Refresh, update branch | Merge queue |
