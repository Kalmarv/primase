# primase-checks

Workspace architecture checks and integration tests spanning readers, passes and writers. This package has no
library API. `tests/architecture.rs` checks the role and layer rules in [CONTRIBUTING](../CONTRIBUTING.md).

Run the architecture check without game data:

```sh
cargo test -p primase-checks --test architecture
```

Real-data tests declare their inputs in [tests/inputs](tests/inputs/README.md). Set `GAME_DATA` and use
`cargo test --profile gate -p primase-checks` to run against configured inputs with debug assertions enabled.

License: GPL-3.0-or-later.
