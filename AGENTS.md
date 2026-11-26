# ext-test

`hnl-ext-test` — `hnl test` as a dynamic `hnl` extension: the test
runner packaged as a `libhnl_ext_test.{so,dylib,dll}` cdylib the dumb
`hnl` host discovers at runtime.

## Layout

| path | role |
| --- | --- |
| `src/` | extension ABI exports + `test` command wiring |
| `xtask/` | vendored gate crate (`fmt`, `clippy`, `test`, `doc`, `gate`) |
| `workflows/gate.yaml` | per-push gate — `handl-gates` rust campaign via bupha |

## Contract

- Commands: `hnl test [--list] [--filter <pat>] [--tag <expr>]
  [--jobs N|auto] [--report human|json] [--changed] [--cache]
  [--watch] [--coverage[=out.json]] [--timeout <dur>] [paths...]`.
  `--timeout` takes seconds (`30`, `30s`, `500ms`).
- Discovery, tag predicates, reporters, and the
  `.handl/cache/test-runner/cache.json` result cache live in
  `hnl-tool-test`; this crate runs the `test` capability of
  `handl-plugin-ext-test` inside a `DefaultHost` and keeps only the CLI
  policy — default paths (`tests/` else `.`), flag validation, output
  streams, exit code.
- Exit codes: `0` ok, `1` test failures or discovery parse errors, `2`
  usage errors.

## Verification

```bash
cargo run --manifest-path xtask/Cargo.toml -- gate   # fmt → clippy → test → doc
```

<!-- myco:agent-awareness -->

## Agent awareness

- Session start / any work discussion: run `myco status --prompt`
  before proposing next steps — know the board first.
- User describes new work → offer to capture it (`myco todo add`,
  `issue new`, `proposal new`). Don't wait to be asked.
- Picking up a task: `myco resume <ID>` is the briefing.
- Wrapping up: `myco handoff <ID>` anything resumable; keep statuses
  honest (claimed → in-progress → blocked/done).

<!-- /myco:agent-awareness -->
