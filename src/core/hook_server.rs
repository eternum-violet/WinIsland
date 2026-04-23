use serde::{Deserialize, Serialize};
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
}

#[derive(Debug, Clone)]
pub enum ClaudeState {
    Idle,
    Thinking { project: String, word: String },
    Working { project: String },
    Done { project: String, message: String },
    Attention { project: String, message: String },
}

impl Default for ClaudeState {
    fn default() -> Self {
        Self::Idle
    }
}

pub struct HookServer {
    state_rx: tokio::sync::watch::Receiver<ClaudeState>,
}

impl HookServer {
    pub fn new() -> Self {
        let (state_tx, state_rx) = tokio::sync::watch::channel(ClaudeState::default());

        tokio::spawn(async move {
            run_server(state_tx).await;
        });

        Self { state_rx }
    }

    pub fn get_state(&self) -> ClaudeState {
        self.state_rx.borrow().clone()
    }
}

async fn run_server(state_tx: tokio::sync::watch::Sender<ClaudeState>) {
    let state_tx = warp::any().map(move || state_tx.clone());

    let event_route = warp::post()
        .and(warp::path("event"))
        .and(warp::body::json())
        .and(state_tx)
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

    warp::serve(event_route)
        .run(([127, 0, 0, 1], 19823))
        .await;
}

async fn handle_event(
    event: HookEvent,
    state_tx: tokio::sync::watch::Sender<ClaudeState>,
) -> Result<impl warp::Reply, warp::Rejection> {
    println!("[HookServer] Received event: {:?}", event.event);
    println!("[HookServer] Full event data: {:?}", event);

    let new_state = match event.event.as_str() {
        "UserPromptSubmit" => {
            let project = event.data.project.unwrap_or_else(|| "Unknown".to_string());
            let word = crate::utils::thinking_words::get_random_thinking_word();
            println!("[HookServer] -> Thinking: {} - {}", project, word);
            ClaudeState::Thinking { project, word }
        }
        "PreToolUse" => {
            let project = event.data.project.as_ref().map(|s| s.as_str()).unwrap_or("Unknown");
            let tool_name = event.data.tool_name.as_ref().map(|s| s.as_str()).unwrap_or("Unknown");
            let action = format_tool_action(tool_name, &event.data);
            println!("[HookServer] -> Tool: {} - {}", project, action);
            ClaudeState::Working { project: format!("{} - {}", project, action) }
        }
        "PostToolUse" => {
            // Keep current state, don't change on PostToolUse
            return Ok(warp::reply::json(&serde_json::json!({"status": "ok"})));
        }
        "Stop" => {
            if event.data.stop_hook_active.unwrap_or(false) {
                println!("[HookServer] -> Ignoring Stop (stopHookActive=true)");
                return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
            }
            let project = event.data.project.unwrap_or_else(|| "Unknown".to_string());
            println!("[HookServer] -> Done: {}", project);
            ClaudeState::Done {
                project,
                message: "任务完成".to_string(),
            }
        }
        "SubagentStop" => {
            let project = event.data.project.unwrap_or_else(|| "Unknown".to_string());
            println!("[HookServer] -> Done (subagent): {}", project);
            ClaudeState::Done {
                project,
                message: "子任务完成".to_string(),
            }
        }
        "Notification" => {
            if let Some(notification_type) = &event.data.notification_type {
                match notification_type.as_str() {
                    "permission_prompt" => {
                        let project = event.data.project.unwrap_or_else(|| "Unknown".to_string());
                        let message = event.data.message.unwrap_or_else(|| "需要确认".to_string());
                        println!("[HookServer] -> Attention: {} - {}", project, message);
                        ClaudeState::Attention { project, message }
                    }
                    "idle_prompt" => {
                        let project = event.data.project.unwrap_or_else(|| "Unknown".to_string());
                        println!("[HookServer] -> Done (idle): {}", project);
                        ClaudeState::Done {
                            project,
                            message: "等待输入".to_string(),
                        }
                    }
                    _ => {
                        println!("[HookServer] -> Unknown notification type: {}", notification_type);
                        return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
                    }
                }
            } else {
                println!("[HookServer] -> Notification without type");
                return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
            }
        }
        "SessionStart" => {
            let project = event.data.project.unwrap_or_else(|| "Unknown".to_string());
            println!("[HookServer] -> Session started: {}", project);
            ClaudeState::Idle
        }
        _ => {
            println!("[HookServer] -> Unknown event: {}", event.event);
            return Ok(warp::reply::json(&serde_json::json!({"status": "ignored"})));
        }
    };

    let _ = state_tx.send(new_state);

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
