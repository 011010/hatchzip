## What this changes

<!-- One or two sentences. Link the issue if there is one. -->

## Why

<!-- What problem it solves. -->

## Verification

<!--
How you know it works, and on what machine and filesystem.
If it affects speed or memory, paste the measured before and after.
-->

- [ ] `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` pass
- [ ] `cargo test` passes
- [ ] Any change to extraction, the journal or the truncation order was run against the crash tests
- [ ] Any number in the README this changes was re-measured
