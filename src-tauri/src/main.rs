// The Windows release build is a GUI app: without this it opens a console window
// behind the real one. Redirected stdio still works under the GUI subsystem, which
// is all `--mcp` needs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `windbag` starts the app; `windbag --mcp` is the agent door instead — a stdio
//! MCP server over the same SQLite store, spawnable by any agent host:
//!
//!   claude mcp add windbag -- /Applications/Windbag.app/Contents/MacOS/windbag --mcp
//!
//! The same executable rather than a second one, so every installed build ships
//! it and Settings can show its real path. It works whether or not the app is
//! running — WAL makes the concurrent access safe — but it cannot publish:
//! publishing is the running app's scheduler thread, and tools that queue
//! something say so themselves.
//!
//! stdout carries protocol frames only — anything else on it corrupts the
//! stream — so every diagnostic goes to stderr, and no logger is installed.

use std::io::{BufRead, Write};
use std::sync::Arc;

use windbag_lib::{db, mcp};

fn main() {
    if std::env::args().nth(1).as_deref() == Some(windbag_lib::MCP_FLAG) {
        serve_mcp();
    } else {
        windbag_lib::run();
    }
}

fn serve_mcp() {
    let path = match db::data_dir() {
        Ok(dir) => dir.join("windbag.sqlite3"),
        Err(err) => {
            eprintln!("windbag --mcp: no data directory: {err}");
            std::process::exit(1);
        }
    };
    let database = match db::Db::open_at(&path) {
        Ok(database) => Arc::new(database),
        Err(err) => {
            eprintln!(
                "windbag --mcp: cannot open the store at {} ({err}) — run the Windbag app \
                 once first.",
                path.display()
            );
            std::process::exit(1);
        }
    };

    let mut session = mcp::Session::new(database);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break; // the host closed the pipe
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                let frame = serde_json::json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {err}") }
                });
                if write_frame(&mut stdout, &frame).is_err() {
                    break;
                }
                continue;
            }
        };
        if let Some(reply) = session.handle(&message)
            && write_frame(&mut stdout, &reply).is_err()
        {
            break; // the host is gone; there is nothing left to serve
        }
    }
}

fn write_frame(out: &mut impl Write, frame: &serde_json::Value) -> std::io::Result<()> {
    writeln!(out, "{frame}")?;
    out.flush()
}
