use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ClaudeActionKind {
    #[default]
    PermissionApproval,
    ChoicePrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaudeChoiceOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeApprovalWait {
    pub waiting_for: String,
    pub status: String,
    pub pid: u32,
    pub detail_text: String,
    pub detail_title: String,
    pub action_kind: ClaudeActionKind,
    pub choice_options: Vec<ClaudeChoiceOption>,
}

#[derive(Debug, Clone)]
struct ActiveClaudeSession {
    pid: u32,
    session_id: String,
    started_at: u64,
    cwd: PathBuf,
    display_name: String,
    approval_wait: Option<ClaudeApprovalWait>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeSessionSummary {
    pub session_id: String,
    pub project: String,
    pub preview_text: String,
    pub full_text: String,
    pub last_active_at: u64,
    pub approval_wait: Option<ClaudeApprovalWait>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeSessionsSnapshot {
    pub sessions: Vec<ClaudeSessionSummary>,
}

#[derive(Debug, Clone)]
struct TranscriptOutput {
    preview_text: String,
    full_text: String,
    last_active_at: u64,
}

pub struct ClaudeWatcher {
    snapshot_rx: tokio::sync::watch::Receiver<ClaudeSessionsSnapshot>,
    _cancel_token: tokio_util::sync::CancellationToken,
}

impl ClaudeWatcher {
    pub fn new() -> Self {
        let (snapshot_tx, snapshot_rx) =
            tokio::sync::watch::channel(ClaudeSessionsSnapshot::default());
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let token = cancel_token.clone();

        tokio::spawn(async move {
            monitor_claude_output(snapshot_tx, token).await;
        });

        Self {
            snapshot_rx,
            _cancel_token: cancel_token,
        }
    }

    pub fn get_snapshot(&self) -> ClaudeSessionsSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    pub fn find_session_pid(&self, session_id: &str) -> Option<u32> {
        let home_dir = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .ok()?;
        let sessions_dir = PathBuf::from(home_dir).join(".claude").join("sessions");
        let entries = std::fs::read_dir(sessions_dir).ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(session) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            let Some(found_session_id) = session.get("sessionId").and_then(|v| v.as_str()) else {
                continue;
            };
            if found_session_id != session_id {
                continue;
            }
            let Some(pid) = session.get("pid").and_then(|v| v.as_u64()) else {
                continue;
            };
            return Some(pid as u32);
        }

        None
    }
}

async fn monitor_claude_output(
    tx: tokio::sync::watch::Sender<ClaudeSessionsSnapshot>,
    cancel_token: tokio_util::sync::CancellationToken,
) {
    let home_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok();
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

                let mut summaries = Vec::new();
                for session in sessions {
                    println!("[ClaudeWatcher] Checking session pid={} cwd={}", session.pid, session.cwd.display());
                    if let Some(summary) = collect_session_summary(&projects_dir, &session).await {
                        let preview: String = summary.preview_text.chars().take(50).collect();
                        println!("[ClaudeWatcher] Got latest text: {}", preview);
                        summaries.push(summary);
                    } else {
                        println!("[ClaudeWatcher] No transcript found for session");
                    }
                }

                summaries.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
                let _ = tx.send(ClaudeSessionsSnapshot { sessions: summaries });
            }
        }
    }
}

