#!/bin/bash

# Test script for Claude Code Hook integration

echo "Testing Claude Code Hook Handler..."

# Test 1: UserPromptSubmit event
echo ""
echo "Test 1: Sending UserPromptSubmit event..."
echo '{
  "event": "UserPromptSubmit",
  "cwd": "D:\\winisland\\WinIsland",
  "sessionId": "test-session-1",
  "timestamp": 1700000000000
}' | node hook-scripts/hook-handler.js

sleep 1

# Test 2: Stop event (task completed)
echo ""
echo "Test 2: Sending Stop event (task completed)..."
echo '{
  "event": "Stop",
  "cwd": "D:\\winisland\\WinIsland",
  "sessionId": "test-session-1",
  "stopHookActive": false,
  "timestamp": 1700000010000
}' | node hook-scripts/hook-handler.js

sleep 1

# Test 3: Notification event (permission prompt)
echo ""
echo "Test 3: Sending Notification event (permission prompt)..."
echo '{
  "event": "Notification",
  "cwd": "D:\\winisland\\WinIsland",
  "sessionId": "test-session-1",
  "notificationType": "permission_prompt",
  "message": "需要执行 npm install",
  "timestamp": 1700000020000
}' | node hook-scripts/hook-handler.js

echo ""
echo "Tests completed. Check WinIsland window for status changes."
