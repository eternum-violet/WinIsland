use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{OpenProcess, GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION};

#[derive(Debug, Clone)]
struct ActiveClaudeSession {
    pid: u32,
    started_at: u64,
    cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ClaudeStatus {
    pub active: bool,
    pub current_text: String,
    pub debug_source: String,
}

impl Default for ClaudeStatus {
    fn default() -> Self {
        Self {
            active: false,
            current_text: String::new(),
            debug_source: String::new(),
        }
    }
}

pub struct ClaudeWatcher {
    status_rx: tokio::sync::watch::Receiver<ClaudeStatus>,
    _cancel_token: tokio_util::sync::CancellationToken,
}

impl ClaudeWatcher {
    pub fn new() -> Self {
        let (status_tx, status_rx) = tokio::sync::watch::channel(ClaudeStatus::default());
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let token = cancel_token.clone();

        // Spawn background task to monitor Claude Code jsonl for status
        tokio::spawn(async move {
            monitor_claude_output(status_tx, token).await;
        });

        Self {
            status_rx,
            _cancel_token: cancel_token,
        }
    }

    pub fn get_status(&self) -> ClaudeStatus {
        self.status_rx.borrow().clone()
    }
}

async fn monitor_claude_output(
    tx: tokio::sync::watch::Sender<ClaudeStatus>,
    cancel_token: tokio_util::sync::CancellationToken,
) {
    let home_dir = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok();
    if home_dir.is_none() {
        return;
    }

    let claude_dir = PathBuf::from(home_dir.unwrap()).join(".claude");
    let sessions_dir = claude_dir.join("sessions");
    let projects_dir = claude_dir.join("projects");

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                break;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(200)) => {
                let sessions = find_active_sessions(&sessions_dir).await;
                println!("[ClaudeWatcher] Found {} active sessions", sessions.len());

                let mut best_status: Option<ClaudeStatus> = None;
                for session in sessions {
                    println!("[ClaudeWatcher] Checking session pid={} cwd={}", session.pid, session.cwd.display());
                    if let Some(transcript_path) = find_best_transcript(&projects_dir, &session.cwd, session.started_at).await {
                        println!("[ClaudeWatcher] Found transcript: {}", transcript_path.display());
                        if let Some(latest_text) = read_latest_assistant_message(&transcript_path).await {
                            let preview: String = latest_text.chars().take(50).collect();
                            println!("[ClaudeWatcher] Got latest text: {}", preview);
                            let status = ClaudeStatus {
                                active: true,
                                current_text: latest_text,
                                debug_source: format!("pid={} cwd={} transcript={}", session.pid, session.cwd.display(), transcript_path.display()),
                            };
                            best_status = Some(status);
                            break;
                        } else {
                            println!("[ClaudeWatcher] No assistant message found in transcript");
                        }
                    } else {
                        println!("[ClaudeWatcher] No transcript found for session");
                    }
                }

                let _ = tx.send(best_status.unwrap_or_default());
            }
        }
    }
}

async fn find_active_sessions(sessions_dir: &PathBuf) -> Vec<ActiveClaudeSession> {
    let mut entries = match tokio::fs::read_dir(sessions_dir).await {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let Ok(session) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };

        let Some(pid) = session.get("pid").and_then(|v| v.as_u64()).map(|v| v as u32) else {
            continue;
        };
        let Some(session_id) = session.get("sessionId").and_then(|v| v.as_str()) else {
            continue;
        };
        let started_at = session.get("startedAt").and_then(|v| v.as_u64()).unwrap_or(0);
        let cwd = session
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_default();

        if !is_process_alive(pid).await {
            continue;
        }

        if session_id.is_empty() || cwd.as_os_str().is_empty() {
            continue;
        }

        sessions.push(ActiveClaudeSession {
            pid,
            started_at,
            cwd,
        });
    }

    sessions.sort_by(|a, b| b.pid.cmp(&a.pid));
    sessions
}

async fn is_process_alive(pid: u32) -> bool {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        let Ok(handle) = handle else {
            return false;
        };

        if handle == HANDLE::default() {
            return false;
        }

        let mut exit_code = 0u32;
        let result = GetExitCodeProcess(handle, &mut exit_code);
        let _ = CloseHandle(handle);

        result.is_ok() && exit_code == 259
    }
}

async fn find_best_transcript(projects_dir: &PathBuf, cwd: &Path, started_at: u64) -> Option<PathBuf> {
    let project_dir = projects_dir.join(sanitize_project_path(cwd));
    let mut entries = tokio::fs::read_dir(&project_dir).await.ok()?;
    let mut best_match: Option<(u64, PathBuf)> = None;

    while let Some(entry) = entries.next_entry().await.ok()? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        if path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("subagents") {
            continue;
        }

        let metadata = entry.metadata().await.ok();
        let modified_score = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let started_score = started_at;
        let score = modified_score.max(started_score);

        match &best_match {
            Some((best_score, _)) if *best_score >= score => {}
            _ => best_match = Some((score, path)),
        }
    }

    best_match.map(|(_, path)| path)
}

fn sanitize_project_path(path: &Path) -> String {
    // Match Claude Code's path sanitization format
    path.to_string_lossy()
        .replace('\\', "-")
        .replace('/', "-")
        .replace(':', "-")
}

async fn read_latest_assistant_message(jsonl_path: &PathBuf) -> Option<String> {
    let content = tokio::fs::read_to_string(jsonl_path).await.ok()?;
    let lines: Vec<&str> = content.lines().collect();

    for line in lines.iter().rev() {
        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
            let message = msg.get("message").unwrap_or(&msg);

            if let Some(content_array) = message.get("content").and_then(|v| v.as_array()) {
                for item in content_array {
                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                            let normalized = text.replace('\n', " ").trim().to_string();
                            if !normalized.is_empty() {
                                return Some(normalized.chars().take(200).collect());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}
