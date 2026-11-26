//! `hnl-ext-test` — the `test` extension for `hnl` (ADR-0149).
//!
//! The runner lives in `hnl-tool-test` (discovery, filters,
//! tag predicates, reporters, the ADR-0089 result cache) behind the `test`
//! capability adapter `handl-plugin-ext-test`; this crate is the
//! *packaging*: it exports the ADR-0149 ABI (`handl_ext_manifest` /
//! `handl_ext_dispatch` / `handl_ext_free`) and implements the
//! `hnl test [--list] [--filter <pat>] [--tag <expr>] [--jobs N|auto]
//! [--report human|json] [--changed] [--cache] [--watch]
//! [--coverage[=out.json]] [--timeout <dur>] [paths...]` command surface —
//! byte-for-byte the semantics the built-in `test` arm had before the host
//! went dumb.
//!
//! Internally the extension is a *host of plugins* like any other
//! (ADR-0149 §Consequences): a `DefaultHost` running the `test` capability
//! of [`handl_plugin_ext_test::TestPlugin`], with the canonical kit's base
//! runtime injected through a capture-less factory reading a thread-cached
//! [`handl_kit_prelude::Kit`] (ADR-0128 §"The 0.8 seam"). The CLI keeps
//! the *policy* — default paths, flag validation, output streams and exit
//! code — while the capability does discovery + execution.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use handl_ext::{ExtRequest, ExtResponse, ExtensionManifest};
use handl_kit_pkg::KitProjectExt;
use handl_kit_prelude::Kit;
use handl_platform::{CapabilityRequest, DefaultHost, PlatformError};
use serde_json::json;

handl_ext::export!(
    ExtensionManifest::new("test")
        .version(env!("CARGO_PKG_VERSION"))
        .command(
            "test",
            "test [--list] [--filter <pat>] [--tag <expr>] [--jobs N|auto] [--report human|json] [--changed] [--cache] [--watch] [--coverage[=out.json]] [--timeout <dur>] [paths...]",
        ),
    dispatch
);

/// ABI dispatch: only the `test` command is declared.
fn dispatch(cmd: &str, req: &ExtRequest) -> ExtResponse {
    match cmd {
        "test" => run(&req.args, Path::new(&req.cwd)),
        other => ExtResponse::error(2, format!("test: unknown command `{other}`\n")),
    }
}

// ---------------------------------------------------------------------------
// `test [paths...]`
// ---------------------------------------------------------------------------

/// Output accumulator: the extension renders one `ExtResponse` instead of
/// printing, so stdout/stderr collect here and the code is returned.
#[derive(Default)]
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn err(&mut self, msg: impl std::fmt::Display) {
        self.stderr.push_str(&format!("{msg}\n"));
    }
}

thread_local! {
    /// The canonical kit, memoized per thread for the tooling plugin.
    ///
    /// `Plugin: Send + Sync` forbids storing a `Kit` (`!Sync`), and a
    /// `Runtime` cannot cross threads — so per ADR-0128 §"The 0.8 seam"
    /// the extension owns the kit cache here and injects a *capture-less*
    /// `Send + Sync` factory (`TestKitSource`) the runner invokes on
    /// whichever thread needs a base runtime.
    static TOOLING_KIT: RefCell<Option<Kit>> = const { RefCell::new(None) };
}

/// Run `f` with the thread-cached canonical kit, assembling on first use.
fn with_tooling_kit<R>(f: impl FnOnce(&Kit) -> R) -> R {
    TOOLING_KIT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            // Project-aware: discover `.handl.toml` from the CWD upward so
            // `import "dep/..."` resolves against `[dependencies]` (ADR-0128
            // Phase-5 name resolution).
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            *slot = Some(
                Kit::discover(&cwd)
                    .or_else(|_| Kit::new())
                    .expect("canonical kit must assemble"),
            );
        }
        f(slot.as_ref().expect("kit populated above"))
    })
}

/// A host with the `test` plugin registered — the extension consumes the
/// same `DefaultHost`/`CapabilityRequest` machinery the built-in command
/// did; only the packaging changed (ADR-0149 §Consequences).
#[allow(clippy::result_large_err)]
fn tooling_host() -> Result<DefaultHost, PlatformError> {
    let mut host = DefaultHost::new();
    host.register_plugin(handl_plugin_ext_test::TestPlugin::new(Arc::new(|| {
        with_tooling_kit(|kit| {
            Ok::<_, String>(hnl_tool_test::base_runtime(
                kit.runtime(),
                kit.module_root().map(Path::to_path_buf),
            ))
        })
    })));
    host.load()?;
    host.resolve_composition()
        .map_err(|e| PlatformError::Plugin(format!("tooling composition: {e}")))?;
    Ok(host)
}

