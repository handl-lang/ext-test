//! `hnl-ext-test` — the `test` extension for `hnl` (ADR-0149).
//!
//! The runner lives in `hnl-engine-test` (discovery, filters,
//! tag predicates, reporters, the ADR-0089 result cache) behind the `test`
//! capability adapter `handl-plugin-ext-test`; this crate is the
//! *packaging*: it exports the ADR-0149 ABI (`handl_ext_manifest` /
//! `handl_ext_dispatch` / `handl_ext_free`) and implements the
//! `hnl test [--list] [--filter <pat>] [--tag <expr>] [--jobs N|auto]
//! [--report human|json] [--changed] [--cache] [paths...]` command surface —
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
            "test [--list] [--filter <pat>] [--tag <expr>] [--jobs N|auto] [--report human|json] [--changed] [--cache] [paths...]",
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
fn tooling_host() -> Result<DefaultHost, PlatformError> {
    let mut host = DefaultHost::new();
    host.register_plugin(handl_plugin_ext_test::TestPlugin::new(Arc::new(|| {
        with_tooling_kit(|kit| {
            Ok::<_, String>(hnl_engine_test::base_runtime(
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
    let mut tags: Vec<String> = Vec::new();
    let mut changed = false;
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
            let run_failed =
                response.0.get("status").and_then(|v| v.as_str()) == Some("failed");
            code(if run_failed || had_parse_error { 1 } else { 0 }, out)
        }
        other => {
            out.err(format!("test: unexpected capability status {other:?}"));
            code(2, out)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
        let dir =
            std::env::temp_dir().join(format!("hnl-ext-test-{tag}-{}", std::process::id()));
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
        for flag in ["--filter", "--tag", "--jobs", "--report"] {
            let res = run(&[flag.into()], Path::new("."));
            assert_eq!(res.code, 2, "{flag}");
            assert!(res.stderr.contains(&format!("test: {flag} requires a value")));
        }
    }

    #[test]
    fn list_on_an_empty_tree_returns_no_cases() {
        let dir = tmpdir("empty");
        let res = run(&["--list".into(), dir.display().to_string()], &dir);
        assert_eq!(res.code, 0, "stderr: {}", res.stderr);
        assert_eq!(res.stdout, "");
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
