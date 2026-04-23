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

const http = require('http');

const PORT = 19823;
const HOST = '127.0.0.1';
const TIMEOUT = 150; // ms

// Read stdin
let inputData = '';

process.stdin.setEncoding('utf8');

process.stdin.on('data', (chunk) => {
  inputData += chunk;
});

process.stdin.on('end', () => {
  try {
    // Claude Code sends JSON with Windows paths containing backslashes
    // We need to escape them properly before parsing
    // Only escape backslashes that are not already escaped
    const fixedInput = inputData.replace(/\\(?!["\\/bfnrtu])/g, '\\\\');
    const hookData = JSON.parse(fixedInput);

    // Extract tool information for PreToolUse/PostToolUse events
    let toolName = null;
    let filePath = null;
    let command = null;

    if (hookData.event === 'PreToolUse' || hookData.event === 'PostToolUse') {
      toolName = hookData.toolName;

      // Extract file path from toolInput
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
      event: hookData.event || 'Unknown',
      data: {
        project: extractProjectName(hookData.cwd),
        cwd: hookData.cwd,
        sessionId: hookData.sessionId,
        message: hookData.message,
        notificationType: hookData.notificationType,
        stopHookActive: hookData.stopHookActive || false,
        timestamp: Date.now(),
        toolName: toolName,
        filePath: filePath,
        command: command
      }
    };

    sendEvent(event);
  } catch (err) {
    // Silent failure - don't interrupt Claude Code
    process.exit(0);
  }
});

function extractProjectName(cwd) {
  if (!cwd) return 'Unknown';

  // Extract last directory name from path
  const parts = cwd.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || 'Unknown';
}

function sendEvent(event) {
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
    // Consume response to free up socket
    res.on('data', () => {});
    res.on('end', () => {
      process.exit(0);
    });
  });

  req.on('error', () => {
    // Silent failure - WinIsland might not be running
    process.exit(0);
  });

  req.on('timeout', () => {
    req.destroy();
    process.exit(0);
  });

  req.write(postData);
  req.end();
}

// Timeout fallback
setTimeout(() => {
  process.exit(0);
}, 200);
