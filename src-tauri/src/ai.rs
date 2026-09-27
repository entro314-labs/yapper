//! The assistant tier: draft posts from context you already have.
//!
//! Three backends, one verb. Windbag does not ship an API key, a gateway or a
//! model catalogue — it borrows an assistant the user already has:
//!
//!   * **Apple Intelligence** — on-device, keyless, offline, free. Through
//!     `tauri-plugin-apple-intelligence`'s Rust API, so this file is the only
//!     place any of it appears.
//!   * **Claude Code** (`claude -p`) and **Codex** (`codex exec`) — the CLIs on
//!     the user's PATH, invoked one-shot with the prompt on stdin. The pattern
//!     is release-kit's: a table of tools, each a command that reads a prompt
//!     and writes text, with every failure mode collapsing to one error rather
//!     than to a half-answer.
//!
//! **Off by default.** An assistant panel that errors on first click because
//! nothing is configured is worse than one you opted into, so `ai_backend` is
//! `off` until Settings says otherwise and the surfaces stay hidden.
//!
//! **The assistant never schedules anything.** It returns drafts; they land in
//! the composer, where the same validation and the same human click that guard
//! every other post still apply. Full agency lives in the MCP server, where an
//! agent host shows each tool call to the person running it.

use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{AppError, Result};

/// How long a draft may take before it is abandoned. Matches release-kit: long
/// enough for a reasoning model on a cold start, short enough that a wedged CLI
/// does not hold the UI open indefinitely.
const TIMEOUT: Duration = Duration::from_secs(180);

pub const META_BACKEND: &str = "ai_backend";
pub const META_MODEL: &str = "ai_model";
pub const META_EFFORT: &str = "ai_effort";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Off,
    Apple,
    Claude,
    Codex,
}

impl Backend {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("apple") => Self::Apple,
            Some("claude") => Self::Claude,
            Some("codex") => Self::Codex,
            _ => Self::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Apple => "apple",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Apple => "Apple Intelligence",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

/// Whether a backend can be used right now, and why not when it cannot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    pub backend: Backend,
    pub label: &'static str,
    pub available: bool,
    /// The framework's or the tool's own words. Shown verbatim in Settings — an
    /// assistant that is "unavailable" with no reason is unfixable.
    pub reason: String,
}

/// What the assistant is asked for. Everything is plain text: none of the three
/// backends shares a message format, so the one thing they can all take is a
/// prompt.
pub struct DraftRequest {
    /// The material to write from: notes, a pasted brief, a link.
    pub context: String,
    /// What to do with it.
    pub instructions: String,
    /// The destinations the drafts must fit, as `("Bluesky", 300)` pairs. Sent
    /// so the model writes to the limit rather than being cut to it afterwards.
    pub destinations: Vec<(String, usize)>,
    /// How many variants to return.
    pub count: usize,
}

/// One draft the assistant produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// One line on why this angle — what makes three variants a choice rather
    /// than three rolls of the same dice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

// ─── The CLI table ──────────────────────────────────────────────────────────

/// One command-line assistant. Supporting another harness is one row here.
struct Cli {
    command: &'static str,
    args: &'static [&'static str],
    probe: &'static [&'static str],
    model_flag: &'static str,
    effort_flag: &'static [&'static str],
    /// Tools whose stdout carries session scaffolding (MCP notices, hook logs)
    /// hand the answer back through a file instead.
    uses_output_file: bool,
}

// Both CLIs run with no tools at all. The material is the user's notes, which
// can carry text written by someone else (a pasted thread, a link's preview),
// and a draft is written straight back into the app: a model that can read
// files, fetch URLs or call a connector turns a prompt injection in a note into
// a draft carrying the contents of ~/.ssh. Each flag below was checked against
// the installed CLI's `--help` and a live session's reported tool list.

const CLAUDE: Cli = Cli {
    command: "claude",
    // Checked against Claude Code 2.1.283. `--restricted` alone still left the
    // file tools (Read, Write, Edit, Glob, Grep), WebSearch and every MCP server
    // including claude.ai connectors; with the two flags after it the session
    // reports `tools: []` and `mcp_servers: []`.
    //   * `--restricted` ignores user, project and local settings files — the
    //     hooks they declare included.
    //   * `--no-session-persistence` keeps a drafting turn out of their history.
    //   * `--tools ""` removes every built-in tool.
    //   * `--strict-mcp-config` loads MCP servers only from `--mcp-config`,
    //     which is never passed, so none.
    // Deliberately NOT `--bare`: it forces ANTHROPIC_API_KEY auth and would
    // break every user signed in with a subscription.
    args: &[
        "-p",
        "--restricted",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
    ],
    probe: &["--version"],
    model_flag: "--model",
    effort_flag: &["--effort"],
    uses_output_file: false,
};

