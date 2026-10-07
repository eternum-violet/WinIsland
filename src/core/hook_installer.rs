use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

pub struct HookInstaller {
    settings_path: PathBuf,
    handler_path: PathBuf,
}

impl HookInstaller {
    pub fn new() -> Result<Self, String> {
        let home_dir = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map_err(|_| "Cannot find home directory")?;

        let settings_path = PathBuf::from(home_dir)
            .join(".claude")
            .join("settings.json");

        // Get absolute path to hook-handler.js
        // Try multiple locations: exe dir, parent of exe dir (for dev builds), and project root
        let exe_path =
            std::env::current_exe().map_err(|e| format!("Cannot get executable path: {}", e))?;

        let exe_dir = exe_path.parent().ok_or("Cannot get executable directory")?;

        // Try: 1. exe_dir/hook-scripts, 2. exe_dir/../../../hook-scripts (for target/release builds)
        let handler_path = if exe_dir
            .join("hook-scripts")
            .join("hook-handler.js")
            .exists()
        {
            exe_dir.join("hook-scripts").join("hook-handler.js")
        } else if exe_dir
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.join("hook-scripts").join("hook-handler.js"))
            .filter(|p| p.exists())
            .is_some()
        {
            exe_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("hook-scripts")
                .join("hook-handler.js")
        } else {
            return Err(
                "Cannot find hook-handler.js. Please ensure hook-scripts directory exists."
                    .to_string(),
            );
        };

        Ok(Self {
            settings_path,
            handler_path,
        })
    }

    pub fn is_installed(&self) -> Result<bool, String> {
        if !self.settings_path.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(&self.settings_path)
            .map_err(|e| format!("Failed to read settings.json: {}", e))?;

        let settings: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse settings.json: {}", e))?;

        if let Some(hooks) = settings.get("hooks") {
            let hooks_str = hooks.to_string();
            Ok(hooks_str.contains("winisland") || hooks_str.contains("hook-handler.js"))
        } else {
            Ok(false)
        }
    }

    pub fn install(&self) -> Result<(), String> {
        // Create backup
        if self.settings_path.exists() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let backup_path = self
                .settings_path
                .with_extension(format!("json.backup_{}", timestamp));
            fs::copy(&self.settings_path, &backup_path)
                .map_err(|e| format!("Failed to create backup: {}", e))?;
            println!("[HookInstaller] Backup created: {}", backup_path.display());
        }

        // Read existing settings or create new
        let mut settings: Value = if self.settings_path.exists() {
            let content = fs::read_to_string(&self.settings_path)
                .map_err(|e| format!("Failed to read settings.json: {}", e))?;
            serde_json::from_str(&content)
                .map_err(|e| format!("Failed to parse settings.json: {}", e))?
        } else {
            json!({})
        };

        // Ensure hooks object exists
        if !settings.get("hooks").is_some() {
            settings["hooks"] = json!({});
        }

        let handler_path_str = self.handler_path.to_string_lossy().replace('\\', "/");

        let hook_command = format!("node \"{}\"", handler_path_str);

        // Short-timeout hook entry for non-blocking events
        let hook_entry_short = json!([{
            "matcher": "",
            "hooks": [{
                "type": "command",
                "command": hook_command,
                "timeout": 5
            }]
        }]);

        // Long-timeout hook entry for PreToolUse (user may take time to respond)
        let hook_entry_pre_tool = json!([{
            "matcher": "",
            "hooks": [{
                "type": "command",
                "command": hook_command,
                "timeout": 120
            }]
        }]);

        // Install hooks for each event type
        let short_events = vec![
            "Stop",
            "Notification",
            "SubagentStop",
            "UserPromptSubmit",
            "PostToolUse",
        ];

        for event in short_events {
            settings["hooks"][event] = hook_entry_short.clone();
        }

        settings["hooks"]["PreToolUse"] = hook_entry_pre_tool;

        // SessionStart with startup matcher
        settings["hooks"]["SessionStart"] = json!([{
            "matcher": "startup",
            "hooks": [{
                "type": "command",
                "command": hook_command,
                "timeout": 5
            }]
        }]);

        // Write back to settings.json
        let settings_str = serde_json::to_string_pretty(&settings)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        // Ensure .claude directory exists
        if let Some(parent) = self.settings_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create .claude directory: {}", e))?;
        }

        fs::write(&self.settings_path, settings_str)
            .map_err(|e| format!("Failed to write settings.json: {}", e))?;

        println!("[HookInstaller] Hooks installed successfully");
        println!("[HookInstaller] Please restart Claude Code for changes to take effect");

        Ok(())
    }

    pub fn uninstall(&self) -> Result<(), String> {
        if !self.settings_path.exists() {
            return Ok(());
        }

        // Create backup
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let backup_path = self
            .settings_path
            .with_extension(format!("json.backup_{}", timestamp));
        fs::copy(&self.settings_path, &backup_path)
            .map_err(|e| format!("Failed to create backup: {}", e))?;

        let content = fs::read_to_string(&self.settings_path)
            .map_err(|e| format!("Failed to read settings.json: {}", e))?;

        let mut settings: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse settings.json: {}", e))?;

        // Remove hooks that contain "winisland" or "hook-handler.js"
        if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            let events = vec![
                "Stop",
                "Notification",
                "SubagentStop",
                "UserPromptSubmit",
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
            ];

            for event in events {
                if let Some(hook_value) = hooks.get(event) {
                    if let Ok(hook_str) = serde_json::to_string(hook_value) {
                        if hook_str.contains("winisland") || hook_str.contains("hook-handler.js") {
                            hooks.remove(event);
                        }
                    }
                }
            }
        }

        let settings_str = serde_json::to_string_pretty(&settings)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        fs::write(&self.settings_path, settings_str)
            .map_err(|e| format!("Failed to write settings.json: {}", e))?;

        println!("[HookInstaller] Hooks uninstalled successfully");

        Ok(())
    }

    pub fn get_handler_path(&self) -> &PathBuf {
        &self.handler_path
    }
}
