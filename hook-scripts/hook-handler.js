#!/usr/bin/env node

/**
 * Claude Code Hook Handler
 *
 * This script is called by Claude Code hooks system.
 * It reads JSON from stdin and forwards events to WinIsland via HTTP POST.
 *
 * Requirements:
 * - Zero external dependencies (uses only Node.js built-in modules)
 * - Fast execution (< 200ms)
 * - Silent failure if WinIsland is not running
 */

const fs = require('fs');
const path = require('path');
const http = require('http');

const PORT = 19823;
const HOST = '127.0.0.1';
const TIMEOUT = 150;
const DEBUG_ENABLED = process.env.WINISLAND_HOOK_DEBUG === '1';
const LOG_PATH = path.join(__dirname, 'hook-debug.log');
const RAW_PREVIEW_LIMIT = 240;

function writeDebugLog(entry) {
  if (!DEBUG_ENABLED) return;

  const record = {
    timestamp: new Date().toISOString(),
    ...entry,
  };

  try {
    fs.appendFileSync(LOG_PATH, `${JSON.stringify(record)}\n`, 'utf8');
  } catch (_) {
    // Keep hook execution silent even if debug logging fails.
  }
}

function buildPreview(value) {
  if (!value) return '';
  return value.slice(0, RAW_PREVIEW_LIMIT);
}

function normalizeHookData(raw) {
  const safeRaw = raw && typeof raw === 'object' ? raw : {};
  const approval = safeRaw.approval ?? safeRaw.permission ?? safeRaw.permission_request ?? null;

  return {
    event: safeRaw.hook_event_name ?? safeRaw.event ?? 'Unknown',
    cwd: safeRaw.cwd ?? null,
    sessionId: safeRaw.session_id ?? safeRaw.sessionId ?? null,
    message: safeRaw.message ?? null,
    notificationType: safeRaw.notification_type ?? safeRaw.notificationType ?? null,
    stopHookActive: safeRaw.stop_hook_active ?? safeRaw.stopHookActive ?? false,
    toolName: safeRaw.tool_name ?? safeRaw.toolName ?? null,
    toolInput: safeRaw.tool_input ?? safeRaw.toolInput ?? null,
    transcriptPath: safeRaw.transcript_path ?? safeRaw.transcriptPath ?? null,
    approvalRequestId: approval?.id ?? approval?.request_id ?? safeRaw.request_id ?? safeRaw.approval_request_id ?? null,
    approvalTitle: approval?.title ?? approval?.summary ?? safeRaw.title ?? null,
    approvalDetail: approval?.detail ?? approval?.message ?? safeRaw.detail ?? null,
    approvalToolName: approval?.tool_name ?? approval?.toolName ?? null,
    approvalCommand: approval?.command ?? null,
    approvalFilePath: approval?.file_path ?? approval?.filePath ?? null,
  };
}
let inputData = '';

process.stdin.setEncoding('utf8');

process.stdin.on('data', (chunk) => {
  inputData += chunk;
});

process.stdin.on('end', () => {
  writeDebugLog({
    phase: 'stdin_received',
    stdinEmpty: inputData.length === 0,
    stdinBytes: Buffer.byteLength(inputData, 'utf8'),
    rawPreview: buildPreview(inputData),
  });

  try {
    const rawHookData = JSON.parse(inputData);
    const hookData = normalizeHookData(rawHookData);

    writeDebugLog({
      phase: 'parse_ok',
      event: hookData.event,
      cwd: hookData.cwd,
      sessionId: hookData.sessionId,
      toolName: hookData.toolName,
      hasToolInput: Boolean(hookData.toolInput),
    });

    // Extract tool information for PreToolUse/PostToolUse events
    let toolName = null;
    let filePath = null;
    let command = null;

    if (hookData.event === 'PreToolUse' || hookData.event === 'PostToolUse') {
      toolName = hookData.toolName;

      if (hookData.toolInput) {
        if (hookData.toolInput.file_path) {
          filePath = hookData.toolInput.file_path;
        }
        if (hookData.toolInput.command) {
          command = hookData.toolInput.command;
        }
      }
    }

    // Extract relevant fields from Claude Code hook event
    const event = {
      type: 'claude-event',
      event: hookData.event,
      data: {
        project: extractProjectName(hookData.cwd),
        cwd: hookData.cwd,
        sessionId: hookData.sessionId,
        message: hookData.message,
        notificationType: hookData.notificationType,
        stopHookActive: hookData.stopHookActive,
        timestamp: Date.now(),
        toolName: toolName,
        filePath: filePath,
        command: command,
        approvalRequestId: hookData.approvalRequestId,
        approvalTitle: hookData.approvalTitle,
        approvalDetail: hookData.approvalDetail,
        approvalToolName: hookData.approvalToolName,
        approvalCommand: hookData.approvalCommand,
        approvalFilePath: hookData.approvalFilePath
      }
    };

    // For PreToolUse: auto-approve safe tools, otherwise wait for user response
    if (hookData.event === 'PreToolUse' && toolName && !SAFE_TOOLS.has(toolName)) {
      const sessionId = hookData.sessionId;
      if (!sessionId) {
        // No session ID, cannot correlate response — auto-approve
        writeDebugLog({ phase: 'pre_tool_use_no_session', toolName });
        process.exit(0);
      }

      sendEvent(event, {
        eventName: event.event,
        sessionId: event.data.sessionId,
        toolName: event.data.toolName,
      }, function afterSent() {
        // Now poll for the user's response
        pollForResponse(sessionId, function(response) {
          if (!response) {
            writeDebugLog({ phase: 'pre_tool_use_timeout', toolName, sessionId });
            // Timeout — auto-approve to avoid blocking
            process.exit(0);
          }

          writeDebugLog({
            phase: 'pre_tool_use_response',
            toolName,
            sessionId,
            approved: response.approved !== false,
          });

          if (response.approved === false) {
            // Deny: output JSON to stdout and exit with code 2
            const denyPayload = JSON.stringify({
              decision: 'deny',
              reason: response.reason || 'User denied via WinIsland',
            });
            process.stdout.write(denyPayload);
            process.exit(2);
          } else if (response.behavior === 'always') {
            // Allow for session: output JSON to stdout and exit with code 0
            const allowPayload = JSON.stringify({
              decision: 'allow',
              behavior: 'always',
            });
            process.stdout.write(allowPayload);
            process.exit(0);
          } else {
            // Approve once: exit with code 0, no output
            process.exit(0);
          }
        });
      });
      return; // Don't fall through — async path
    }

    sendEvent(event, {
      eventName: event.event,
      sessionId: event.data.sessionId,
      toolName: event.data.toolName,
    });
  } catch (err) {
    writeDebugLog({
      phase: 'parse_failed',
      error: err instanceof Error ? err.message : String(err),
      rawPreview: buildPreview(inputData),
    });
    process.exit(0);
  }
});

