# Third-Party Notices

CMRemote bundles or links against the following third-party software.
Each entry preserves the original copyright notice and licence
identifier.

This file is the canonical inventory required by the *Definition of
done* in `ROADMAP.md` ➜ Clean-room redesign track. New dependencies
must be added here in the same PR that introduces them.

---

## Rust agent (`agent-rs/`) — direct workspace dependencies

| Crate                  | Version | Licence            | Source                                           |
|------------------------|---------|--------------------|--------------------------------------------------|
| `tokio`                | 1.41    | MIT                | <https://crates.io/crates/tokio>                 |
| `futures`              | 0.3     | MIT OR Apache-2.0  | <https://crates.io/crates/futures>               |
| `serde`                | 1.0     | MIT OR Apache-2.0  | <https://crates.io/crates/serde>                 |
| `serde_json`           | 1.0     | MIT OR Apache-2.0  | <https://crates.io/crates/serde_json>            |
| `tracing`              | 0.1     | MIT                | <https://crates.io/crates/tracing>               |
| `tracing-subscriber`   | 0.3     | MIT                | <https://crates.io/crates/tracing-subscriber>    |
| `thiserror`            | 1.0     | MIT OR Apache-2.0  | <https://crates.io/crates/thiserror>             |
| `anyhow`               | 1.0     | MIT OR Apache-2.0  | <https://crates.io/crates/anyhow>                |
| `uuid`                 | 1.10    | MIT OR Apache-2.0  | <https://crates.io/crates/uuid>                  |
| `once_cell`            | 1.20    | MIT OR Apache-2.0  | <https://crates.io/crates/once_cell>             |
| `tempfile` (dev-only)  | 3.13    | MIT OR Apache-2.0  | <https://crates.io/crates/tempfile>              |

Transitive dependencies are not listed individually; their licences
must remain MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, or
Unicode-DFS-2016 to satisfy the eventual `cargo deny` policy added in
slice R0+1.

## .NET agent and server

The .NET portions of CMRemote depend on packages from
NuGet whose licences are tracked by the `dotnet list package
--include-transitive` output and the upstream package metadata. A
dedicated audit lands as part of the PR D hardening pass.
