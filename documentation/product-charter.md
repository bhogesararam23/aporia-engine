# APORIA — Product Identity Charter
Version 1.0 · Foundational product definition

> Adopted 2026-10-29 as the governing product definition. The Trust Atlas instrument described in
> [architecture.md](architecture.md) is the first capability of the product defined here; it is not the
> product's identity. The internal decision record is `docs/decisions/0041-product-identity-and-first-slice.md`.

## 1. Identity

**Name:** APORIA

**Category:** Agentic Research and Development Workbench

**Tagline:** Turn unfinished work into better results.

**One-line description:** APORIA is an intelligent research and engineering agent that investigates existing work, identifies missing experiments and improvement opportunities, executes approved tasks in isolated workspaces, analyses the results, and helps users make meaningful progress.

**Mission:** Help scientists, researchers, engineers and developers improve the quality of their work by discovering what they have not investigated, performing the work needed to address those gaps, and producing evidence-backed results.

**Vision:** Build a capable, extensible agent that can take a person's unfinished computational work and help carry it from its current state towards a better-tested, better-understood and more complete result.

## 2. The central idea

People often have something that already works partially: a research prototype, a codebase, a simulation, a machine-learning model, an experiment, a dataset, or a set of results.

They may know what they want to achieve but not know:
- which experiments they have overlooked;
- which assumptions remain untested;
- why their implementation produces unexpected results;
- what the most valuable next experiment would be;
- which improvements are worth the time and computational cost;
- whether an apparent improvement is reproducible;
- what evidence is still missing before they can trust their conclusions.

APORIA exists to help bridge that gap.

It should not merely tell someone what they could do. It should inspect the actual work, develop a grounded understanding of the problem, identify valuable next steps, execute permitted work, evaluate the results and continue investigating when justified.

**The product is the agent and the complete workflow. Algorithms, metrics, search strategies and evidence analysis are capabilities used by that agent.**

## 3. Who APORIA serves

### Primary users

Scientists, research engineers, independent researchers, software developers, engineering students and technical teams working on computational problems.

### Initial scope

The first complete product should specialise in code-driven work:

- source-code repositories and partially completed projects;
- Python, Rust and other supported codebases;
- scripts, notebooks, builds and test suites;
- numerical methods, simulations and computational models;
- machine-learning training and evaluation pipelines;
- experimental configurations and parameter sweeps;
- datasets, logs, measurements and result files;
- existing benchmarks and research implementations.

The first version must work well across this category before attempting to support every conceivable research discipline.

The architecture should nevertheless allow new execution adapters, analysis capabilities and domain-specific tools to be added later.

## 4. How APORIA works

### Stage 1 — Receive the work

The user gives APORIA a project, files, experiment, dataset or research objective and explains what they want to improve.

The starting point may be incomplete or disorganised. APORIA should not require the user to prepare a perfect specification before it can help.

### Stage 2 — Inspect the actual state

APORIA examines available files, repository structure, code, dependencies, existing tests, experiment definitions, logs and results.

It establishes what is present, what has already been tried, which capabilities are available, what it can execute and what remains unknown.

It must distinguish between something that is genuinely missing and something it simply has not yet verified.

### Stage 3 — Identify gaps and opportunities

APORIA creates an evidence-backed list of missing investigations, potential defects, untested assumptions, experimental opportunities and possible improvements.

Each item explains why it matters, what evidence supports the recommendation, how it could be investigated and what remains uncertain.

Recommendations are prioritised as:

- **Critical:** a verified defect, invalidating condition or essential missing check that blocks the intended conclusion.
- **High priority:** an important unresolved question or experiment likely to affect the result.
- **Recommended:** a meaningful improvement supported by the project's current state.
- **Exploratory:** a plausible new direction whose benefit remains uncertain.

APORIA must not label an experiment mandatory merely because it is a standard practice. It should explain the condition that makes the experiment necessary.

### Stage 4 — Form an investigation plan

The agent converts the gaps into a practical, ordered plan.

Each task should specify its objective, hypothesis or question, dependencies, proposed method, expected evidence, success criteria, estimated resources and risks.

Where several explanations are plausible, APORIA should prefer experiments that efficiently distinguish them rather than immediately committing to one explanation.

The user can review and modify the plan before execution.

### Stage 5 — Work inside an isolated sandbox

APORIA creates an isolated investigation workspace containing the necessary copy or worktree, configuration, dependencies and experiment inputs.

It can perform permitted operations such as:

- inspecting and modifying code;
- creating test cases and experiment scripts;
- building and running the project;
- executing tests and benchmarks;
- running controlled parameter sweeps;
- collecting and analysing results;
- comparing implementations;
- investigating failures;
- creating and testing patches.

It preserves the original work unless the user explicitly authorises changes outside the sandbox.

The sandbox is a real execution environment, not just a directory for storing reports. Untrusted code must receive appropriate process isolation, resource limits and permissions.

### Stage 6 — Execute, observe and adapt

APORIA carries out the approved plan using real tools. It records actual outcomes, handles failures, inspects evidence and determines which tasks should follow.

It may suggest new experiments when results contradict the original hypothesis or reveal another important question. Materially expanding the approved scope requires user authorisation.

The agent must never invent execution results or claim a hypothesis is confirmed simply because the experiment completed.

### Stage 7 — Improve and verify

Where appropriate, APORIA implements a proposed change inside the sandbox, compares the original and modified versions, runs relevant regression tests and evaluates whether the change actually helps.

An unsuccessful modification is still an informative outcome when it is measured and reported honestly.

### Stage 8 — Deliver useful results

At the end of an investigation, APORIA provides the user with:

