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
cargo xtask gate        # fmt --check → clippy -D warnings → test → doc
```

`cargo xtask` wraps the repo's `xtask/` crate (`fmt`, `clippy`, `test`, `doc`,
`check`, `gate`); `ci.sh` is the gate entry. The crate builds a `cdylib` +
`rlib`; `cargo test` links the `rlib` statically.

```sh
cargo build        # target/debug/libhnl_ext_test.so
HANDL_EXT_DIR=$PWD/target/debug ../host-cli/target/debug/hnl test --list
```

Sibling-repo deps resolve from sibling checkouts via the `[patch]` tables;
without them the `branch = "main"` git sources are used.

## License

MIT OR Apache-2.0.