const CODEX: Cli = Cli {
    command: "codex",
    // Checked against codex-cli 0.157.1. `codex exec` otherwise boots the
    // user's whole session: config.toml's MCP servers and notify hook, plugins,
    // hooks, memories, apps, a shell, a code-mode runtime, image viewing and web
    // search. With all of this the model reports no shell and no file access,
    // and a call to `exec` fails closed.
    //   * `--ignore-user-config` skips config.toml — the only way to drop its
    //     `[mcp_servers]`: `-c mcp_servers={}` merges into them rather than
    //     replacing them. Auth still comes from CODEX_HOME. The user's default
    //     model and effort go with it; Settings → Assistant supplies both.
    //   * `--ephemeral` keeps the turn out of their session history.
    //   * `--sandbox read-only` backstops anything that still executes.
    //   * `--disable` turns off features on by default: `plugins`, `apps`
    //     (connectors), `hooks`, `memories`, `shell_tool` and `unified_exec`
    //     (commands), `code_mode_host` (the `exec` code runtime), `view_image`
    //     (reads local images), `browser_use`, `computer_use` and
    //     `image_generation`.
    //   * `web_search="disabled"` turns off the hosted search tool.
    // An unknown feature is a hard error ("Unknown feature flag"), so a codex
    // too old for one of these fails the draft with that message rather than
    // drafting with tools. Codex keeps removed features in its table, so a
    // newer one keeps accepting them.
    args: &[
        "exec",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ephemeral",
        "--sandbox",
        "read-only",
        "--disable",
        "plugins",
        "--disable",
        "apps",
        "--disable",
        "hooks",
        "--disable",
        "memories",
        "--disable",
        "shell_tool",
        "--disable",
        "unified_exec",
        "--disable",
        "code_mode_host",
        "--disable",
        "view_image",
        "--disable",
        "browser_use",
        "--disable",
        "computer_use",
        "--disable",
        "image_generation",
        "-c",
        "web_search=\"disabled\"",
    ],
    probe: &["--version"],
    model_flag: "-m",
    effort_flag: &["-c", "model_reasoning_effort"],
    uses_output_file: true,
};

fn cli_for(backend: Backend) -> Option<Cli> {
    match backend {
        Backend::Claude => Some(CLAUDE),
        Backend::Codex => Some(CODEX),
        _ => None,
    }
}

// ─── Availability ───────────────────────────────────────────────────────────

pub fn availability(app: &AppHandle, backend: Backend) -> Availability {
    let (available, reason) = match backend {
        Backend::Off => (false, "The assistant is switched off.".to_string()),
        Backend::Apple => apple_availability(app),
        Backend::Claude | Backend::Codex => {
            let cli = cli_for(backend).unwrap_or(CLAUDE);
            match probe(&cli) {
                Some(version) => (true, version),
                None => (
                    false,
                    format!(
                        "`{}` is not on Windbag's PATH, or it is not signed in.",
                        cli.command
                    ),
                ),
            }
        }
    };
    Availability {
        backend,
        label: backend.label(),
        available,
        reason,
    }
}

/// Every backend's availability at once, for the Settings picker.
pub fn all_availability(app: &AppHandle) -> Vec<Availability> {
    [Backend::Apple, Backend::Claude, Backend::Codex]
        .into_iter()
        .map(|backend| availability(app, backend))
        .collect()
}

#[cfg(target_os = "macos")]
fn apple_availability(app: &AppHandle) -> (bool, String) {
    use tauri_plugin_apple_intelligence::AppleIntelligenceExt;

    match app.apple_intelligence().check_availability() {
        Ok(status) => (
            status.available,
            if status.available {
                "Apple's on-device model is ready.".to_string()
            } else {
                status.reason
            },
        ),
        Err(err) => (false, format!("Apple Intelligence is unavailable: {err}")),
    }
}

#[cfg(not(target_os = "macos"))]
fn apple_availability(_app: &AppHandle) -> (bool, String) {
    (
        false,
        "Apple Intelligence needs macOS on Apple silicon.".to_string(),
    )
}

/// A version probe that answers is the only reliable "installed and runnable"
/// signal — a `which` hit can still be a broken shim.
fn probe(cli: &Cli) -> Option<String> {
    let output = cli_command(cli.command)
        .args(cli.probe)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(if version.is_empty() {
        cli.command.to_string()
    } else {
        version
    })
}

// ─── Finding the CLIs ───────────────────────────────────────────────────────

/// How long the login shell may take to report its PATH. Generous — an rc file
/// that loads nvm, pyenv and a prompt theme takes seconds — but bounded, since
/// an rc file can also wait on input it will never get.
#[cfg(not(windows))]
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);