function extractProjectName(cwd) {
  if (!cwd) return 'Unknown';

  // Extract last directory name from path
  const parts = cwd.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || 'Unknown';
}

function sendEvent(event, meta, onSent) {
  const postData = JSON.stringify(event);

  const options = {
    hostname: HOST,
    port: PORT,
    path: '/event',
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(postData)
    },
    timeout: TIMEOUT
  };

  const req = http.request(options, (res) => {
    writeDebugLog({
      phase: 'http_ok',
      event: meta.eventName,
      sessionId: meta.sessionId || null,
      toolName: meta.toolName || null,
      statusCode: res.statusCode || null,
    });

    // Consume response to free up socket
    res.on('data', () => {});
    res.on('end', () => {
      if (onSent) {
        onSent();
      } else {
        process.exit(0);
      }
    });
  });

  req.on('error', (err) => {
    writeDebugLog({
      phase: 'http_error',
      event: meta.eventName,
      sessionId: meta.sessionId || null,
      toolName: meta.toolName || null,
      error: err instanceof Error ? err.message : String(err),
    });
    // Silent failure - WinIsland might not be running
    if (onSent) {
      onSent();
    } else {
      process.exit(0);
    }
  });

  req.on('timeout', () => {
    writeDebugLog({
      phase: 'http_timeout',
      event: meta.eventName,
      sessionId: meta.sessionId || null,
      toolName: meta.toolName || null,
    });
    req.destroy();
    if (onSent) {
      onSent();
    } else {
      process.exit(0);
    }
  });

  req.write(postData);
  req.end();
}

// Read-only tools that are safe to auto-approve
const SAFE_TOOLS = new Set([
  'Read', 'Glob', 'Grep', 'LS', 'WebFetch', 'WebSearch',
]);

function pollForResponse(responseKey, callback) {
  const responsePath = path.join(
    process.env.TEMP || process.env.TMP || '/tmp',
    `winisland_response_${responseKey}.json`
  );

  const startTime = Date.now();
  const maxWait = 110000; // slightly under the 120s hook timeout

  function check() {
    if (Date.now() - startTime > maxWait) {
      writeDebugLog({
        phase: 'poll_timeout',
        responseKey,
        responsePath,
      });
      callback(null);
      return;
    }

    fs.readFile(responsePath, 'utf8', (err, data) => {
      if (err) {
        // File not yet written; poll again after 300ms
        setTimeout(check, 300);
        return;
      }

      try {
        const response = JSON.parse(data);
        writeDebugLog({
          phase: 'response_received',
          responseKey,
          responsePreview: JSON.stringify(response).slice(0, 200),
        });
        // Clean up response file
        try { fs.unlinkSync(responsePath); } catch (_) {}
        callback(response);
      } catch (parseErr) {
        writeDebugLog({
          phase: 'response_parse_error',
          responseKey,
          error: parseErr instanceof Error ? parseErr.message : String(parseErr),
        });
        setTimeout(check, 300);
      }
    });
  }

  check();
}

// Timeout fallback: generous timeout to allow PreToolUse polling to complete
setTimeout(() => {
  process.exit(0);
}, 115000);
