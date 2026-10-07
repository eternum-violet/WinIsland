use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use warp::Filter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub event: String,
    pub data: HookEventData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventData {
    pub project: Option<String>,
    pub cwd: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub message: Option<String>,
    #[serde(rename = "notificationType")]
    pub notification_type: Option<String>,
    #[serde(rename = "stopHookActive")]
    pub stop_hook_active: Option<bool>,
    pub timestamp: Option<u64>,
    #[serde(rename = "toolName")]
    pub tool_name: Option<String>,
    #[serde(rename = "filePath")]
    pub file_path: Option<String>,
    pub command: Option<String>,
    #[serde(rename = "approvalRequestId")]
    pub approval_request_id: Option<String>,
    #[serde(rename = "approvalTitle")]
    pub approval_title: Option<String>,
    #[serde(rename = "approvalDetail")]
    pub approval_detail: Option<String>,
    #[serde(rename = "approvalToolName")]
    pub approval_tool_name: Option<String>,
    #[serde(rename = "approvalCommand")]
    pub approval_command: Option<String>,
    #[serde(rename = "approvalFilePath")]
    pub approval_file_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalAction {
    Deny,
    ApproveOnce,
    AllowForSession,
}

impl ApprovalAction {
    pub fn as_response_payload(&self) -> serde_json::Value {
        match self {
            Self::Deny => serde_json::json!({
                "approved": false,
                "reason": "User denied via WinIsland",
            }),
            Self::ApproveOnce => serde_json::json!({
                "approved": true,
            }),
            Self::AllowForSession => serde_json::json!({
                "approved": true,
                "behavior": "always",
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub title: String,
    pub detail: String,
    pub tool_name: Option<String>,
    pub command: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeState {
    Idle,
    Thinking {
        project: String,
        word: String,
    },
    Working {
        project: String,
    },
    Done {
        project: String,
        message: String,
    },
    Attention {
        project: String,
        message: String,
    },
    ApprovalRequired {
        project: String,
        request: ApprovalRequest,
    },
}

impl Default for ClaudeState {
    fn default() -> Self {
        Self::Idle
    }
}

impl fmt::Display for ClaudeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Thinking { .. } => write!(f, "Thinking"),
            Self::Working { .. } => write!(f, "Working"),
            Self::Done { .. } => write!(f, "Done"),
            Self::Attention { .. } => write!(f, "Attention"),
            Self::ApprovalRequired { .. } => write!(f, "ApprovalRequired"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeSessionStateMeta {
    pub project: Option<String>,
    pub approval_detail: Option<String>,
    pub approval_tool_name: Option<String>,
    pub approval_command: Option<String>,
    pub approval_file_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeSessionStates {
    pub current: ClaudeState,
    pub per_session: std::collections::HashMap<String, ClaudeState>,
    pub per_session_meta: HashMap<String, ClaudeSessionStateMeta>,
}

pub struct HookServer {
    state_rx: tokio::sync::watch::Receiver<ClaudeSessionStates>,
}

#[derive(Default)]
struct HookLifecycle {
    pending_done_generation: u64,
}

impl HookLifecycle {
    fn cancel_pending_done(&mut self) {
        self.pending_done_generation = self.pending_done_generation.wrapping_add(1);
    }

    fn arm_pending_done(&mut self) -> u64 {
        self.pending_done_generation = self.pending_done_generation.wrapping_add(1);
        self.pending_done_generation
    }

    fn is_pending_done_current(&self, generation: u64) -> bool {
        self.pending_done_generation == generation
    }
}

impl HookServer {
    pub fn new() -> Self {
        let (state_tx, state_rx) = tokio::sync::watch::channel(ClaudeSessionStates {
            current: ClaudeState::default(),
            per_session: std::collections::HashMap::new(),
            per_session_meta: HashMap::new(),
        });

        tokio::spawn(async move {
            run_server(state_tx).await;
        });

        Self { state_rx }
    }

    pub fn get_state(&self) -> ClaudeState {
        self.state_rx.borrow().current.clone()
    }

    pub fn get_session_states(&self) -> ClaudeSessionStates {
        self.state_rx.borrow().clone()
    }
}

async fn run_server(state_tx: tokio::sync::watch::Sender<ClaudeSessionStates>) {
    let lifecycle = Arc::new(Mutex::new(HookLifecycle::default()));
    let state_tx = warp::any().map(move || state_tx.clone());
    let lifecycle_filter = warp::any().map(move || lifecycle.clone());

    let event_route = warp::post()
        .and(warp::path("event"))
        .and(warp::body::json())
        .and(state_tx)
        .and(lifecycle_filter)
        .and_then(handle_event);

    println!("[HookServer] Starting HTTP server on 127.0.0.1:19823");

    match tokio::net::TcpListener::bind("127.0.0.1:19823").await {
        Ok(_) => {
            println!("[HookServer] Port 19823 is available");
        }
        Err(e) => {
            eprintln!("[HookServer] ERROR: Failed to bind port 19823: {}", e);
            eprintln!("[HookServer] Claude Code integration will not work!");
            return;
        }
    }

    warp::serve(event_route).run(([127, 0, 0, 1], 19823)).await;
}

async fn read_latest_assistant_preview() -> Option<String> {
    let home_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let projects_dir = PathBuf::from(home_dir).join(".claude").join("projects");

    let mut best_result: Option<(u64, String)> = None;

    let mut project_entries = tokio::fs::read_dir(&projects_dir).await.ok()?;
    while let Ok(Some(project_entry)) = project_entries.next_entry().await {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let mut entries = tokio::fs::read_dir(&project_dir).await.ok()?;
        let mut latest_path: Option<(u64, PathBuf)> = None;

        while let Ok(Some(entry)) = entries.next_entry().await {
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
            let modified = entry
                .metadata()
                .await
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as u64;
            match &latest_path {
                Some((best_modified, _)) if *best_modified >= modified => {}
                _ => latest_path = Some((modified, path)),
            }
        }

        if let Some((modified, path)) = latest_path {
            let content = tokio::fs::read_to_string(&path).await.ok()?;
            for line in content.lines().rev() {
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let message = msg.get("message").unwrap_or(&msg);
                let Some(content_array) = message.get("content").and_then(|v| v.as_array())
                else {
                    continue;
                };
                for item in content_array {
                    if item.get("type").and_then(|v| v.as_str()) != Some("text") {
                        continue;
                    }
                    let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let normalized = text.replace('\n', " ").trim().to_string();
                    if !normalized.is_empty() {
                        let preview: String = normalized.chars().take(120).collect();
                        if best_result
                            .as_ref()
                            .map(|(best_mod, _)| modified > *best_mod)
                            .unwrap_or(true)
                        {
                            best_result = Some((modified, preview));
                        }
                        break;
                    }
                }
                if best_result
                    .as_ref()
                    .map(|(best_mod, _)| modified >= *best_mod)
                    .unwrap_or(true)
                {
                    break;
                }
            }
        }
    }

    best_result.map(|(_, text)| text)
}

async fn read_latest_assistant_preview_for_session(session_id: &str) -> Option<String> {
    let home_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let projects_dir = PathBuf::from(home_dir).join(".claude").join("projects");

    let mut project_entries = tokio::fs::read_dir(&projects_dir).await.ok()?;
    while let Ok(Some(project_entry)) = project_entries.next_entry().await {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let direct_path = project_dir.join(format!("{}.jsonl", session_id));
        if tokio::fs::metadata(&direct_path).await.is_err() {
            continue;
        }
        let content = tokio::fs::read_to_string(&direct_path).await.ok()?;
        for line in content.lines().rev() {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let message = msg.get("message").unwrap_or(&msg);
            let Some(content_array) = message.get("content").and_then(|v| v.as_array()) else {
                continue;
            };
            for item in content_array {
                if item.get("type").and_then(|v| v.as_str()) != Some("text") {
                    continue;
                }
                let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
                    continue;
                };
                let normalized = text.replace('\n', " ").trim().to_string();
                if !normalized.is_empty() {
                    return Some(normalized.chars().take(120).collect());
                }
            }
        }
    }
    None
}

fn build_approval_request(data: &HookEventData) -> Option<ApprovalRequest> {
    let title = data
        .approval_title
        .clone()
        .or_else(|| data.message.clone())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;

    let detail = data
        .approval_detail
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| title.clone());

    Some(ApprovalRequest {
        request_id: data
            .approval_request_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| title.clone()),
        title,
        detail,
        tool_name: data.approval_tool_name.clone().filter(|s| !s.is_empty()),
        command: data.approval_command.clone().filter(|s| !s.is_empty()),
        file_path: data.approval_file_path.clone().filter(|s| !s.is_empty()),
    })
}

fn sanitize_project_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "-")
        .replace('/', "-")
        .replace(':', "-")
        .replace('_', "-")
}

async fn handle_event(
    event: HookEvent,
    state_tx: tokio::sync::watch::Sender<ClaudeSessionStates>,
    lifecycle: Arc<Mutex<HookLifecycle>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    println!("[HookServer] Received event: {:?}", event.event);
    println!("[HookServer] Full event data: {:?}", event);

    let prev_snapshot = state_tx.borrow().clone();
    let prev_state = prev_snapshot.current.clone();
    let session_id = event
        .data
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let mut lifecycle_guard = lifecycle.lock().await;

    let new_state = match event.event.as_str() {
        "UserPromptSubmit" => {
            lifecycle_guard.cancel_pending_done();
            let project = event
                .data
                .project
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            let word = crate::utils::thinking_words::get_random_thinking_word();
            println!("[HookServer] -> Thinking: {} - {}", project, word);
            ClaudeState::Thinking { project, word }
        }
        "PreToolUse" => {
            lifecycle_guard.cancel_pending_done();
            let project = event
                .data
                .project
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or("Unknown");
            let tool_name = event
                .data
                .tool_name
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or("Unknown");
            let action = format_tool_action(tool_name, &event.data);
            println!("[HookServer] -> Tool: {} - {}", project, action);
            ClaudeState::Working {
                project: format!("{} - {}", project, action),
            }
        }
        "PostToolUse" => {
            lifecycle_guard.cancel_pending_done();
            let project = event
                .data
                .project
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            let word = crate::utils::thinking_words::get_random_thinking_word();
            println!(
                "[HookServer] -> Thinking after tool: {} - {}",
                project, word
            );
            ClaudeState::Thinking { project, word }
        }
        "Stop" => {
            if event.data.stop_hook_active.unwrap_or(false) {
                println!("[HookServer] -> Ignoring Stop (stopHookActive=true)");
                return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
            }
            let result_text = if let Some(session_id) = event
                .data
                .session_id
                .as_deref()
                .filter(|s| !s.is_empty() && *s != "unknown")
            {
                if let Some(text) = read_latest_assistant_preview_for_session(session_id).await {
                    Some(text)
                } else {
                    read_latest_assistant_preview().await
                }
            } else {
                read_latest_assistant_preview().await
            };
            if let Some(result_text) = result_text {
                println!("[HookServer] -> Scheduling delayed final result");
                let delayed_state = ClaudeState::Done {
                    project: "Claude".to_string(),
                    message: result_text,
                };
                let delayed_tx = state_tx.clone();
                let delayed_session_id = event.data.session_id.clone();
                let generation = lifecycle_guard.arm_pending_done();
                let delayed_lifecycle = lifecycle.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    let lifecycle_guard = delayed_lifecycle.lock().await;
                    if lifecycle_guard.is_pending_done_current(generation) {
                        let mut snapshot = delayed_tx.borrow().clone();
                        snapshot.current = delayed_state.clone();
                        if let Some(session_id) = delayed_session_id
                            .clone()
                            .filter(|s| !s.is_empty() && s != "unknown")
                        {
                            snapshot
                                .per_session
                                .insert(session_id, delayed_state.clone());
                        }
                        let _ = delayed_tx.send(snapshot);
                    }
                });
            } else {
                println!("[HookServer] -> Preserving current state on Stop");
            }
            prev_state.clone()
        }
        "SubagentStop" => {
            let result_text = if let Some(session_id) = event
                .data
                .session_id
                .as_deref()
                .filter(|s| !s.is_empty() && *s != "unknown")
            {
                if let Some(text) = read_latest_assistant_preview_for_session(session_id).await {
                    Some(text)
                } else {
                    read_latest_assistant_preview().await
                }
            } else {
                read_latest_assistant_preview().await
            };
            if let Some(result_text) = result_text {
                println!("[HookServer] -> Scheduling delayed final result (subagent)");
                let delayed_state = ClaudeState::Done {
                    project: "Claude".to_string(),
                    message: result_text,
                };
                let delayed_tx = state_tx.clone();
                let delayed_session_id = event.data.session_id.clone();
                let generation = lifecycle_guard.arm_pending_done();
                let delayed_lifecycle = lifecycle.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    let lifecycle_guard = delayed_lifecycle.lock().await;
                    if lifecycle_guard.is_pending_done_current(generation) {
                        let mut snapshot = delayed_tx.borrow().clone();
                        snapshot.current = delayed_state.clone();
                        if let Some(session_id) = delayed_session_id
                            .clone()
                            .filter(|s| !s.is_empty() && s != "unknown")
                        {
                            snapshot
                                .per_session
                                .insert(session_id, delayed_state.clone());
                        }
                        let _ = delayed_tx.send(snapshot);
                    }
                });
            } else {
                println!("[HookServer] -> Preserving current state on SubagentStop");
            }
            prev_state.clone()
        }
        "Notification" => {
            if let Some(notification_type) = &event.data.notification_type {
                match notification_type.as_str() {
                    "permission_prompt" => {
                        let project = event
                            .data
                            .project
                            .clone()
                            .unwrap_or_else(|| "Unknown".to_string());
                        if let Some(request) = build_approval_request(&event.data) {
                            println!("[HookServer] -> Approval: {} - {}", project, request.title);
                            ClaudeState::ApprovalRequired { project, request }
                        } else {
                            let message =
                                event.data.message.unwrap_or_else(|| "需要确认".to_string());
                            println!("[HookServer] -> Attention: {} - {}", project, message);
                            ClaudeState::Attention { project, message }
                        }
                    }
                    "idle_prompt" => {
                        println!("[HookServer] -> Ignoring idle_prompt for state transitions");
                        prev_state.clone()
                    }
                    _ => {
                        println!(
                            "[HookServer] -> Unknown notification type: {}",
                            notification_type
                        );
                        return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
                    }
                }
            } else {
                println!("[HookServer] -> Notification without type");
                return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
            }
        }
        "SessionStart" => {
            lifecycle_guard.cancel_pending_done();
            let project = event
                .data
                .project
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            println!("[HookServer] -> Session started: {}", project);
            ClaudeState::Idle
        }
        _ => {
            println!("[HookServer] -> Unknown event: {}", event.event);
            return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
        }
    };

    drop(lifecycle_guard);

    let mut next_snapshot = prev_snapshot.clone();
    next_snapshot.current = new_state.clone();
    if let Some(session_id) = event
        .data
        .session_id
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown")
    {
        match event.event.as_str() {
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "Stop" | "SubagentStop"
            | "Notification" => {
                next_snapshot
                    .per_session
                    .insert(session_id.clone(), new_state.clone());
                let meta = next_snapshot
                    .per_session_meta
                    .entry(session_id.clone())
                    .or_default();
                meta.project = event.data.project.clone();
                if event.event == "Notification"
                    && event.data.notification_type.as_deref() == Some("permission_prompt")
                {
                    meta.approval_detail = event.data.approval_detail.clone();
                    meta.approval_tool_name = event.data.approval_tool_name.clone();
                    meta.approval_command = event.data.approval_command.clone();
                    meta.approval_file_path = event.data.approval_file_path.clone();
                }
            }
            "SessionStart" => {
                next_snapshot.per_session.remove(&session_id);
                next_snapshot.per_session_meta.remove(&session_id);
            }
            _ => {}
        }
    }

    println!(
        "CLAUDE_HOOK event={} session={} prev={} next={}",
        event.event, session_id, prev_state, new_state
    );
    if prev_state.to_string() != new_state.to_string()
        || prev_snapshot.per_session != next_snapshot.per_session
    {
        let _ = state_tx.send(next_snapshot);
    }

    Ok(warp::reply::json(&serde_json::json!({"status": "ok"})))
}

fn format_tool_action(tool_name: &str, data: &HookEventData) -> String {
    match tool_name {
        "Read" => {
            if let Some(file) = data.file_path.as_ref() {
                format!("读取 {}", extract_filename(file))
            } else {
                "读取文件".to_string()
            }
        }
        "Write" => {
            if let Some(file) = data.file_path.as_ref() {
                format!("写入 {}", extract_filename(file))
            } else {
                "写入文件".to_string()
            }
        }
        "Edit" => {
            if let Some(file) = data.file_path.as_ref() {
                format!("编辑 {}", extract_filename(file))
            } else {
                "编辑文件".to_string()
            }
        }
        "Bash" => {
            if let Some(cmd) = data.command.as_ref() {
                let short_cmd = if cmd.len() > 30 {
                    format!("{}...", &cmd[..30])
                } else {
                    cmd.clone()
                };
                format!("执行 {}", short_cmd)
            } else {
                "执行命令".to_string()
            }
        }
        "Grep" => "搜索代码".to_string(),
        "Glob" => "查找文件".to_string(),
        "WebSearch" => "网络搜索".to_string(),
        "WebFetch" => "获取网页".to_string(),
        _ => format!("{}", tool_name),
    }
}

fn extract_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}
