# The agent workbench — `aporia agent`

Status: **implemented; updated 2026-11-27.** This page describes what the application does today.
The contract it was built against is [iterative-agent-spec.md](iterative-agent-spec.md); the first
slice, `aporia investigate`, still works unchanged and is described in
[agent-workflow.md](agent-workflow.md).

## What it is

`aporia agent <project-dir>` opens a persistent investigation of a project and lets a person work
with it in a terminal:

```sh
aporia agent <project-dir> --objective TEXT --provider PROVIDER \
    [--model NAME] [--api-key-env VAR] [--ca-file FILE] [--workdir DIR] \
    [--allow-exec PROGRAM]... [--allow-write] \
    [--max-turns N] [--max-tool-calls N] [--timeout MS] [--batch]

aporia agent list [--workdir DIR]
aporia agent resume <investigation-dir> --provider PROVIDER [--model NAME] [--batch]
aporia agent status <investigation-dir>
aporia agent export <investigation-dir> --out FILE [--yes]
aporia agent report <investigation-dir>
```

The agent inspects the project with real tools, proposes a plan grounded in what it read, and waits
for approval before anything runs. After approval it works in an isolated copy of the project:
it runs programs, runs the instrument, reads and compares results, keeps a backlog of the work it
found but did not do, and closes with a report.

`--provider` is `scripted:<transcript.json>` (a committed transcript: a test double, and the
offline demonstrations), `http://host:port/v1` (a local OpenAI-compatible endpoint), or
`https://host/v1` (the same shape over TLS). HTTP and HTTPS need `--model`. HTTPS also takes
`--api-key-env VAR` — the name of the environment variable holding the bearer key; the key itself
never appears on the command line, in the record, or in an error message — and `--ca-file FILE` to
add a root certificate (an enterprise CA, or a test server). Verification is never disabled.
`--batch` runs without asking: every approval inside the ceilings is granted automatically.
Without it, the session asks, and the approval prompt shows the plan's gaps, tasks and scope.

Exit statuses follow the existing list: `0` finished, `1` finished with a critical or high
finding, `2` usage, `3` the project or workspace is unusable, `5` the record failed its integrity
check, `10` the plan was refused, `11` provider failure, `12` budget exhausted (the record is
incomplete), `13` the record could not be written.

## The conversation

The session is a loop, and the person is part of it. Between turns the prompt accepts a message
or a command:

| command | what it shows |
|---|---|
| `/help` | the commands |
| `/status` | the status, the phase and the tools available now |
| `/plan` | the approved plan |
| `/gaps` | the gaps the investigation has found |
| `/tools` | the tools this session may use |
| `/experiments` | the improvement backlog, most valuable first |
| `/findings` | the findings recorded so far |
| `/changes` | the changes made in the workspace, with their before/after identities |
| `/cancel` | stop at the next opportunity |
| `/pause` | leave the session open; resume it later from its journal |
| `/quit` | leave; the record stays on disk |

Anything else is a message to the agent, added to the conversation and seen by the model on its
next turn.

## The tools

Every capability is a registered tool with a stable name, a typed argument schema, a risk class
and an execution limit. The model is offered the schemas of the tools the current scope permits,
and every call passes host validation before anything runs.

| group | tools |
|---|---|
| inspection | `list_files`, `search_files`, `search_text`, `read_file`, `read_file_range` |
| discovery | `inspect_project`, `inspect_dependencies`, `inspect_configuration`, `inspect_test_structure`, `inspect_experiment_files` |
| source | `source_git` — head, status, diff and log from the original project, with fixed read-only commands |
| execution | `run_command` — an approved program, by bare name, in the workspace, with a timeout, a minimal environment, bounded output and declared artefacts |
| instrument | `instrument_run` — the Trust Atlas, still a separate process with its archive contract |
| results | `read_results`, `compare_results` — a result file, and the differences between two of them |
| control | `propose_plan`, `revise_plan`, `record_finding`, `propose_experiment`, `update_experiment`, `update_task`, `finish` |
| export | `export_changeset` — prepares the workspace's net changes as a unified diff inside the record and asks permission to write it to a destination outside the sandbox; the write happens only after the recorded approval |

Read-only tools never ask for approval. `run_command` needs its program named in the approved
scope; the plan asks for that, and the person approves the plan.

## Permissions

The plan carries the scope it needs: the programs it may run, whether it may edit the workspace,
and its budgets. The command line sets the ceiling — `--allow-exec` names the programs that may
be asked for, `--allow-write` allows asking to edit — and an approval can grant less than the
ceiling, never more.

When a revision asks for more than was approved — a new program, write access, a larger budget, a
changed objective — the person is asked again. Nothing expands silently. Every decision is
recorded in the journal before it takes effect. Refusing one program's expansion does not end the
investigation: the agent continues inside the scope that is still approved. An approval can never
grant more than the command-line ceiling; an attempt is recorded and refused.

## Keeping the loop honest

- **Every tool call gets its result.** A denial that raises a permission question is still
  delivered to the model, so the transcript is one a provider can continue.
- **Approvals name their plan version.** With two proposals waiting, the decision approves the
  one it names, never "the first undecided".