/// Run the `test` command, resolving relative paths against `cwd`.
fn run(args: &[String], cwd: &Path) -> ExtResponse {
    let mut out = Out::default();

    let mut filter: Option<String> = None;
    let mut list = false;
    let mut report_kind = "human".to_string();
    let mut jobs: Option<String> = None;
    let mut cache = false;
    let mut watch = false;
    let mut tags: Vec<String> = Vec::new();
    let mut changed = false;
    // `--coverage[=path]`: `Some(None)` = enabled, default output path;
    // `Some(Some(path))` = enabled, explicit JSON output path.
    let mut coverage: Option<Option<String>> = None;
    // `--timeout <dur>`: per-test bound (`RunConfig::timeout` /
    // `WatchRunOptions::timeout`). Bare number = seconds; `ms`/`s`
    // suffixes allowed. `None` = unbounded (the runner default).
    let mut timeout: Option<Duration> = None;
    let mut paths = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--filter" => {
                i += 1;
                match args.get(i) {
                    Some(v) => filter = Some(v.clone()),
                    None => {
                        out.err("test: --filter requires a value");
                        return code(2, out);
                    }
                }
            }
            "--tag" => {
                i += 1;
                match args.get(i) {
                    Some(v) => tags.push(v.clone()),
                    None => {
                        out.err("test: --tag requires a value");
                        return code(2, out);
                    }
                }
            }
            "--jobs" => {
                i += 1;
                match args.get(i) {
                    Some(v) => jobs = Some(v.clone()),
                    None => {
                        out.err("test: --jobs requires a value");
                        return code(2, out);
                    }
                }
            }
            "--timeout" => {
                i += 1;
                match args.get(i) {
                    Some(v) => match parse_timeout(v) {
                        Some(d) => timeout = Some(d),
                        None => {
                            out.err(
                                "test: --timeout requires a positive duration (e.g. `30`, `30s`, `500ms`)",
                            );
                            return code(2, out);
                        }
                    },
                    None => {
                        out.err("test: --timeout requires a value");
                        return code(2, out);
                    }
                }
            }
            "--report" => {
                i += 1;
                match args.get(i) {
                    Some(v) => report_kind = v.clone(),
                    None => {
                        out.err("test: --report requires a value");
                        return code(2, out);
                    }
                }
            }
            "--list" => list = true,
            "--changed" => changed = true,
            "--cache" => cache = true,
            "--watch" => watch = true,
            "--coverage" => coverage = Some(None),
            s if s.starts_with("--coverage=") => {
                let path = &s["--coverage=".len()..];
                coverage = Some(if path.is_empty() {
                    None
                } else {
                    Some(path.to_string())
                });
            }
            "--json" => report_kind = "json".to_string(),
            "--" => {}
            s if s.starts_with("--") => {
                out.err(format!("test: unknown flag `{s}`"));
                return code(2, out);
            }
            s => paths.push(s.to_string()),
        }
        i += 1;
    }

    // Default paths: `tests/` if it exists, otherwise the current directory.
    // (`cwd` is the host process's CWD — the extension runs in-process, so
    // `Path::new("tests")` and the runner see the same directory.)
    if paths.is_empty() {
        if cwd.join("tests").is_dir() {
            paths.push("tests".to_string());
        } else {
            paths.push(".".to_string());
        }
    }

    let path_bufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();

    // `--watch` bypasses the one-shot capability path entirely: watch mode
    // is a long-lived loop that streams each round's report straight to
    // the process's stdout/stderr (the `ExtResponse` channel only carries
    // a final result, which a watch session never reaches).
    if watch {
        return run_watch_mode(
            &path_bufs,
            cwd,
            filter.as_deref(),
            &tags,
            jobs.as_deref(),
            &report_kind,
            cache,
            list,
            changed,
            timeout,
        );
    }

    // Park the kit discovered from the request CWD so the injected plugin
    // factory shares this instance instead of assembling a second.
    if let Ok(kit) = Kit::discover(cwd) {
        TOOLING_KIT.with(|cell| *cell.borrow_mut() = Some(kit));
    }

    let mut host = match tooling_host() {
        Ok(h) => h,
        Err(e) => {
            out.err(format!("test: tooling host unavailable: {e}"));
            return code(2, out);
        }
    };

    // Same capability path a third-party consumer uses (ADR-0085 step 4). The
    // command keeps the *policy* — default paths, flag validation, output
    // stream and exit code — and the runner capability does discovery +
    // execution.
    let mut payload = json!({
        "paths": path_bufs
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
        "list": list,
        "changed": changed,
        "report": report_kind,
    });
    if let Some(f) = &filter {
        payload["filter"] = json!(f);
    }
    if !tags.is_empty() {
        payload["tags"] = json!(tags);
    }
    if cache {
        payload["cache"] = json!(true);
    }
    if coverage.is_some() {
        payload["coverage"] = json!(true);
    }
    if let Some(j) = &jobs {
        payload["jobs"] = if j == "auto" {
            json!("auto")
        } else {
            match j.parse::<usize>() {
                Ok(n) if n > 0 => json!(n),
                _ => {
                    out.err("test: --jobs requires a positive integer or `auto`");
                    return code(2, out);
                }
            }
        };
    }
    if let Some(t) = timeout {
        // Seconds as a float — the capability parses `timeout` back into a
        // `Duration` (`RunConfig::timeout`).
        payload["timeout"] = json!(t.as_secs_f64());
    }

    let request = CapabilityRequest {
        capability: "test".into(),
        payload,
    };
    let response = match host.run(&request) {
        Ok(r) => r,
        Err(e) => {
            out.err(format!("test: {e}"));
            return code(2, out);
        }
    };

    // Runner warnings (e.g. cache load/save failures) — the engine reports
    // them rather than aborting, exactly as before the reroute.
    if let Some(warnings) = response.0.get("warnings").and_then(|v| v.as_array()) {
        for w in warnings {
            if let Some(msg) = w.as_str() {
                out.err(format!("test: warning: {msg}"));
            }
        }
    }

    // Report parse errors discovered while scanning test modules.
    let mut had_parse_error = false;
    if let Some(errors) = response.0.get("parseErrors").and_then(|v| v.as_array()) {
        for e in errors {
            let file = e
                .get("file")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let msg = e.get("message").and_then(|v| v.as_str()).unwrap_or("");
            out.err(format!("{file}: parse error: {msg}"));
            had_parse_error = true;
        }
    }

    match response.0.get("status").and_then(|v| v.as_str()) {
        Some("list") => {
            if let Some(cases) = response.0.get("cases").and_then(|v| v.as_array()) {
                for case in cases {
                    let name = case.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let module = case.get("module").and_then(|v| v.as_str()).unwrap_or("");
                    out.stdout.push_str(&format!("{name} ({module})\n"));
                }
            }
            code(if had_parse_error { 1 } else { 0 }, out)
        }
        Some("ok") | Some("failed") => {
            let rendered = response
                .0
                .get("report")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // The JSON reporter emits a single line; the human reporter has
            // its own trailing newline — same split as before the reroute.
            if report_kind == "json" {
                out.stdout.push_str(&format!("{rendered}\n"));
            } else {
                out.stdout.push_str(rendered);
            }
            // `--coverage[=path]`: the capability returns the merged
            // coverage report as JSON; the CLI owns the file write
            // (default `coverage.json` in the CWD).
            if let Some(cov_path) = &coverage
                && let Some(cov) = response.0.get("coverage")
            {
                let path = cov_path
                    .clone()
                    .unwrap_or_else(|| "coverage.json".to_string());
                match std::fs::write(&path, serde_json::to_string_pretty(cov).unwrap_or_default()) {
                    Ok(()) => out.err(format!("test: coverage written to {path}")),
                    Err(e) => out.err(format!("test: failed to write coverage to {path}: {e}")),
                }
            }
            let run_failed = response.0.get("status").and_then(|v| v.as_str()) == Some("failed");
            code(if run_failed || had_parse_error { 1 } else { 0 }, out)
        }
        other => {
            out.err(format!("test: unexpected capability status {other:?}"));
            code(2, out)
        }
    }
}

