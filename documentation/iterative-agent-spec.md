# The v0.2 iterative agent — specification, gap audit and acceptance criteria

Status: **specification adopted 2026-11-10; implementation in progress.** This page is the contract
the v0.2 build is measured against. The status table at the bottom says what is implemented today;
everything else on this page is a commitment, not a capability. The governing product definition
remains [product-charter.md](product-charter.md); the implemented first slice remains documented in
[agent-workflow.md](agent-workflow.md) and keeps working unchanged.

## 1. What v0.2 is

The first slice (`aporia investigate`) runs a fixed two-phase workflow: the model proposes a plan,
a person approves it, the host executes the approved task list, and the model may only interpret the
recorded results. Follow-up experiments are suggestions. That is a honest slice, but it is not the
product: the charter's agent is supposed to examine a result and decide what to do next.

v0.2 replaces the fixed plan executor with an **iterative, supervised, persistent investigation
loop**, exposed as a terminal workbench (`aporia agent <project-dir>`). The agent inspects a real
project through real tools, plans, asks approval, executes, examines results, proposes follow-up
work, edits its sandbox copy where justified, verifies the edits, compares before and after, and
delivers a report plus a prioritised experiment backlog — across several cycles in one session that
survives interruption.

The Trust Atlas instrument becomes one tool among others, behind the same registry, with its
process boundary and replayable archive contract preserved. Historical measurements are immutable.

## 2. Gap audit of the starting state (verified at `8095e33`)

| area | what exists | what is missing |
|---|---|---|
| orchestration | separate planning and interpretation loops; host executes a fixed validated task order | a unified loop where a tool result can change the next action |
| tools | five hardcoded schemas, dispatch by name-match | a host-owned registry: typed schemas, validation, risk classes, permissions, limits, structured results, error classes, invocation identity |
| inspection | one bounded scan into numbered facts; `read_file` capped at 32 KiB | file listing, content search, line ranges, dependency/configuration/test/experiment discovery during the investigation |
| execution | argv-only exec in the copy, scrubbed environment by name policy, timeout on the direct child | working-directory choice, explicit environment allowlist, cancellation, artifact registration, honest process-tree limits |
| editing | none | patch representation and validation, atomic application, change tracking, diffs, rollback, authorised export |
| experiments | `suggest_experiment` records prose | a persistent backlog with statuses, executable specifications, approval, execution, comparison |
| persistence | hash-chained journal, manifest written last, one process per investigation | session identity, checkpointing, crash-safe resume that never blindly re-runs an interrupted operation, list/show/status |
| provider | scripted transcript; plain-HTTP OpenAI-compatible client without TLS | HTTPS via a maintained TLS client, credential handling that never leaks into records, bounded retries, contract tests |
| application | batch `aporia investigate` | an interactive workbench with conversational input, approval control, progress, pause/resume |
| demonstration | instrument-only projectile transcript | a code-driven project with a real defect, exercised end to end through the actual tool loop |

## 3. Architecture decisions

1. **Beside, not instead of.** The v0.2 engine is new modules in `aporia-agent` sharing the
   provider boundary, journal, sandbox and inspection with the first slice. `aporia investigate`
   and its record contract are preserved; existing tests and archives stay green.
2. **One tool registry.** Every capability the model can use — inspection, execution, the
   instrument, experiments, editing — is a registered tool with a stable name, a typed argument
   schema, a risk class, required permissions and execution limits. The schemas the model receives
   are generated from the registry. Every call passes host validation and the permission check; a
   command written in prose is not a command that runs.
3. **One bounded loop.** The engine asks the model, validates and dispatches each requested call,
   returns every result with its call id, and repeats until the model finishes, a budget runs out,
   the user cancels, or approval is needed. Tool failures are returned to the model as failures.
4. **Structured approval.** The initial plan carries an explicit scope: permitted executables,
   workspace write permission, budgets. Read-only inspection and already-approved operations do
   not re-ask. A plan revision that materially expands the scope requires a new recorded approval.
   No silent scope expansion.
5. **The journal stays authoritative.** Session state is reconstructible from the hash-chained
   journal. Resume reconstructs the transcript; an invocation recorded as started but never
   completed is reported to the model as interrupted, not re-executed.
6. **Editing is a lifecycle.** Inspect → propose change → validate → apply atomically in the
   sandbox → test → compare → accept or revert → present diff. The source project is never written;
   export to any user-controlled location requires explicit authorisation.