- **A third identical failing call is refused.** The host counts exact (name, arguments) failures
  and tells the model to change its approach instead of burning the budget repeating itself.
- **The conversation is bounded; the record is not.** When tool outputs pass 256 KiB in total, the
  oldest are elided from the conversation with a notice; the journal keeps every byte. The rule is
  a pure function of the messages, so a resume reconstructs the same bounded transcript.
- **A nearly spent budget tells the model to converge.** With two turns or four calls left, the
  host says so once: record what is established, keep the rest in the backlog, finish.
- **A finished experiment must cite recorded evidence.** `update_experiment` refuses a
  completed/failed/inconclusive status whose result cites no recorded F, I or E id. An outcome
  without evidence is not an outcome.

## The record

The investigation lives in a directory under `--workdir` (default `.aporia/investigations`):

```
inv-1750000000/
  journal.jsonl        one hash-chained event per line: the model, the calls, the approvals, the results
  inspection.json      the facts the first scan produced
  sandbox.json         every file that was copied, with its digest
  session.json         the versioned checkpoint
  workspace/           the isolated copy the agent worked in
  report.md            the report, generated by the host from the record
  investigation.json   the same content, machine-readable
  manifest.json        written last: the outcome, the counts, the journal head
```

`aporia agent list` lists the sessions and their status; `aporia agent status <dir>` prints one
session's checkpoint; `aporia agent report <dir>` prints a finished investigation's report without
executing anything.

### Resume

The journal is authoritative; `session.json` is a derived summary written after every step. A
process that dies leaves either the previous checkpoint or a stray temporary file, never a
half-written one. `aporia agent resume <dir> --provider PROVIDER` replays the journal into the
conversation, the investigation state and the invocation ledger, and continues:

- a call recorded as **completed** is answered from the ledger, never run again;
- a call recorded as **started but never completed** is reported to the model as interrupted,
  never run again — the previous process may have run it halfway;
- a call **requested but never started** runs once, because it never ran;
- a pending approval survives and is asked again;
- a **finished** record is history, not work to resume, and a **broken chain** is a named refusal;
- a corrupt checkpoint is rebuilt from the journal, not trusted.

The resume appends a `session.resumed` event naming the interrupted calls, so the record says what
happened. The wall-clock budget starts fresh for the continuation; turn and call budgets count the
whole investigation.

### Export

`aporia agent export <dir> --out FILE` computes the workspace's net changes against the original
project as a unified diff, shows the files and the patch's digest, and writes only after the person
answers yes (`--yes` for scripts). The export is journalled (`changeset.exported`, `by: cli`), and a
finished record's manifest is refreshed so its integrity check still passes. The destination may
not be inside the source project or the workspace. Binary changes are named as not representable,
never silently dropped. The same lifecycle is available to the model through `export_changeset`,
which prepares the patch in the record and waits for a recorded `ChangesetExport` approval.

## The experiment backlog

Work the investigation identified but did not perform is kept as a backlog item: the question,
why it matters, the evidence behind it, the method, the variables, the data it needs, what would
count as success, the risk, the status, and — once it ran — the result and the follow-up. The
statuses are `proposed`, `approved`, `ready`, `running`, `completed`, `failed`, `inconclusive`,
`blocked` and `cancelled`.

An experiment without an available runner stays a **proposal**: it is never reported as a
completed experiment, and a failed one stays in the history.

## The sandbox, honestly

The workspace is a complete copy of the project, and the original is only read. Paths a tool names
are resolved inside the copy; a parent component, an absolute path or a link that points out is
refused. Commands run without a shell, from a chosen directory inside the copy, with a deadline, a
bounded output and an environment that starts empty: the child receives only the names the host
allows, and credential-shaped names are refused even when they are listed.

What this is **not**: an operating-system jail. There is no process-tree kill — the timeout and
the cancellation kill the direct child, and a grandchild can outlive it. There is no network
isolation, because there are no network tools. [limitations.md](limitations.md) says the same
things in full.

## The demonstrations

Two committed projects, materially different, each with a real defect its own tests do not catch:

- `software/examples/agent-demo/` — a Python numerical project: the integration rule closes the
  loop over its samples, so the error approaches `-0.5` instead of shrinking. The transcript drives
  inspect → propose → approve → measure → fix → measure again → backlog → close, and the test
  `agent_demo` in `aporia-cli` asserts the record contains the run, the approval, the finding, the
  change and the backlog.
- `software/examples/agent-demo-node/` — a Node.js data pipeline: the variance estimator divides by
  `n` while the project documents the sample (`n - 1`) estimator. The transcript drives the same
  workflow, runs the project's own tests inside the sandbox, and the test `agent_demo_node` asserts
  the before/after measurements are both in the record.

Both demonstrations are offline: the provider is a committed transcript, and no claim is made that
a live model was consulted. The adversarial suite (`agent_adversarial`) and the definition-of-done
chain (`agent_definition_of_done`) exercise interruption, resume, refusal, exhaustion, timeout,
rollback, credential canaries and the authorised export round-trip through the real binary.