// ---------------------------------------------------------------------------
// `test --watch`
// ---------------------------------------------------------------------------

/// Run `hnl test --watch`: the initial run plus a rerun per debounced fs
/// event batch under the project root (`cwd`), streaming each round's
/// report to the process's real stdout/stderr.
///
/// This never returns an `ExtResponse` in practice — [`hnl_tool_test::run_watch`]
/// blocks until the process is killed. It only returns early on argument
/// errors or a watcher/discovery failure, so the caller still gets a
/// well-formed response for those paths.
#[allow(clippy::too_many_arguments)]
fn run_watch_mode(
    paths: &[PathBuf],
    cwd: &Path,
    filter: Option<&str>,
    tags: &[String],
    jobs: Option<&str>,
    report_kind: &str,
    cache: bool,
    list: bool,
    changed: bool,
    timeout: Option<Duration>,
) -> ExtResponse {
    let mut out = Out::default();
    if list {
        out.err("test: --watch cannot be combined with --list");
        return code(2, out);
    }
    if changed {
        out.err("test: --watch cannot be combined with --changed");
        return code(2, out);
    }
    if cache {
        out.err("test: --cache is not supported with --watch (cache disabled)");
    }

    // Same predicate composition the capability applies: --filter AND
    // every --tag expression.
    let mut predicate = hnl_tool_test::filter_predicate(filter.unwrap_or(""));
    for expr in tags {
        predicate = predicate.and(hnl_tool_test::tag_expr_predicate(expr));
    }

    // Same jobs mapping as the capability: omitted = sequential (the
    // runner default), `auto` = available parallelism, N = parallel(N).
    let policy = match jobs {
        None => handl_plugin_lib_scheduler::SchedulePolicy::sequential(),
        Some("auto") => handl_plugin_lib_scheduler::SchedulePolicy::default(),
        Some(j) => match j.parse::<usize>() {
            Ok(n) if n > 0 => handl_plugin_lib_scheduler::SchedulePolicy::parallel(n),
            _ => {
                out.err("test: --jobs requires a positive integer or `auto`");
                return code(2, out);
            }
        },
    };

    // Park the kit discovered from the request CWD (same as `run`) so the
    // injected factory shares this instance.
    if let Ok(kit) = Kit::discover(cwd) {
        TOOLING_KIT.with(|cell| *cell.borrow_mut() = Some(kit));
    }

    let source: hnl_tool_test::TestKitSource = Arc::new(|| {
        with_tooling_kit(|kit| {
            Ok::<_, String>(hnl_tool_test::base_runtime(
                kit.runtime(),
                kit.module_root().map(Path::to_path_buf),
            ))
        })
    });

    // Vitest-style ergonomics: watch the whole project root the user ran
    // `hnl test` in, not just the test directories — helpers living outside
    // `tests/` still trigger reruns. The watcher-side filter restricts
    // events to `.hnl`/`.handl`/`.handl.toml`, so build output and `.git`
    // never wake the loop.
    let watch_root = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let options = hnl_tool_test::WatchRunOptions {
        paths: paths.to_vec(),
        watch_roots: vec![watch_root],
        predicate,
        policy,
        timeout,
        ..hnl_tool_test::WatchRunOptions::default()
    };

    let report_kind = report_kind.to_string();
    let mut sink = |round: &hnl_tool_test::WatchRound| {
        if round.index > 0 {
            eprintln!(
                "\nwatch: rerun #{} — {} file(s) changed",
                round.index,
                round.changed.len()
            );
            for p in &round.changed {
                eprintln!("  {}", p.display());
            }
        }
        // Parse errors discovered while scanning test modules — same
        // rendering as the one-shot path.
        for (file, msg) in &round.plan.parse_errors {
            eprintln!("{}: parse error: {}", file.display(), msg);
        }
        let summary = hnl_tool_test::summarize(&round.report);
        let rendered = match report_kind.as_str() {
            "json" => <hnl_tool_test::JsonReporter as hnl_tool_test::Reporter>::format(
                &hnl_tool_test::JsonReporter,
                &round.report,
                &summary,
            ),
            _ => <hnl_tool_test::HumanReporter as hnl_tool_test::Reporter>::format(
                &hnl_tool_test::HumanReporter,
                &round.report,
                &summary,
            ),
        };
        if report_kind == "json" {
            println!("{rendered}");
        } else {
            print!("{rendered}");
        }
        use std::io::Write;
        let _ = std::io::stdout().flush();
    };

    match hnl_tool_test::run_watch(&source, &options, &mut sink) {
        Ok(()) => code(0, out),
        Err(e) => {
            out.err(format!("test: {e}"));
            code(2, out)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a `--timeout` duration: a bare number is seconds; `ms`/`s`
/// suffixes give milliseconds/seconds (e.g. `30`, `30s`, `500ms`).
/// Must be positive and finite.
fn parse_timeout(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    let (num, scale) = if let Some(n) = raw.strip_suffix("ms") {
        (n, 1e-3)
    } else if let Some(n) = raw.strip_suffix('s') {
        (n, 1.0)
    } else {
        (raw, 1.0)
    };
    let secs = num.trim().parse::<f64>().ok()? * scale;
    (secs.is_finite() && secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

fn code(c: i32, out: Out) -> ExtResponse {
    ExtResponse {
        code: c,
        stdout: out.stdout,
        stderr: out.stderr,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    /// A unique temp dir per test.
    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hnl-ext-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unknown_flag_is_usage_error() {
        let res = run(&["--bogus".into()], Path::new("."));
        assert_eq!(res.code, 2);
        assert!(res.stderr.contains("test: unknown flag `--bogus`"));
    }

    #[test]
    fn value_flags_require_a_value() {
        for flag in ["--filter", "--tag", "--jobs", "--report", "--timeout"] {
            let res = run(&[flag.into()], Path::new("."));
            assert_eq!(res.code, 2, "{flag}");
            assert!(
                res.stderr
                    .contains(&format!("test: {flag} requires a value"))
            );
        }
    }

    #[test]
    fn timeout_rejects_non_positive_and_garbage_values() {
        for bad in ["0", "-1", "abc", "30x", "ms"] {
            let res = run(
                &["--timeout".into(), bad.into(), ".".into()],
                Path::new("."),
            );
            assert_eq!(res.code, 2, "--timeout {bad}");
            assert!(
                res.stderr.contains("--timeout requires a positive duration"),
                "stderr: {}",
                res.stderr
            );
        }
    }

    #[test]
    fn parse_timeout_accepts_seconds_and_unit_suffixes() {
        assert_eq!(parse_timeout("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_timeout("1.5"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_timeout("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_timeout("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_timeout(" 2s "), Some(Duration::from_secs(2)));
        assert_eq!(parse_timeout("0"), None);
        assert_eq!(parse_timeout("-5"), None);
        assert_eq!(parse_timeout("abc"), None);
        assert_eq!(parse_timeout("30m"), None);
    }

    #[test]
    fn watch_rejects_list_and_changed() {
        // Both combos fail fast before the watcher is set up — `run` would
        // otherwise block forever in the watch loop.
        let res = run(&["--watch".into(), "--list".into()], Path::new("."));
        assert_eq!(res.code, 2);
        assert!(
            res.stderr
                .contains("--watch cannot be combined with --list")
        );

        let res = run(&["--watch".into(), "--changed".into()], Path::new("."));
        assert_eq!(res.code, 2);
        assert!(
            res.stderr
                .contains("--watch cannot be combined with --changed")
        );
    }

    #[test]
    fn list_on_an_empty_tree_returns_no_cases() {
        let dir = tmpdir("empty");
        let res = run(&["--list".into(), dir.display().to_string()], &dir);
        assert_eq!(res.code, 0, "stderr: {}", res.stderr);
        assert_eq!(res.stdout, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn coverage_flag_writes_json_report() {
        let dir = tmpdir("cov");
        let out = dir.join("cov.json");
        let res = run(
            &[
                format!("--coverage={}", out.display()),
                dir.display().to_string(),
            ],
            &dir,
        );
        assert_eq!(res.code, 0, "stderr: {}", res.stderr);
        assert!(
            res.stderr.contains("coverage written"),
            "stderr: {}",
            res.stderr
        );
        let text = std::fs::read_to_string(&out).expect("coverage json written");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        // The report labels its coordinate space and carries the merged
        // per-source table (empty on a tree with no `.hnl` tests).
        assert!(json.get("coordinateSpace").is_some(), "{json}");
        assert!(json["sources"].is_array(), "{json}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The `export!`-generated manifest symbol, called directly.
    #[test]
    fn manifest_declares_test() {
        let raw = handl_ext_manifest();
        assert!(!raw.is_null());
        let json = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe { handl_ext_free(raw) };
        let manifest: ExtensionManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest.name, "test");
        let names: Vec<&str> = manifest.commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["test"]);
    }
}