7. **UI-free core.** The engine emits progress events and pauses for decisions; the terminal
   workbench renders them. A later graphical frontend must not require engine changes.
8. **Honest boundaries.** The sandbox is copy isolation with path containment, not an OS jail.
   Documentation states exactly what is and is not isolated. No network-isolation claim, no
   live-model claim without a tested endpoint, no universal-language claim.

## 4. End-to-end workflow specification

The workflow a complete v0.2 session must support:

1. **Open.** `aporia agent <project-dir>` creates a persistent investigation: identity, journal,
   sandbox copy with per-file digests, baseline snapshot identity.
2. **Objective.** The user describes the goal conversationally; further context can be added at
   any pause.
3. **Inspect.** The agent uses bounded read-only tools to understand the actual project: files,
   code, configuration, dependencies, tests, experiment definitions, results, source revision.
4. **Plan.** The agent proposes gaps (each citing recorded evidence) and ordered tasks, with an
   explicit scope, budgets, risks and success criteria. The host validates every reference.
5. **Approve.** The user sees the plan and approves or refuses the scope. Refusal ends execution
   honestly; the record keeps the proposal.
6. **Iterate.** The agent executes approved operations, examines each result, and chooses the next
   operation: further inspection, an experiment, a targeted test, a code change. New questions
   become backlog items or plan revisions; material scope expansion re-asks approval.
7. **Experiment.** An experiment has a question, method, variables, baseline, metrics and success
   criteria. With an available runner and approval it executes; without one it stays a proposal.
   Outcomes are completed, failed or inconclusive — never invented.
8. **Improve.** A justified change is applied to the sandbox copy through the patch lifecycle,
   tested, and compared against the baseline and the objective. Compiling is not improvement.
9. **Interrupt safely.** The user can pause, cancel, or the process can die; the record stays
   consistent and the investigation can be resumed without repeating completed work.
10. **Deliver.** A host-generated report from structured events: what was inspected, understood,
    found, approved, executed, changed, tested, improved, left uncertain, and what to do next —
    with workspace/artifact locations and reproduction instructions. Machine-readable alongside
    the human-readable form.

## 5. Acceptance criteria

v0.2 is done only when, through the actual application:

- [ ] a user opens a supported code-driven project and starts an interactive session;
- [ ] the agent inspects the real project with real tool calls and produces evidence-backed gaps;
- [ ] the user reviews and approves an ordered plan with an explicit scope;
- [ ] the agent executes approved tools in the sandbox and visibly reacts to results with
      justified follow-up actions (not a scripted report);
- [ ] scope expansion triggers a new approval; all decisions are recorded;
- [ ] at least one experiment is created, approved, executed and its outcome recorded;
- [ ] at least one code change is applied in the workspace, tested, and compared before/after;
- [ ] an interrupted investigation resumes without corrupting the record or re-running completed
      operations;
- [ ] the full history, evidence, changes and outcomes are inspectable after the session;
- [ ] a patch/deliverable export happens only with explicit authorisation;
- [ ] the final report includes a prioritised backlog of further experiments;
- [ ] one complete project workflow is demonstrated end to end offline through the real registry,
      loop, execution, file changes, tests and records;
- [ ] `aporia investigate`, the instrument, the archives and all historical results still work
      and verify unchanged;
- [ ] live-model support is either tested against a real endpoint or explicitly reported untested.

## 6. Implementation status

Updated as milestones land; a row is checked only when its tests and gates pass.

| milestone | state |
|---|---|
| A — foundation: session/event types, permissions, record contract | done (`session.rs`, `scope.rs`, `events.rs`, the versioned checkpoint) |
| B — tool registry and execution foundation | done (`tools/`: registry, schema validation, dispatch, invocation ledger, error classes, output limits) |
| C — project intelligence tools | done (file listing, literal search, ranged reading, discovery, read-only source Git) |
| D — execution and scientific tools | done (`run_command`, `instrument_run`, result reading and comparison) |
| E — iterative agent loop | done (`engine.rs`: one loop, plan versions, scope approval, findings, backlog) |
| F — sandbox editing and patch lifecycle | partial (create, modify, delete, revert and change tracking; no unified-diff export) |
| G — persistence and interactive application | partial (`aporia agent` with the conversation commands; session resume and list are not yet implemented) |
| H — provider, demonstration, release audit | partial (the offline demonstration is real and tested; HTTPS provider support and the live smoke path are not implemented) |