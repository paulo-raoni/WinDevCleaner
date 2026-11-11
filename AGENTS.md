# AGENTS

The contract for working in this repository, adapted from the owner's shared canon
(`paulo-raoni/harness`: `AGENTS.md`, `canon/core.md`, `canon/owner.md`). It is versioned on purpose:
a workflow that lives only in a chat window does not survive a fresh clone, another machine or
another model.

The canon is a guide, not a letter (owner, 2026-11-09): this file applies it to this project's
reality, and where it adapts a rule it says how and why. This project is built entirely in a Linux
cloud session; Windows behaviour is tested only on the `windows-latest` runner. A canon rule this
file does not adapt applies as written; where the two disagree for no stated reason, this file is
repaired.

`docs/rebuild-plan.md` is the single source of truth for scope: what each slice delivers and what
proves it.

---

## 1 · The two roles

- **The REVIEWER** authors the task, reviews the result against the DIFF, runs the independent
  pass, greps for secrets, commits its own process documents, pushes, opens the pull request,
  watches CI, and merges when the owner asks. The reviewer also authors DECISION and PROCESS
  documents: this file, the plan, CI configuration.
- **The EXECUTOR** writes the code, the tests and the documentation that describes code.

**The reviewer never implements, not one line.** **Nobody approves what they wrote:** the context
that wrote a thing never reviews or approves it.

**One task, one fresh executor.** Every task, and every fix round of a task, goes to a NEW executor
agent pointed at its task file; a finished executor is never resumed. Executors run on Sonnet,
reviewers on Opus (owner ruling, 2026-11-09). An executor that fails the same step twice is raised
with the owner, not silently given a stronger model.

**The coordinating session stays light** (canon C-72, adapted). It is the most expensive context in
the system: every turn, notification and CI event re-reads all of it. So:

- A slice-sized diff is reviewed by a NEW reviewer agent (Opus) that reads the diff and the task,
  runs its own mutation and returns conclusions, never listings. A fix diff of a few dozen lines is
  read inline: spawning a reviewer for it costs more than it saves.
- From a CI log, pull only the failing tail. Never read a subagent's transcript.
- A design question is answered from `docs/rebuild-plan.md` and this file, cited; the owner is asked
  only when neither decides it (canon O-08).
