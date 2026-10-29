# The first investigation transcript

`projectile.json` is a **human-authored script**, not a model. It exists so the workflow can be run
end to end on a real corpus project without a provider key: inspect, plan, approve, execute in a
sandbox, record a finding, and leave a verifiable record.

The model fact is `F3` in the current
`software/benchmarks/aerospace/projectile_sign_mutant` tree. That id is not decoration: if the
project changes so that `model.ap` is no longer the third fact, the plan is rejected with the reason
and the transcript must be rewritten. Evidence grounding is enforced, not advised.

From `software/`:

```sh
cargo build --release -p aporia-cli
./target/release/aporia investigate benchmarks/aerospace/projectile_sign_mutant \
    --objective "Determine where this model's declared r >= 0 rule stops holding, using the existing instrument, and leave a replayable record." \
    --provider scripted:examples/investigate/projectile.json \
    --approve --workdir .scratch/agent-demo
```

The report and journal are under the investigation directory it prints. Verify the record later with:

```sh
./target/release/aporia investigate report .scratch/agent-demo/inv-...
```

What this demonstrates: the agent loop, the approval gate, the sandbox copy, a real instrument run
with a real archive, evidence-checked findings, and a tamper-evident record. What it does not
demonstrate: a language model. The report says so in its first section.
