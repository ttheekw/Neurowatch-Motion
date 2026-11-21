# Analytics library snapshots

`nw-analytics-protocol`, `nw-analytics-sdk` and `nw-service-protocol` contain the
library sources needed to build this daemon without a NeuroWatch checkout. They are
copies of NeuroWatch's crates of the same names at NeuroWatch v0.1.10:
commit `eeddef965d6a486be14151b79d9e34680d04f514`
(https://github.com/ttheekw/Neurowatch/tree/eeddef965d6a486be14151b79d9e34680d04f514/crates).
They use NeuroWatch's Apache-2.0 license.

Only each crate's `Cargo.toml` and library `src/` files are included; NeuroWatch's
Python bridge executable, tests and fixtures are left out. The manifests write out
NeuroWatch's workspace inheritance as explicit package and dependency values; the
library sources are unchanged. `sources.sha256` records them and the manifests, and CI and the release build check it.

To update, copy the crates from a newer NeuroWatch release the same way, review
dependency changes, regenerate `sources.sha256`, and run
`cargo test --workspace --locked`. Keep these snapshots compatible with the
recorder's analytics protocol. `neurowatch.ref` separately pins the upstream
GStreamer packaging scripts used for Linux releases.
