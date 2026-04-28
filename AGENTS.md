# Agent Instructions

## Feature And Test Pairing

Every behavior change must update the matching build gate in the same change.

- If a feature is added, changed, or fixed, add or update at least one focused test that would fail without that feature.
- If a bug is fixed, first encode the observed failure shape as a regression test, then fix the code.
- Relay behavior must be covered in both directions when direction matters: A -> B and B -> A.
- Claude/Codex behavior must be covered for the relevant pane combinations, not only the easiest happy path.
- Hook and transcript behavior must prefer realistic integration tests over direct channel injection when the production path crosses HTTP, env vars, or transcript discovery.
- When controls change (`Ctrl-*`, pause, queue, route on/off, manual relay, stop relay), update tests for the control state and the resulting relay/write behavior.
- Do not treat `cargo test` passing as sufficient if the changed feature has no test that directly exercises it.

Required local gate before handoff:

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```
