## Orchestrator first pass and sandboxed cargo

- A task's "first pass" in the overseer log and the repo rate now needs its one implement attempt to pass with no review finding other than P3. A parent counts only when every subtask at every depth is first pass. Owner answers no longer decide it; they stay in "owner touches".
- Agent runs whose sandbox cannot reach orchd's compiler cache now run cargo without the compiler wrapper from your Cargo config (such as sccache), so `cargo test` inside them no longer fails with "Operation not permitted".