/// Brackets the PATH in the shell's output. Login and interactive shells print
/// whatever their rc files print (a "Last login" line, a fortune, job-control
/// warnings with no terminal); only what sits between two of these is PATH.
#[cfg(not(windows))]
const PATH_SENTINEL: &str = "__WINDBAG_PATH__";

/// A command for one assistant CLI, found and run with [`cli_path`].
///
/// PATH is set on the child rather than resolved to an absolute program path
/// because the child needs it too: an npm-installed `codex` is a
/// `#!/usr/bin/env node` script, and `env` searches the PATH it inherits. On
/// Unix, std searches a PATH set on the command when it looks up the program.
fn cli_command(program: &str) -> Command {
    let mut command = Command::new(program);
    if let Some(path) = cli_path() {
        command.env("PATH", path);
    }
    command
}

/// The PATH the assistant CLIs live on, or `None` to inherit Windbag's own.
///
/// An app launched from Finder, the Dock or at login inherits launchd's PATH,
/// `/usr/bin:/bin:/usr/sbin:/sbin` — none of the places `claude` and `codex`
/// install to (`~/.local/bin`, Homebrew, npm's prefix). The user's login
/// shell knows them, so it is asked once and the answer kept for the session.
/// Nothing is written to Windbag's own environment: the rest of the process
/// has no business running things from the user's PATH.
#[cfg(not(windows))]
fn cli_path() -> Option<&'static std::ffi::OsStr> {
    static PATH: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let Some(shell) = std::env::var_os("SHELL").filter(|shell| !shell.is_empty()) else {
            log::warn!("SHELL is unset; the assistant CLIs are looked up on the inherited PATH");
            return None;
        };
        let path = login_shell_path(&shell);
        if path.is_none() {
            log::warn!(
                "{} did not report a PATH; the assistant CLIs are looked up on the inherited PATH",
                shell.to_string_lossy()
            );
        }
        path
    })
    .as_deref()
}

/// Windows GUI apps inherit the user's full PATH from the registry, so there is
/// nothing to resolve.
#[cfg(windows)]
fn cli_path() -> Option<&'static std::ffi::OsStr> {
    None
}

/// Asks `shell`, started as a login and interactive shell so both its profile
/// and its rc file run, for its PATH. `None` when it fails, hangs or prints
/// no bracketed PATH — `csh` and `nu` reject the invocation, for instance.
#[cfg(not(windows))]
fn login_shell_path(shell: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let mut command = Command::new(shell);
    command.args([
        "-l",
        "-i",
        "-c",
        &format!(r#"printf '%s%s%s' {PATH_SENTINEL} "$PATH" {PATH_SENTINEL}"#),
    ]);
    if let Some(home) = dirs::home_dir() {
        command.current_dir(home);
    }
    let finished = match run_with_timeout(command, b"", SHELL_TIMEOUT) {
        Ok(Some(finished)) => finished,
        Ok(None) => {
            log::warn!("the login shell did not answer within {SHELL_TIMEOUT:?}");
            return None;
        }
        Err(err) => {
            log::warn!("could not start the login shell: {err}");
            return None;
        }
    };
    parse_login_path(&String::from_utf8_lossy(&finished.output.stdout))
        .map(std::ffi::OsString::from)
}

/// The PATH between the first two sentinels, when there is a non-blank one.
#[cfg(not(windows))]
fn parse_login_path(output: &str) -> Option<String> {
    let (_, rest) = output.split_once(PATH_SENTINEL)?;
    let (path, _) = rest.split_once(PATH_SENTINEL)?;
    let path = path.trim();
    (!path.is_empty()).then(|| path.to_string())
}

// ─── Drafting ───────────────────────────────────────────────────────────────

/// Asks the configured backend for `request.count` drafts.
///
/// Returns whatever the model produced, parsed out of its answer. A backend that
/// is missing, unauthenticated, over quota or slow all arrive here as one error
/// carrying the tool's own last words — see [`run_cli`].
pub fn suggest(
    app: &AppHandle,
    backend: Backend,
    model: Option<&str>,
    effort: Option<&str>,
    request: &DraftRequest,
) -> Result<Vec<Suggestion>> {
    if backend == Backend::Off {
        return Err(AppError::InvalidInput(
            "No assistant is configured. Pick one in Settings → Assistant.".into(),
        ));
    }

    let prompt = build_prompt(request);
    let answer = match backend {
        Backend::Apple => run_apple(app, &prompt)?,
        Backend::Claude | Backend::Codex => {
            let cli = cli_for(backend).ok_or_else(|| {
                AppError::Internal("No command is registered for this assistant.".into())
            })?;
            run_cli(&cli, &prompt, model, effort)?
        }
        Backend::Off => unreachable!("guarded above"),
    };

    let suggestions = parse_suggestions(&answer);
    if suggestions.is_empty() {
        return Err(AppError::Platform(format!(
            "{} answered, but not with anything Windbag could read as a draft. \
             Try again, or a different assistant.",
            backend.label()
        )));
    }
    Ok(suggestions)
}

/// The prompt. One string for all three backends: none of them shares a message
/// format, and a prompt that reads well to a person reads well to all of them.
fn build_prompt(request: &DraftRequest) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are drafting social media posts. Write in the voice of the material you are \
         given — do not add enthusiasm, hashtags or emoji that are not already there.\n\n",
    );

    if !request.destinations.is_empty() {
        prompt.push_str("These are the destinations and their hard character limits:\n");
        for (name, limit) in &request.destinations {
            let _ = writeln!(prompt, "- {name}: {limit} characters");
        }
        // The tightest limit, stated as one number, because a model given five
        // limits writes to the loosest and the post is then rejected.
        if let Some(tightest) = request.destinations.iter().map(|(_, limit)| *limit).min() {
            let _ = writeln!(
                prompt,
                "\nEvery draft MUST be at most {tightest} characters so it fits all of them."
            );
        }
        prompt.push('\n');
    }

    prompt.push_str("--- MATERIAL ---\n");
    prompt.push_str(request.context.trim());
    prompt.push_str("\n--- END MATERIAL ---\n\n");

    if !request.instructions.trim().is_empty() {
        prompt.push_str(request.instructions.trim());
        prompt.push_str("\n\n");
    }

    let _ = writeln!(
        prompt,
        "Write {} DIFFERENT drafts — different angles on the material, not rewordings of \
         one another.\n\n\
         Reply with ONLY a JSON array, no prose before or after, no code fence:\n\
         [{{\"body\": \"the post text\", \"rationale\": \"one short line on the angle\"}}]",
        request.count.clamp(1, 5)
    );
    prompt
}

