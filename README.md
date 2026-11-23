# ext-test

`hnl-ext-test` — `hnl test` as a dynamic `hnl` extension.

An `ext-*` crate: the test runner packaged as a
`libhnl_ext_test.{so,dylib,dll}` shared library for the dumb `hnl` host. The
host discovers it via `--ext-dir`, `$HANDL_EXT_DIR`, `~/.handl/ext/`, or a dev
`target/debug/` next to the `hnl` binary.

## Commands

```text
hnl test [--list] [--filter <pat>] [--tag <expr>] [--jobs N|auto]
         [--report human|json] [--changed] [--cache] [paths...]
```

Discovery, tag predicates, reporters, and the `.handl/cache/test-runner/
cache.json` result cache live in `hnl-engine-test`; this crate runs
the `test` capability of `handl-plugin-ext-test` inside a `DefaultHost` and
keeps only the CLI policy — default paths (`tests/` else `.`), flag
validation, output streams, exit code.

Exit codes: `0` ok, `1` test failures or discovery parse errors, `2` usage
errors.

## Development

```bash
cargo run --manifest-path xtask/Cargo.toml -- gate   # fmt → clippy → test → doc
```

`xtask/` provides the gate tasks (`fmt`, `clippy`, `test`, `doc`, `check`,
`gate`); `workflows/gate.yaml` is the per-push gate — the `handl-gates` bundle's rust campaign (fmt → clippy → test → doc), run by bupha/the fleet runner via `peregrin campaign run`. The crate
builds a `cdylib` + `rlib`; `cargo test` links the `rlib` statically.

```sh
cargo build        # target/debug/libhnl_ext_test.so
HANDL_EXT_DIR=$PWD/target/debug ../host-cli/target/debug/hnl test --list
```

Deps are git deps on `branch = "main"`; locally the workspace-root
`.cargo/config.toml` patches them to sibling checkouts (dev-only, never
committed).

## License

MIT.
