//! The assistant tier: draft posts from context you already have.
//!
//! Three backends, one verb. Yapper does not ship an API key, a gateway or a
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
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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

const CLAUDE: Cli = Cli {
    command: "claude",
    // `--restricted` drops the tools that run commands or code: a prose prompt
    // needs none of them, and the context here is the user's own notes.
    // `--no-session-persistence` keeps a drafting turn out of their history.
    // Deliberately NOT `--bare`: it forces ANTHROPIC_API_KEY auth and would
    // break every user signed in with a subscription.
    args: &["-p", "--restricted", "--no-session-persistence"],
    probe: &["--version"],
    model_flag: "--model",
    effort_flag: &["--effort"],
    uses_output_file: false,
};

const CODEX: Cli = Cli {
    command: "codex",
    // `codex exec` otherwise boots the user's whole session — plugins with their
    // MCP servers, hooks, memories and apps — several thousand tokens and
    // seconds of startup a one-shot prompt never uses. All four are stable
    // flags; an unknown one on some future codex fails the draft rather than
    // silently drafting with the wrong context.
    args: &[
        "exec",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--disable",
        "plugins",
        "--disable",
        "hooks",
        "--disable",
        "memories",
        "--disable",
        "apps",
        "-c",
        "notify=[]",
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
                        "`{}` is not on Yapper's PATH, or it is not signed in.",
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
    let output = Command::new(cli.command)
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
            "{} answered, but not with anything Yapper could read as a draft. \
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
/// `std::process::Command` has no timeout, so the wait happens on a watchdog
/// thread and the child is killed when it runs over. Without that, a CLI waiting
/// on an auth prompt it can never receive would hold the request forever.
fn run_cli(cli: &Cli, prompt: &str, model: Option<&str>, effort: Option<&str>) -> Result<String> {
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

    let answer_file = if cli.uses_output_file {
        let dir = std::env::temp_dir().join(format!("yapper-draft-{}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::Internal(format!("Could not make a scratch directory: {e}")))?;
        let path = dir.join("answer.md");
        args.push("--output-last-message".to_string());
        args.push(path.to_string_lossy().to_string());
        Some(path)
    } else {
        None
    };

    let mut child = Command::new(cli.command)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            AppError::InvalidInput(format!(
                "Could not run `{}` ({err}). Install it, or pick a different assistant \
                 in Settings.",
                cli.command
            ))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        // A broken pipe here means the child exited before reading the prompt;
        // its own error message is the useful one, so this is not raised.
        let _ = stdin.write_all(prompt.as_bytes());
    }

    let output = wait_with_timeout(child, TIMEOUT)
        .ok_or_else(|| {
            AppError::Platform(format!(
                "`{}` did not answer within {} seconds.",
                cli.command,
                TIMEOUT.as_secs()
            ))
        })?
        .map_err(|err| {
            AppError::Internal(format!("Could not read `{}`'s answer: {err}", cli.command))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Platform(format!(
            "`{}` failed: {}",
            cli.command,
            last_meaningful_line(&stderr)
        )));
    }

    let answer = match &answer_file {
        Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
        None => String::from_utf8_lossy(&output.stdout).to_string(),
    };
    if let Some(path) = answer_file {
        let _ = std::fs::remove_file(path);
    }
    Ok(answer)
}

/// Waits for a child, killing it past the deadline. `None` means it was killed.
fn wait_with_timeout(
    child: std::process::Child,
    timeout: Duration,
) -> Option<std::io::Result<std::process::Output>> {
    let (done, waited) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = child.wait_with_output();
        // The receiver is gone once the deadline passed; the send failing is
        // exactly that case and needs no handling.
        let _ = done.send(result);
    });

    match waited.recv_timeout(timeout) {
        Ok(result) => {
            let _ = handle.join();
            Some(result)
        }
        Err(_) => {
            // `wait_with_output` consumed the child, so it cannot be killed by
            // handle here — the process is orphaned and will exit on its own
            // when its pipes close. Both CLIs are short-lived and read stdin to
            // EOF, so this is a bounded leak rather than an unbounded one.
            None
        }
    }
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