#[cfg(target_os = "macos")]
fn run_apple(app: &AppHandle, prompt: &str) -> Result<String> {
    use tauri_plugin_apple_intelligence::{
        AppleAIGenerateRequest, AppleAIMessage, AppleIntelligenceExt,
    };

    let request = AppleAIGenerateRequest {
        messages: vec![AppleAIMessage {
            role: "user".into(),
            content: Some(prompt.to_string()),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            images: None,
        }],
        tools: None,
        // Guided generation is deliberately not used: it has documented gaps
        // (properties a schema declares that the guide cannot carry are dropped)
        // and the text path has to work anyway for the two CLI backends. One
        // parser for all three beats a second shape that can rot on its own.
        schema: None,
        model: Some("on-device".into()),
        reasoning_level: None,
        temperature: Some(0.8),
        max_tokens: None,
        top_p: None,
        top_k: None,
        seed: None,
        tool_choice: None,
        stop_after_tool_calls: None,
    };

    let result = app
        .apple_intelligence()
        .generate(request)
        .map_err(|err| AppError::Platform(format!("Apple Intelligence: {err}")))?;
    Ok(result.text)
}

#[cfg(not(target_os = "macos"))]
fn run_apple(_app: &AppHandle, _prompt: &str) -> Result<String> {
    Err(AppError::InvalidInput(
        "Apple Intelligence needs macOS on Apple silicon.".into(),
    ))
}

/// Runs one CLI with the prompt on stdin.
///
/// `std::process::Command` has no timeout, so [`run_with_timeout`] enforces
/// one. Without it, a CLI waiting on an auth prompt it can never receive would
/// hold the request forever.
///
/// It runs in an empty directory made for this one draft. Inheriting the app's
/// working directory would put it in `/` for a bundled app, or in this
/// repository during development: places whose project config the CLI would
/// pick up, and whose path it describes to the model.
fn run_cli(cli: &Cli, prompt: &str, model: Option<&str>, effort: Option<&str>) -> Result<String> {
    let scratch = Scratch::new()
        .map_err(|e| AppError::Internal(format!("Could not make a scratch directory: {e}")))?;
    let answer_file = cli.uses_output_file.then(|| scratch.0.join("answer.md"));
    let args = build_args(cli, model, effort, answer_file.as_deref());

    let mut command = cli_command(cli.command);
    command.args(&args).current_dir(&scratch.0);
    let finished = run_with_timeout(command, prompt.as_bytes(), TIMEOUT)
        .map_err(|err| {
            AppError::InvalidInput(format!(
                "Could not run `{}` ({err}). Install it, or pick a different assistant \
                 in Settings.",
                cli.command
            ))
        })?
        .ok_or_else(|| {
            AppError::Platform(format!(
                "`{}` did not answer within {} seconds.",
                cli.command,
                TIMEOUT.as_secs()
            ))
        })?;
    check_finished(cli.command, &finished)?;

    match &answer_file {
        Some(path) => std::fs::read_to_string(path).map_err(|err| {
            AppError::Platform(format!(
                "`{}` finished without writing an answer: {err}",
                cli.command
            ))
        }),
        None => Ok(String::from_utf8_lossy(&finished.output.stdout).to_string()),
    }
}