- findings and their supporting evidence;
- experiments completed and experiments still recommended;
- reproducible measurements and comparisons;
- code changes and reviewable diffs;
- failures, limitations and unresolved hypotheses;
- a prioritised next-step backlog;
- a persistent investigation record and reproducibility information.

The result should help the user decide what to do next without having to reconstruct the entire investigation manually.

## 5. The sandbox and investigation workspace

Every substantial investigation should have its own persistent workspace.

It should track the original project identity, source revision, working copy, objective, plan, configuration, tool invocations, experiment runs, results, logs, changes, checkpoints and generated artefacts.

The workspace should support reopening previous investigations, resuming interrupted tasks, comparing runs and selectively exporting patches.

APORIA should preserve enough information to explain what happened even after the interactive agent session ends.

Network access, destructive operations, changes to the original project, remote execution and publication must be governed by explicit permissions appropriate to their risks. The agent may operate autonomously inside an approved scope, but must not bypass that scope.

## 6. The experiment and improvement backlog

The investigation backlog is a core feature, not an optional report at the end.

APORIA should be able to say:

- This experiment has not been performed, according to the inspected records.
- This assumption has not been tested under the current conditions.
- This baseline would help establish whether the proposed method improves anything.
- This ablation would isolate the contribution of a particular component.
- This repeated-run experiment would measure variability.
- This boundary or stress test would examine a suspected failure condition.
- This result remains inconclusive, so the next informative test is different from the previous one.

Each recommendation should state the question, rationale, method, expected information, resource cost, success criteria and possible follow-up.

The agent should prioritise useful information and meaningful progress, rather than maximising the number of experiments it launches.

## 7. Rust and the application architecture

APORIA must be developed as a proper software application built around Rust, not as a research algorithm with a thin interface around it.

The architecture should include the following logical components.

**Rust application host:** Manages configuration, lifecycle, permissions, tool execution, cancellation, resource limits and coordination.

**Agent orchestrator:** Translates user objectives into plans, chooses supported operations, interprets actual results and coordinates subsequent actions. It must not fabricate capabilities that the application does not have.

**Tool system:** Exposes typed and testable operations for filesystem inspection, source modification, building, testing, experiment execution, analysis, version control and reporting.

**Execution and sandbox manager:** Creates isolated workspaces, launches programs, enforces execution constraints and captures outputs.

**Analysis capabilities:** Provide relevant project, behavioural, numerical, statistical, experimental and reliability analyses when supported by the target and available tools.

**Investigation store:** Persists plans, actions, findings, observations, results, provenance, checkpoints and reproducibility data.

**User-facing workbench:** Provides the agent interaction, plan review, live progress, tool activity, findings, experiment management and inspection surfaces.

**Model-provider boundary:** Allows the underlying language model to be replaceable without making the core execution engine dependent on a specific model or provider.

The existing APORIA Rust analysis and execution code should become capabilities within this larger product. It should be reused where appropriate, not discarded, but it should not dictate the entire application identity.

A CLI should remain available for advanced users, scripts and CI. The primary experience, however, must be an agent-led application that coordinates actual work rather than merely exposing low-level commands.

## 8. Product boundaries

APORIA is an agent for research and engineering improvement. It is not intended to become a general-purpose agent that tries to do every possible task.

Its distinguishing workflow is:

**Inspect existing work → discover gaps → plan experiments → execute in a sandbox → analyse evidence → improve or investigate further → preserve and deliver results.**

APORIA is not limited to numerical boundary discovery, numerical reliability or any single algorithm. Those are supported capabilities, not the product definition.

APORIA is also not an automatic correctness oracle. It must distinguish observed evidence, interpretations, hypotheses, incomplete investigations and established results.

Automatic repair is useful, but repair without verification is not success.

## 9. Non-negotiable principles

**Real action:** Tool calls must perform actual work and produce inspectable results.

**Grounded recommendations:** Missing experiments and defects must be based on inspected evidence or be clearly labelled as hypotheses.

**User-directed autonomy:** The agent can carry out routine work within an approved scope and budget. Consequential actions remain controlled.

**Isolation:** Experiments should not corrupt original projects or contaminate each other's results.

**Reproducibility:** Preserve inputs, configuration, source identity, tool results and the information needed to rerun an investigation.

**Honest uncertainty:** Failed, incomplete and inconclusive work must remain visible.

**Measured improvement:** A change counts as an improvement only relative to an appropriate objective and supporting evidence.

**Extensibility:** New tools, analysis methods, model providers and domain-specific capabilities can be added without redesigning the entire application.

**Product-first development:** Build features around complete user workflows, not around the number of algorithms, crates, benchmarks or commits.

## 10. Definition of a successful first complete version

A user should be able to provide an existing code-driven project and a goal such as:

"Inspect this project, determine what is preventing me from achieving my objective, identify the most valuable missing experiments, run the useful ones in an isolated workspace, and help me improve the result."

APORIA should then be able to:

1. Inspect the project and establish its actual state.
2. Produce a ranked and evidence-backed gap list.
3. Propose a concrete investigation plan.
4. Create and manage an isolated workspace.
5. Execute the permitted build, test and experiment workflow.
6. Examine results and identify useful follow-up work.
7. Implement and verify justified changes when appropriate.
8. Produce a reproducible record, useful deliverables and remaining recommendations.

At least one complete workflow must work end to end on a real supported project. A collection of disconnected features does not meet this bar.

## 11. The governing product principle

APORIA should help people make progress they would otherwise struggle to make alone.

It must not merely tell users how to do research or development. It should actively help them investigate, experiment, improve, verify and understand their work, while preserving the evidence needed to judge the outcome.

**APORIA is an agent that helps researchers and developers discover what needs to be done, do the useful work in a controlled environment, and learn from the result.**
