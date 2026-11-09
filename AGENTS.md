# AGENTS

The contract for working in this repository, adapted from the owner's shared canon
(`paulo-raoni/harness`: `AGENTS.md`, `canon/core.md`, `canon/owner.md`). It is versioned on purpose:
a workflow that lives only in a chat window does not survive a fresh clone, another machine or
another model. Where this file and the canon disagree, the canon wins and this file is repaired.

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
  35 minutes.
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

## 6 · Language

**Portuguese with the owner. English in every artifact:** code, commits, pull request bodies,
documentation. The only exception is the pt-BR text of the message catalogue and of the rules.

## 7 · The task file's life

**The task file stays.** The executor never edits or deletes it; the reviewer removes it once the
slice has landed.