/// The full argv for one draft: the table row, then the user's model and
/// effort, then where codex should write its answer.
fn build_args(
    cli: &Cli,
    model: Option<&str>,
    effort: Option<&str>,
    answer_file: Option<&Path>,
) -> Vec<String> {
    let mut args: Vec<String> = cli.args.iter().map(|arg| (*arg).to_string()).collect();
    if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
        args.push(cli.model_flag.to_string());
        args.push(model.to_string());
    }
    if let Some(effort) = effort.filter(|value| !value.trim().is_empty()) {
        match cli.effort_flag {
            // codex takes it as a config override: `-c model_reasoning_effort="high"`.
            [flag, key] => {
                args.push((*flag).to_string());
                args.push(format!("{key}=\"{effort}\""));
            }
            [flag] => {
                args.push((*flag).to_string());
                args.push(effort.to_string());
            }
            _ => {}
        }
    }
    if let Some(path) = answer_file {
        args.push("--output-last-message".to_string());
        args.push(path.to_string_lossy().to_string());
    }
    args
}

/// A directory that exists for one draft and is removed with it. Numbered, not
/// just per-process: two drafts in flight at once must not share codex's
/// answer file.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "windbag-draft-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            log::warn!(
                "could not remove the draft scratch directory {}: {err}",
                self.0.display()
            );
        }
    }
}

/// Whether a CLI that exited in time produced an answer worth reading.
///
/// A non-zero exit reports the tool's own last words, even when handing it the
/// prompt also failed: a CLI that quits before reading stdin (not signed in,
/// unknown flag) breaks the pipe as a side effect, and "broken pipe" tells the
/// user nothing the CLI's message does not. A zero exit with the prompt only
/// partly delivered is refused — that answer is to a question nobody asked.
fn check_finished(command: &str, finished: &Finished) -> Result<()> {
    if !finished.output.status.success() {
        let stderr = String::from_utf8_lossy(&finished.output.stderr);
        return Err(AppError::Platform(format!(
            "`{command}` failed: {}",
            last_meaningful_line(&stderr)
        )));
    }
    if let Err(err) = &finished.input {
        return Err(AppError::Platform(format!(
            "`{command}` exited before reading the whole prompt: {err}"
        )));
    }
    Ok(())
}

/// How often [`run_with_timeout`] looks at the child. Short enough that a fast
/// answer is not held back noticeably, long enough to cost nothing over 180 s.
const POLL: Duration = Duration::from_millis(50);

/// A child that exited within its deadline, with everything it wrote.
struct Finished {
    output: Output,
    /// Whether all of the input reached the child's stdin.
    input: std::io::Result<()>,
}

/// Runs `command` with `input` on stdin, killing it at the deadline.
///
/// `Ok(None)` means the deadline passed. The child is then killed and reaped
/// here rather than orphaned — a CLI waiting on an auth prompt it can never
/// receive would otherwise sit in the process table until logout.
///
/// Every blocking pipe operation runs on its own thread and is waited on only
/// until the deadline. Writing the prompt blocks for good when a child never
/// reads it and the prompt outgrows the pipe buffer; reading blocks until every
/// holder of the write end is gone, and a process the CLI started can inherit
/// stdout and outlive it. Neither may hold the request open. Only the direct
/// child is killed: its descendants would need a process-group signal, which
/// takes `unsafe` libc calls this crate does not make, so a descendant that
/// keeps a pipe open leaves its reader thread parked until it exits.
fn run_with_timeout(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
) -> std::io::Result<Option<Finished>> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let written = child.stdin.take().map(|mut stdin| {
        let input = input.to_vec();
        // Dropping `stdin` when the write ends closes the pipe: the EOF that
        // tells the CLI the prompt is complete.
        in_background(move || stdin.write_all(&input))
    });
    let stdout = child
        .stdout
        .take()
        .map(|pipe| in_background(move || read_all(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| in_background(move || read_all(pipe)));

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let now = Instant::now();
        if now >= deadline {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        std::thread::sleep(POLL.min(deadline - now));
    };

    let mut captured = [Vec::new(), Vec::new()];
    for (slot, pipe) in captured.iter_mut().zip([stdout, stderr]) {
        let Some(pipe) = pipe else { continue };
        match pipe.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(bytes) => *slot = bytes?,
            Err(_) => return Ok(None),
        }
    }
    let input = match written {
        Some(written) => {
            match written.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(result) => result,
                Err(_) => return Ok(None),
            }
        }
        None => Ok(()),
    };
    let [stdout, stderr] = captured;
    Ok(Some(Finished {
        output: Output {
            status,
            stdout,
            stderr,
        },
        input,
    }))
}

