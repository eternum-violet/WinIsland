#!/usr/bin/env node

const http = require('http');
const fs = require('fs');
const path = require('path');

const LOG_FILE = path.join(process.env.TEMP || '/tmp', 'winisland-hook-debug.log');

function log(message) {
  const timestamp = new Date().toISOString();
  fs.appendFileSync(LOG_FILE, `[${timestamp}] ${message}\n`);
}

log('=== Hook handler started ===');

function extractProjectName(cwd) {
  if (!cwd) return 'Unknown';
  const parts = cwd.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || 'Unknown';
}

function sendEvent(event) {
  const data = JSON.stringify(event);
  log(`Sending event: ${data}`);

  const options = {
    hostname: '127.0.0.1',
    port: 19823,
    path: '/event',
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(data)
    },
    timeout: 3000
  };

  const req = http.request(options, (res) => {
    log(`Response status: ${res.statusCode}`);
    res.on('data', (chunk) => {
      log(`Response data: ${chunk}`);
    });
  });

  req.on('error', (err) => {
    log(`Request error: ${err.message}`);
  });

  req.on('timeout', () => {
    log('Request timeout');
    req.destroy();
  });

  req.write(data);
  req.end();
}

let inputData = '';
process.stdin.on('data', (chunk) => {
  inputData += chunk;
});

process.stdin.on('end', () => {
  log(`Received input: ${inputData}`);

  try {
    const hookData = JSON.parse(inputData);
    log(`Parsed event: ${hookData.event}`);

    let toolName = null;
    let filePath = null;
    let command = null;

    if (hookData.event === 'PreToolUse' || hookData.event === 'PostToolUse') {
      toolName = hookData.toolName;
      log(`Tool name: ${toolName}`);

      if (hookData.toolInput) {
        if (hookData.toolInput.file_path) {
          filePath = hookData.toolInput.file_path;
          log(`File path: ${filePath}`);
        }
        if (hookData.toolInput.command) {
          command = hookData.toolInput.command;
          log(`Command: ${command}`);
        }
      }
    }

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
    log('Event sent successfully');
  } catch (err) {
    log(`Error: ${err.message}`);
    log(`Stack: ${err.stack}`);
  }

  process.exit(0);
});