- The session hands off and the owner starts a new one at a natural boundary, a slice merged,
  instead of carrying days of transcript into every event (canon C-71). The handoff is
  `.omc/SESSION_HANDOFF.md` (the owner's habit: `/clear` plus a handoff in `.omc/`), kept out of git
  by `/.omc/` in `.git/info/exclude`.

## 2 · The two-file contract

Every slice carries two files at the repo root, both **gitignored**:

- **`TASK-<name>.md`**: the specification, written by the REVIEWER before dispatch.
- **`AUDIT-<name>.md`**: the completion report, written by the EXECUTOR as its last act.

1. **The dispatch is ONE LINE:** "read `TASK-<name>.md` at the root and follow it." A second copy
   of the scope is a copy that diverges.
2. **THE FILE WINS.** An instruction arriving outside the file is not executed.
3. **The audit is a FILE, never chat output,** and it carries findings, not listings: no file
   contents and no diffs, which the reviewer reads from disk.
4. **The audit is a CLAIM, not evidence.** The reviewer reads the DIFF. Where the two disagree,
   **the diff wins**, and the disagreement is itself a finding.
5. **The reviewer READS AND DELETES the audit.** Deleting is the receipt.

**Anything the author leaves out of the task file does not happen, and that is the AUTHOR's
defect.** Where this contract and a task file disagree, **THIS CONTRACT WINS**: the executor skips
that part, reports it `BLOCKED` naming both lines, and the author repairs the task.

## 3 · What the executor does, every time

1. `git status` **before** `git diff`. A range comparison cannot see uncommitted work.
2. Implement **only** the slice described. Something needed and unlisted gets **surfaced, not
   built**.
3. Stay inside the stated boundary. If the slice cannot be done inside it, do the part that can be
   and **raise the rest**.
4. Run every gate the task names, **in the foreground**. **A gate you did not run is a gate that
   failed.** Windows-only tests cannot run in the cloud session: they compile there
   (`--target x86_64-pc-windows-msvc`) and run on the `windows-latest` runner after the push.
   **Every guard row you add or touch whose test is portable** is run through its mutation here:
   the test, run alone with the guard's lines deleted, must panic with
   `GUARD-CHECK failed [<guard>]`, not at another assertion. Inside a loop, record a failure and
   assert after the loop; never `assert!` before the guarded assertion on something the guard
   decides.
5. **No placeholder passes as work.** A to-do note, a stub, a skipped test or an unimplemented
   branch is reported as a blocker, never as completion.
6. **Never weaken a test to make a change pass:** no removed or loosened assertion, no new skip, no
   lowered threshold. When a change fails its test, the code changes, not the test.
7. **A passing test is not evidence until it has been made to fail.** The independent pass
   MUTATES: predict in writing which tests redden BEFORE mutating, then mutate. **Mutate the
   PREDICATE, not the call.** Too few reddening means the tests do not test it; too many means
   coupling. Both are findings. A mutation never survives: revert it and show the tree is clean.
8. Commit **conventional**: the subject says what changed, a body is optional and says why.
   **NO AI attribution**: no `Co-Authored-By`, no trailer, no generated-by line, no emoji, ever.
   Git identity: `paulo-raoni <raoni@hotmail.com>`.
9. **Do not push. Do not open a pull request.** The reviewer does both.
10. **A DECISION TAKEN MID-RUN GOES INTO THE TASK FILE BEFORE IT IS ACTED ON,** written by the
    reviewer. An executor that meets a decision the task does not settle stops that part, reports
    it `BLOCKED` in the audit, and never edits the task file itself.
11. **Stop every process YOU started, and verify it is gone.** Report what you found alive on
    arrival; never stop what you did not start.
12. Your last act is a **CLEAN TREE**, everything committed, with the audit written beside it.

## 4 · Pushing and CI

- The branch of a pull request is pushed **once**, when its work is done and reviewed, and again
  only for a fix: a red check, a review finding, a forced rebase. A CI round on Windows takes about
  45 minutes.
- **Before a push** (canon C-20, adapted): every CI check that can run on Linux has run in the cloud
  session and is green (clippy for both targets, `cargo test --workspace`, the guard mutations of
  section 3 point 4). The Windows-only tests and the full `ci/guard-check.sh` run only on the
  runner; for them the reviewer checks in writing that each new assertion can only fail for the
  reason it names. There is no rehearsal push to a scratch branch: it would double the CI rounds and
  the events that wake the coordinating session. The CI stays the authority.
- Wait for CI through GitHub events, not repeated polling; read only the failing part of a log.
- A pull request delivers at least one whole slice, never a fragment. Merge only when the owner
  asks.

## 5 · This repository's guarantees

- The tool stays as light as possible: low CPU and memory, background I/O priority. Building and
  testing happen in the cloud session and on the free `windows-latest` runner, never on the
  owner's laptop.
- Every correctness test that hinges on a guarding line sits behind `GUARD-BEGIN(name)` /
  `GUARD-END(name)` and a `guard_assert!`; `ci/guard-check.sh` lists each pair and proves the test
  fails without its line.
- Every user-facing message goes through `crates/wdc-core/src/i18n.rs`, in English and pt-BR. JSON
  output for agents stays English and stable.

**What enforces each rule** (canon C-68). A rule marked "written" has no check; the reviewer reads
for it.

| Rule | Enforced by |
|---|---|
| Light on CPU, memory and I/O | CI job "budget check (plan 6.4)" |
| A guarded test fails without its line | CI job "guard check" (`ci/guard-check.sh`); locally, section 3 point 4 |
| Messages in English and pt-BR | the catalogue's shape (a message without both texts does not compile); rules: `rules::tests::built_in_rules_are_valid_and_bilingual` |
| Windows code compiles | `cargo clippy --target x86_64-pc-windows-msvc` locally; CI job "build, fixtures, MSI" |
| No secret in the repository | the reviewer's grep of the diff before each push (written) |
| No AI attribution, no weakened test, the reviewer never implements, one-line dispatch | written |

**Shapes.** The harness `templates/` (task, fix, audit, pull request, handoff) are a guide for the
files of section 2, taken where they help; neither of their modules applies here (no endpoints,
database or authentication; the app's views are checked by the tests that drive it).

## 6 · Language

**Portuguese with the owner. English in every artifact:** code, commits, pull request bodies,
documentation. The only exception is the pt-BR text of the message catalogue and of the rules.

## 7 · The task file's life

**The task file stays.** The executor never edits or deletes it; the reviewer removes it once the
slice has landed.