/// Runs `work` on its own thread; the receiver gets its result.
fn in_background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> mpsc::Receiver<T> {
    let (done, result) = mpsc::channel();
    std::thread::spawn(move || {
        // The receiver is gone once the deadline passed; the send failing is
        // exactly that case and needs no handling.
        let _ = done.send(work());
    });
    result
}

fn read_all(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The last line of stderr that says something. CLIs print banners, session ids
/// and warnings before the actual failure — "codex failed (exit 1)" tells nobody
/// they are over quota, and the quota line is the last one.
fn last_meaningful_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with("---"))
        .map_or_else(
            || "no output".to_string(),
            |line| line.chars().take(300).collect(),
        )
}

// ─── Answer parsing ─────────────────────────────────────────────────────────

/// Pulls drafts out of whatever the model actually sent.
///
/// Models wrap JSON in code fences, preface it with "Here are three drafts:",
/// and occasionally answer in prose despite the instruction. All three are
/// handled: the JSON array is located and parsed, and a plain-prose answer
/// degrades to a single draft rather than to an error — a usable draft the user
/// can edit beats a failure over formatting.
pub fn parse_suggestions(answer: &str) -> Vec<Suggestion> {
    let cleaned = strip_fences(answer);

    // A parsed array is the answer, whatever survives filtering. Falling through
    // to the prose path here would hand back the raw JSON as a "draft" when the
    // model returned an array of blanks.
    if let Some(json) = extract_array(&cleaned)
        && let Ok(parsed) = serde_json::from_str::<Vec<Suggestion>>(json)
    {
        return parsed
            .into_iter()
            .filter(|suggestion| !suggestion.body.trim().is_empty())
            .collect();
    }

    let prose = cleaned.trim();
    if prose.is_empty() {
        return Vec::new();
    }
    vec![Suggestion {
        body: prose.to_string(),
        title: None,
        rationale: None,
    }]
}

/// Removes a leading fenced-code marker and its closing partner.
fn strip_fences(text: &str) -> String {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_string();
    };
    // The opening fence may carry a language tag on the same line.
    let body = rest.split_once('\n').map_or("", |(_, after)| after);
    body.rsplit_once("```")
        .map_or(body, |(before, _)| before)
        .trim()
        .to_string()
}

