# The v0.2 iterative agent — specification, gap audit and acceptance criteria

Status: **specification adopted 2026-11-10; the acceptance criteria below are met as of
2026-11-27 except the live-model test, which remains unverified (no endpoint or credential was
available on the build machine).** This page is the contract the build is measured against. The
status table at the bottom says what is implemented today; the governing product definition
remains [product-charter.md](product-charter.md); the implemented first slice remains documented in
[agent-workflow.md](agent-workflow.md) and keeps working unchanged. The application's own contract
is [agent-workbench.md](agent-workbench.md).

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

- [x] a user opens a supported code-driven project and starts an interactive session
      (`aporia agent <project-dir>`, with the conversation commands);
- [x] the agent inspects the real project with real tool calls and produces evidence-backed gaps
      (the demonstration's gap cites a recorded fact);
- [x] the user reviews and approves an ordered plan with an explicit scope
      (the plan carries its scope; the approval is journalled);
- [x] the agent executes approved tools in the sandbox and reacts to results with justified
      follow-up actions (the demonstration measures, fixes, measures again and keeps a backlog);
- [x] scope expansion triggers a new approval; all decisions are recorded;
- [x] at least one experiment is created and kept in the backlog with its method, evidence and
      success criteria — an experiment that did not run stays a proposal;
- [x] at least one code change is applied in the workspace, with the digests before and after,
      and the original project is untouched;
- [x] an interrupted investigation resumes without corrupting the record or re-running completed
      operations (`aporia agent resume <dir>` reconstructs the transcript, the investigation state
      and the invocation ledger from the hash-chained journal; a started-but-unfinished call is
      reported as interrupted, a requested-but-never-started call runs once; a broken chain and a
      finished record are named refusals; tested at engine and binary level, including a full
      workflow interrupted mid-edit and resumed to completion);
- [x] the full history, evidence, changes and outcomes are inspectable after the session
      (`aporia agent list`, `aporia agent report`, the journal);
- [x] a patch export happens only with explicit authorisation (the `export_changeset` tool prepares
      the unified diff inside the record and raises a `ChangesetExport` approval; the write to the
      user's destination happens only after the recorded decision. `aporia agent export <dir>
      --out FILE` asks the person directly. The patch round-trips through `git apply` onto a
      pristine copy; a conflicting target is refused; a refused export writes nothing; the source
      project is never a valid destination; binary changes are named, not silently dropped);
- [x] the final report includes a prioritised backlog of further experiments;
- [x] one complete project workflow is demonstrated end to end offline through the real registry,
      loop, execution, file changes, tests and records (`software/examples/agent-demo`);
- [x] `aporia investigate`, the instrument, the archives and all historical results still work
      and verify unchanged;
- [ ] live-model support is tested against a real endpoint (**unverified**: no provider credential
      or live endpoint was available on this machine. What *is* tested: the HTTPS provider runs
      real TLS handshakes and full agent sessions against a local TLS endpoint with a test-only
      certificate — wire shape, bearer credentials from a named environment variable, redaction of
      echoed keys, bounded retries, and a scan proving the key never reaches the record. A live
      vendor endpoint has not been exercised, and no live-model quality claim is made).

## 6. Implementation status

Updated as milestones land; a row is checked only when its tests and gates pass.

| milestone | state |
|---|---|
| A — foundation: session/event types, permissions, record contract | done (`session.rs`, `scope.rs`, `events.rs`, the versioned checkpoint) |
| B — tool registry and execution foundation | done (`tools/`: registry, schema validation, dispatch, invocation ledger, error classes, output limits) |
| C — project intelligence tools | done (file listing, literal search, ranged reading, discovery, read-only source Git) |
| D — execution and scientific tools | done (`run_command`, `instrument_run`, result reading and comparison) |
| E — iterative agent loop | done (`engine.rs`: one loop, plan versions, scope approval, findings, backlog; every tool result delivered, refused expansions continue the session, a third identical failing call is refused, the conversation is bounded while the journal is not, and a nearly spent budget tells the model to converge) |
| F — sandbox editing and patch lifecycle | done (create, modify, delete, revert with correct before/after identities, change tracking, unified-diff export through `diff.rs` and `tools/export.rs`, authorised writes, `git apply` round-trip tests) |
| G — persistence and interactive application | done (`aporia agent` with the conversation commands including `/pause` and `/findings`; checkpoints after every step; `resume`, `status`, `list`, `report`, `export` verbs; crash-safe resume in `resume.rs`) |
| H — provider, demonstration, release audit | done except the live-model test (HTTPS via `native-tls` with credential redaction and bounded retries, contract-tested against a local TLS endpoint; two offline demonstrations on materially different project types — Python numerical and Node.js data pipeline — plus adversarial and definition-of-done workflows; a live vendor endpoint remains unverified) |

## 7. What changed after the first v0.2 build (2026-11-27)

The first v0.2 build implemented the loop but left the three acceptance gaps above open, and an
audit of the implementation found defects the status table had not caught. Both are now closed:

- **Record fidelity for resume.** The journal carries the user's words, whole tool arguments, the
  call id on every completion, and the configured budgets; the event contract in `events.rs`
  matches what the engine writes. The engine writes a checkpoint after every step.
- **Resume.** `resume.rs` replays the journal into the conversation, the investigation state and
  the invocation ledger, answers trailing unanswered calls (completed from the ledger, interrupted
  as interrupted, never-started once), and appends a `session.resumed` event. A corrupt checkpoint
  is rebuilt from the journal; a broken chain is refused.
- **Export.** A dependency-free unified diff (`diff.rs`), the `export_changeset` tool with a
  recorded approval, and the `aporia agent export` verb. A finished record's manifest is refreshed
  after an authorised append so `verify_record` still passes.
- **HTTPS.** `provider/https.rs` on `native-tls` (the platform trust store), sharing the wire code
  with the plain-HTTP provider. Credentials come from a named environment variable, are redacted
  from echoed error bodies, and never appear in `describe`, the journal or any artefact.
- **Workflow hardening.** Approvals name the plan version they decide; a refused scope expansion
  continues the investigation; the approval prompt shows the plan; `/changes` shows the real
  changes; experiment outcomes must cite recorded evidence; the report carries the delivery,
  approvals, artefacts, journal identity and reproduction instructions.