async fn collect_session_summary(
    projects_dir: &PathBuf,
    session: &ActiveClaudeSession,
) -> Option<ClaudeSessionSummary> {
    let transcript_path = find_session_transcript(projects_dir, session).await?;
    println!(
        "[ClaudeWatcher] Found transcript: {}",
        transcript_path.display()
    );
    let mut transcript = read_latest_assistant_output(&transcript_path).await?;

    if let Some(approval_wait) = session.approval_wait.as_ref() {
        if let Some(enriched) =
            enrich_approval_wait_from_transcript(&transcript_path, approval_wait).await
        {
            transcript.preview_text = enriched.detail_title.trim().chars().take(120).collect();
            return Some(ClaudeSessionSummary {
                session_id: session.session_id.clone(),
                project: session.display_name.clone(),
                preview_text: transcript.preview_text,
                full_text: transcript.full_text,
                last_active_at: transcript.last_active_at.max(session.started_at),
                approval_wait: Some(enriched),
            });
        }
    }

    Some(ClaudeSessionSummary {
        session_id: session.session_id.clone(),
        project: session.display_name.clone(),
        preview_text: transcript.preview_text,
        full_text: transcript.full_text,
        last_active_at: transcript.last_active_at.max(session.started_at),
        approval_wait: session.approval_wait.clone(),
    })
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

        let Some(pid) = session
            .get("pid")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
        else {
            continue;
        };
        let Some(session_id) = session.get("sessionId").and_then(|v| v.as_str()) else {
            continue;
        };
        let started_at = session
            .get("startedAt")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cwd = session
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_default();
        let approval_wait = extract_approval_wait(&session, pid);

        let display_name = session
            .get("displayName")
            .or_else(|| session.get("name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| short_session_label(session_id));

        if !is_process_alive(pid).await {
            continue;
        }

        if session_id.is_empty() || cwd.as_os_str().is_empty() {
            continue;
        }

        sessions.push(ActiveClaudeSession {
            pid,
            session_id: session_id.to_string(),
            started_at,
            cwd,
            display_name,
            approval_wait,
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

async fn find_session_transcript(
    projects_dir: &PathBuf,
    session: &ActiveClaudeSession,
) -> Option<PathBuf> {
    let project_dir = projects_dir.join(sanitize_project_path(&session.cwd));
    let direct_path = project_dir.join(format!("{}.jsonl", session.session_id));
    if tokio::fs::metadata(&direct_path).await.is_ok() {
        return Some(direct_path);
    }

    find_best_transcript(projects_dir, &session.cwd, session.started_at).await
}

async fn find_best_transcript(
    projects_dir: &PathBuf,
    cwd: &Path,
    started_at: u64,
) -> Option<PathBuf> {
    let project_dir = projects_dir.join(sanitize_project_path(cwd));
    let mut entries = tokio::fs::read_dir(&project_dir).await.ok()?;
    let mut best_match: Option<(u64, PathBuf)> = None;

    while let Some(entry) = entries.next_entry().await.ok()? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        if path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("subagents")
        {
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

fn extract_approval_wait(session: &serde_json::Value, pid: u32) -> Option<ClaudeApprovalWait> {
    let status = session.get("status")?.as_str()?.trim().to_string();
    let waiting_for = session.get("waitingFor")?.as_str()?.trim().to_string();
    if status != "waiting" || waiting_for.is_empty() {
        return None;
    }
    Some(ClaudeApprovalWait {
        detail_title: waiting_for.clone(),
        detail_text: waiting_for.clone(),
        waiting_for,
        status,
        pid,
        action_kind: ClaudeActionKind::PermissionApproval,
        choice_options: Vec::new(),
    })
}

fn short_session_label(session_id: &str) -> String {
    let short = session_id.chars().take(8).collect::<String>();
    format!("Session {}", short)
}

fn sanitize_project_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "-")
        .replace('/', "-")
        .replace(':', "-")
        .replace('_', "-")
}

async fn enrich_approval_wait_from_transcript(
    jsonl_path: &PathBuf,
    approval_wait: &ClaudeApprovalWait,
) -> Option<ClaudeApprovalWait> {
    let content = tokio::fs::read_to_string(jsonl_path).await.ok()?;
    enrich_approval_wait_from_content(&content, approval_wait)
}

fn enrich_approval_wait_from_content(
    content: &str,
    approval_wait: &ClaudeApprovalWait,
) -> Option<ClaudeApprovalWait> {
    let lines = content.lines().collect::<Vec<_>>();

    for (index, line) in lines.iter().enumerate().rev() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if let Some(enriched) = extract_hook_approval_detail(&msg, approval_wait) {
            return Some(enriched);
        }

        if let Some(enriched) = extract_ask_user_question_detail(&lines, index, &msg, approval_wait)
        {
            return Some(enriched);
        }
    }

    None
}

fn extract_hook_approval_detail(
    msg: &serde_json::Value,
    approval_wait: &ClaudeApprovalWait,
) -> Option<ClaudeApprovalWait> {
    if msg.get("type").and_then(|v| v.as_str()) != Some("attachment") {
        return None;
    }
    let attachment = msg.get("attachment")?;
    if attachment.get("type").and_then(|v| v.as_str()) != Some("hook_success") {
        return None;
    }
    let hook_name = attachment
        .get("hookName")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !hook_name.contains("PreToolUse") {
        return None;
    }

    let command = attachment
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let stderr = attachment
        .get("stderr")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let stdout = attachment
        .get("stdout")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    let detail_title = if !command.is_empty() {
        command.to_string()
    } else {
        approval_wait.waiting_for.clone()
    };

    let mut detail_parts = Vec::new();
    if !command.is_empty() {
        detail_parts.push(command.to_string());
    }
    if !stderr.is_empty() {
        detail_parts.push(stderr.to_string());
    }
    if !stdout.is_empty() {
        detail_parts.push(stdout.to_string());
    }

    if detail_parts.is_empty() {
        return None;
    }

    Some(ClaudeApprovalWait {
        waiting_for: approval_wait.waiting_for.clone(),
        status: approval_wait.status.clone(),
        pid: approval_wait.pid,
        detail_title,
        detail_text: detail_parts.join("\n\n"),
        action_kind: ClaudeActionKind::PermissionApproval,
        choice_options: Vec::new(),
    })
}

fn extract_ask_user_question_detail(
    lines: &[&str],
    index: usize,
    msg: &serde_json::Value,
    approval_wait: &ClaudeApprovalWait,
) -> Option<ClaudeApprovalWait> {
    let message = msg.get("message")?;
    let content = message.get("content")?.as_array()?;
    let tool_use = content.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("tool_use")
            && item.get("name").and_then(|v| v.as_str()) == Some("AskUserQuestion")
    })?;

    let questions = tool_use.get("input")?.get("questions")?.as_array()?;
    let question_texts = questions
        .iter()
        .filter_map(|question| question.get("question").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if question_texts.is_empty() {
        return None;
    }

    let tool_use_id = tool_use.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let answered_same_question = if index + 1 < lines.len() {
        let next_msg = match serde_json::from_str::<serde_json::Value>(lines[index + 1]) {
            Ok(v) => v,
            Err(_) => {
                return None;
            }
        };
        let user_content = match next_msg
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_array())
        {
            Some(arr) => arr,
            None => &Vec::new(),
        };
        user_content.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("tool_result")
                && item.get("tool_use_id").and_then(|v| v.as_str()) == Some(tool_use_id)
        })
    } else {
        false
    };
    if answered_same_question {
        return None;
    }

    let detail_title = question_texts[0].clone();
    let detail_text = question_texts.join("\n\n");
    let choice_options = questions
        .iter()
        .filter_map(|question| question.get("options").and_then(|v| v.as_array()))
        .flat_map(|options| options.iter())
        .filter_map(|option| {
            let label = option.get("label").and_then(|v| v.as_str())?.trim();
            if label.is_empty() {
                return None;
            }
            let value = option
                .get("value")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or(label);
            Some(ClaudeChoiceOption {
                label: label.to_string(),
                value: value.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if choice_options.is_empty() {
        return None;
    }

    Some(ClaudeApprovalWait {
        waiting_for: approval_wait.waiting_for.clone(),
        status: approval_wait.status.clone(),
        pid: approval_wait.pid,
        detail_title,
        detail_text,
        action_kind: ClaudeActionKind::ChoicePrompt,
        choice_options,
    })
}

async fn read_latest_assistant_output(jsonl_path: &PathBuf) -> Option<TranscriptOutput> {
    let content = tokio::fs::read_to_string(jsonl_path).await.ok()?;

    for line in content.lines().rev() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let message = msg.get("message").unwrap_or(&msg);

        let Some(content_array) = message.get("content").and_then(|v| v.as_array()) else {
            continue;
        };

        let mut full_text_parts = Vec::new();
        for item in content_array {
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        full_text_parts.push(trimmed.to_string());
                    }
                }
            }
        }

        if full_text_parts.is_empty() {
            continue;
        }

        let full_text = full_text_parts.join("\n\n");
        let preview_text = full_text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();

        let last_active_at = msg.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);

        return Some(TranscriptOutput {
            preview_text,
            full_text,
            last_active_at,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeActionKind, ClaudeApprovalWait, ClaudeChoiceOption, enrich_approval_wait_from_content,
    };

    #[test]
    fn skips_answered_choice_prompt_from_ask_user_question_tool_use() {
        let transcript = r#"{"message":{"content":[{"type":"tool_use","id":"call_choice","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach should we use?","options":[{"label":"Inline"},{"label":"Subagent"},{"label":"Skip"}]}]}}]}}
{"message":{"content":[{"type":"tool_result","tool_use_id":"call_choice","content":"User answered"}]}}"#;
        let approval_wait = ClaudeApprovalWait {
            waiting_for: "approve AskUserQuestion".to_string(),
            status: "waiting".to_string(),
            pid: 42,
            detail_text: "approve AskUserQuestion".to_string(),
            detail_title: "approve AskUserQuestion".to_string(),
            action_kind: ClaudeActionKind::PermissionApproval,
            choice_options: Vec::new(),
        };

        let result = enrich_approval_wait_from_content(transcript, &approval_wait);
        assert!(result.is_none(), "Answered AskUserQuestion should be skipped");
    }

    #[test]
    fn extracts_pending_choice_prompt_at_end_of_transcript() {
        let transcript = r#"{"message":{"content":[{"type":"tool_use","id":"call_choice","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach should we use?","options":[{"label":"Inline"},{"label":"Subagent"},{"label":"Skip"}]}]}}]}}"#;
        let approval_wait = ClaudeApprovalWait {
            waiting_for: "approve AskUserQuestion".to_string(),
            status: "waiting".to_string(),
            pid: 42,
            detail_text: "approve AskUserQuestion".to_string(),
            detail_title: "approve AskUserQuestion".to_string(),
            action_kind: ClaudeActionKind::PermissionApproval,
            choice_options: Vec::new(),
        };

        let enriched = enrich_approval_wait_from_content(transcript, &approval_wait).unwrap();

        assert_eq!(enriched.detail_title, "Which approach should we use?");
        assert_eq!(enriched.detail_text, "Which approach should we use?");
        assert_eq!(
            enriched.choice_options,
            vec![
                ClaudeChoiceOption {
                    label: "Inline".to_string(),
                    value: "Inline".to_string()
                },
                ClaudeChoiceOption {
                    label: "Subagent".to_string(),
                    value: "Subagent".to_string()
                },
                ClaudeChoiceOption {
                    label: "Skip".to_string(),
                    value: "Skip".to_string()
                },
            ]
        );
        assert_eq!(enriched.action_kind, ClaudeActionKind::ChoicePrompt);
    }

    #[test]
    fn extracts_pending_choice_prompt_even_when_waiting_for_text_differs() {
        let transcript = r#"{"message":{"content":[{"type":"tool_use","id":"call_choice","name":"AskUserQuestion","input":{"questions":[{"question":"请选择一个选项来测试审批功能？","options":[{"label":"批准"},{"label":"拒绝"},{"label":"稍后决定"},{"label":"Type something."},{"label":"Chat about this"}]}]}}]}}"#;
        let approval_wait = ClaudeApprovalWait {
            waiting_for: "请选择一个选项来测试审批功能".to_string(),
            status: "waiting".to_string(),
            pid: 42,
            detail_text: "请选择一个选项来测试审批功能".to_string(),
            detail_title: "请选择一个选项来测试审批功能".to_string(),
            action_kind: ClaudeActionKind::PermissionApproval,
            choice_options: Vec::new(),
        };

        let enriched = enrich_approval_wait_from_content(transcript, &approval_wait).unwrap();

        assert_eq!(enriched.detail_title, "请选择一个选项来测试审批功能？");
        assert_eq!(enriched.action_kind, ClaudeActionKind::ChoicePrompt);
        assert_eq!(enriched.choice_options.len(), 5);
        assert_eq!(enriched.choice_options[0].label, "批准");
        assert_eq!(enriched.choice_options[4].label, "Chat about this");
    }
}