/// The outermost `[...]` in a string, bracket-counted so an array containing
/// strings with brackets in them survives. Returns `None` when there is none.
fn extract_array(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find('[')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'[' if !in_string => depth += 1,
            b']' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return text.get(start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_json_array_parses() {
        let answer = r#"[{"body":"first","rationale":"the angle"},{"body":"second"}]"#;
        let parsed = parse_suggestions(answer);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].body, "first");
        assert_eq!(parsed[0].rationale.as_deref(), Some("the angle"));
    }

    #[test]
    fn a_fenced_array_parses() {
        let answer = "```json\n[{\"body\":\"inside a fence\"}]\n```";
        assert_eq!(parse_suggestions(answer)[0].body, "inside a fence");
    }

    #[test]
    fn prose_around_the_array_is_ignored() {
        let answer = "Here are two drafts:\n[{\"body\":\"kept\"}]\nHope these help!";
        let parsed = parse_suggestions(answer);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].body, "kept");
    }

    #[test]
    fn brackets_inside_a_draft_do_not_end_the_array_early() {
        let answer = r#"[{"body":"see [1] and [2]"},{"body":"second"}]"#;
        let parsed = parse_suggestions(answer);
        assert_eq!(
            parsed.len(),
            2,
            "a bracket in the text must not truncate the array"
        );
        assert_eq!(parsed[0].body, "see [1] and [2]");
    }

    #[test]
    fn an_escaped_quote_inside_a_draft_survives() {
        let answer = r#"[{"body":"they said \"no\" [twice]"}]"#;
        assert_eq!(
            parse_suggestions(answer)[0].body,
            r#"they said "no" [twice]"#
        );
    }

    #[test]
    fn a_prose_answer_degrades_to_one_usable_draft() {
        let parsed = parse_suggestions("Shipping today. No JSON in sight.");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].body, "Shipping today. No JSON in sight.");
        assert!(parsed[0].rationale.is_none());
    }

    #[test]
    fn an_empty_answer_yields_nothing_rather_than_an_empty_draft() {
        assert!(parse_suggestions("   \n  ").is_empty());
        assert!(parse_suggestions(r#"[{"body":"  "}]"#).is_empty());
    }

    #[test]
    fn the_prompt_states_the_tightest_limit_as_a_single_number() {
        let prompt = build_prompt(&DraftRequest {
            context: "some material".into(),
            instructions: String::new(),
            destinations: vec![("Bluesky".into(), 300), ("LinkedIn".into(), 3000)],
            count: 3,
        });
        assert!(prompt.contains("at most 300 characters"), "{prompt}");
        assert!(prompt.contains("- LinkedIn: 3000 characters"));
    }

    #[test]
    fn the_draft_count_is_clamped_to_something_sane() {
        let request = |count| DraftRequest {
            context: "x".into(),
            instructions: String::new(),
            destinations: Vec::new(),
            count,
        };
        assert!(build_prompt(&request(99)).contains("Write 5 DIFFERENT"));
        assert!(build_prompt(&request(0)).contains("Write 1 DIFFERENT"));
    }

    #[test]
    fn the_quota_line_is_what_a_failure_reports() {
        let stderr = "OpenAI Codex v0.150.1\n--------\nmodel: gpt-5.6-sol\n\
                      ERROR: You've hit your usage limit. Try again at 12:07 PM.";
        assert!(last_meaningful_line(stderr).contains("usage limit"));
    }

    #[test]
    fn an_empty_stderr_still_says_something() {
        assert_eq!(last_meaningful_line("\n  \n"), "no output");
    }

    /// A scratch path unique to one test, so parallel tests never share files.
    #[cfg(unix)]
    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("windbag-ai-test-{}-{name}", std::process::id()))
    }

    #[cfg(unix)]
    #[test]
    fn a_child_past_the_deadline_is_killed_not_orphaned() {
        let marker = scratch("killed-marker");
        let _ = std::fs::remove_file(&marker);
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("sleep 1; touch '{}'", marker.display()));

        let started = std::time::Instant::now();
        let finished = run_with_timeout(command, b"", Duration::from_millis(100)).expect("spawns");
        assert!(
            finished.is_none(),
            "a run past its deadline reports a timeout"
        );
        assert!(started.elapsed() < Duration::from_secs(1));

        // Had the shell survived the deadline, it would write the marker here.
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the timed-out child kept running");
    }

    #[cfg(unix)]
    #[test]
    fn a_grandchild_holding_the_pipes_does_not_outlive_the_deadline() {
        // The shell exits at once, but the backgrounded sleep inherits stdout and
        // keeps it open for 30 s — a reader waiting for EOF would wait that long.
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 30 & echo started");

        let started = std::time::Instant::now();
        let finished = run_with_timeout(command, b"", Duration::from_millis(300)).expect("spawns");
        assert!(finished.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_finishes_in_time_hands_back_its_output() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("cat; echo oops >&2");

        let finished = run_with_timeout(command, b"the prompt", Duration::from_secs(10))
            .expect("spawns")
            .expect("finishes in time");
        assert!(finished.output.status.success());
        assert!(finished.input.is_ok());
        assert_eq!(finished.output.stdout, b"the prompt");
        assert_eq!(finished.output.stderr, b"oops\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_prompt_the_child_never_read_is_reported_not_dropped() {
        // More than any pipe buffer holds, to a child that exits without reading.
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("echo 'Error: not signed in' >&2; exit 3");

        let finished = run_with_timeout(command, &vec![b'x'; 1 << 20], Duration::from_secs(10))
            .expect("spawns")
            .expect("finishes in time");
        assert_eq!(finished.output.status.code(), Some(3));
        assert!(finished.input.is_err(), "the broken pipe was swallowed");
    }

    #[cfg(unix)]
    fn finished(code: i32, input: std::io::Result<()>, stderr: &str) -> Finished {
        use std::os::unix::process::ExitStatusExt;
        Finished {
            output: Output {
                // A wait status carries the exit code in its second byte.
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: Vec::new(),
                stderr: stderr.as_bytes().to_vec(),
            },
            input,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_cli_reports_its_own_words_over_the_broken_pipe() {
        let broken = Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        let err = check_finished("claude", &finished(1, broken, "Error: not signed in\n"))
            .expect_err("a non-zero exit fails");
        let message = err.to_string();
        assert!(message.contains("not signed in"), "{message}");
        assert!(!message.to_lowercase().contains("broken pipe"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_that_succeeded_without_the_whole_prompt_is_not_trusted() {
        let broken = Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        let err = check_finished("codex", &finished(0, broken, ""))
            .expect_err("an answer to half a prompt is not an answer");
        assert!(err.to_string().contains("prompt"), "{err}");
        assert!(check_finished("codex", &finished(0, Ok(()), "")).is_ok());
    }

    /// Whether `args` carries `flag` immediately followed by `value`.
    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value)
    }

    #[test]
    fn claude_runs_with_no_tools_and_no_mcp_servers() {
        let args = build_args(&CLAUDE, Some("sonnet"), Some("high"), None);
        assert_eq!(args[0], "-p");
        assert!(has_pair(&args, "--tools", ""), "{args:?}");
        assert!(args.iter().any(|arg| arg == "--strict-mcp-config"));
        assert!(!args.iter().any(|arg| arg == "--mcp-config"));
        assert!(args.iter().any(|arg| arg == "--restricted"));
        assert!(args.iter().any(|arg| arg == "--no-session-persistence"));
        assert!(has_pair(&args, "--model", "sonnet"));
        assert!(has_pair(&args, "--effort", "high"));
        assert!(!args.iter().any(|arg| arg == "--output-last-message"));
    }

    #[test]
    fn codex_runs_sandboxed_without_user_config_or_tools() {
        let answer = Path::new("/tmp/windbag-draft-1-0/answer.md");
        let args = build_args(&CODEX, Some("gpt-5"), Some("low"), Some(answer));
        assert_eq!(args[0], "exec");
        assert!(args.iter().any(|arg| arg == "--ignore-user-config"));
        assert!(args.iter().any(|arg| arg == "--ephemeral"));
        assert!(has_pair(&args, "--sandbox", "read-only"));
        for feature in [
            "plugins",
            "apps",
            "hooks",
            "memories",
            "shell_tool",
            "unified_exec",
            "code_mode_host",
            "view_image",
            "browser_use",
            "computer_use",
            "image_generation",
        ] {
            assert!(has_pair(&args, "--disable", feature), "{feature} stays on");
        }
        assert!(has_pair(&args, "-c", r#"web_search="disabled""#));
        assert!(has_pair(&args, "-m", "gpt-5"));
        assert!(has_pair(&args, "-c", r#"model_reasoning_effort="low""#));
        assert!(has_pair(
            &args,
            "--output-last-message",
            "/tmp/windbag-draft-1-0/answer.md"
        ));
    }

    #[test]
    fn a_blank_model_or_effort_is_left_to_the_cli() {
        let args = build_args(&CLAUDE, Some("  "), Some(""), None);
        assert!(!args.iter().any(|arg| arg == "--model" || arg == "--effort"));
    }

    #[test]
    fn concurrent_drafts_get_separate_empty_directories() {
        let first = Scratch::new().expect("scratch");
        let second = Scratch::new().expect("scratch");
        assert_ne!(first.0, second.0);
        assert_eq!(std::fs::read_dir(&first.0).expect("exists").count(), 0);
        let path = first.0.clone();
        drop(first);
        assert!(!path.exists(), "the scratch directory outlived its draft");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_login_path_is_read_between_the_sentinels_despite_shell_noise() {
        let output = format!(
            "Last login: Sat Sep 27 on ttys001\nzsh: no job control\n\
             {PATH_SENTINEL}/Users/me/.local/bin:/opt/homebrew/bin:/usr/bin{PATH_SENTINEL}\
             \nfortune: a banner the rc file prints on exit\n"
        );
        assert_eq!(
            parse_login_path(&output).as_deref(),
            Some("/Users/me/.local/bin:/opt/homebrew/bin:/usr/bin")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_shell_that_never_printed_the_path_yields_nothing() {
        assert!(parse_login_path("").is_none());
        assert!(parse_login_path("zsh: command not found: printf").is_none());
        assert!(parse_login_path(&format!("{PATH_SENTINEL}/usr/bin")).is_none());
        assert!(parse_login_path(&format!("{PATH_SENTINEL}{PATH_SENTINEL}")).is_none());
        assert!(parse_login_path(&format!("{PATH_SENTINEL}  \n{PATH_SENTINEL}")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_login_shell_reports_its_path() {
        let path = login_shell_path(std::ffi::OsStr::new("/bin/sh")).expect("sh prints a PATH");
        assert!(!path.is_empty());
        assert!(!path.to_string_lossy().contains(PATH_SENTINEL));
    }

    #[test]
    fn cli_commands_are_found_and_run_with_the_resolved_path() {
        let command = cli_command("claude");
        let path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value);
        assert_eq!(path, cli_path());
    }

    #[test]
    fn backends_round_trip_through_their_stored_names() {
        for backend in [
            Backend::Off,
            Backend::Apple,
            Backend::Claude,
            Backend::Codex,
        ] {
            assert_eq!(Backend::parse(Some(backend.as_str())), backend);
        }
        assert_eq!(Backend::parse(None), Backend::Off);
        assert_eq!(Backend::parse(Some("nonsense")), Backend::Off);
    }
}
