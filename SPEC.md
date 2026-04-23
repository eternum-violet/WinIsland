# Claude Island Windows — 完整技术规格书

> 本文档是项目的唯一需求来源。请严格按照本文档实现，不要自行添加未列出的功能。
> 每完成一个功能模块，更新 `todolist.json` 中对应项的 `done` 字段为 `true`。

---

## 1. 项目概述

### 是什么

一个 Windows 桌面应用，模仿 macOS 灵动岛（Dynamic Island）设计，浮在屏幕顶部居中位置，**实时展示 Claude Code 的工作状态**。

### 不是什么

- 不是 Toast 通知工具（没有系统通知弹窗）
- 不是提示音工具（Phase 1 不涉及声音）
- 不是 Claude Code 的 GUI 客户端（不能在里面输入命令）

### 核心体验

用户在 CMD/PowerShell/终端里用 Claude Code 下达任务后，切去做其他事。屏幕顶部的小药丸会：

1. **变为 working 状态** — 药丸内出现呼吸脉冲动画，用户余光可感知"在干活"
2. **任务完成时扩展** — 药丸平滑扩大，显示项目名 + "✅ 任务完成"，停留 4 秒后收回
3. **需要权限时闪烁** — 药丸变色闪烁，显示"🔐 需要确认"，提醒用户切回终端

---

## 2. 技术栈

| 层级 | 技术 | 版本要求 |
|------|------|----------|
| 框架 | Electron | >= 28 |
| 前端 | React 18 + CSS | - |
| 构建 | Vite | >= 5 |
| 通信 | ws (WebSocket) | - |
| 打包 | electron-builder | - |
| 包管理 | npm | >= 8 |

**不使用**：TypeScript（MVP 阶段用 JS 降低复杂度）、Tailwind（纯 CSS 更可控动画）、任何状态管理库（React useState/useReducer 足够）。

---

## 3. 项目结构

```
claude-island-win/
├── CLAUDE.md                          # Claude Code 项目指令
├── SPEC.md                            # 本文档
├── todolist.json                      # 进度追踪
├── package.json
├── package-lock.json
├── vite.config.js                     # Vite 配置（React + Electron renderer）
├── electron-builder.yml               # 打包配置
│
├── electron/                          # Electron 主进程
│   ├── main.js                        # 入口：窗口创建、托盘、WebSocket Server
│   ├── preload.js                     # contextBridge 暴露 IPC 给 renderer
│   ├── ws-server.js                   # WebSocket Server 封装
│   ├── tray.js                        # 系统托盘逻辑
│   ├── installer.js                   # 安装/卸载 Claude Code Hooks 的逻辑
│   └── auto-launch.js                 # 开机自启管理
│
├── src/                               # React 前端（renderer 进程）
│   ├── main.jsx                       # React 挂载入口
│   ├── App.jsx                        # 根组件
│   ├── App.css                        # 全局样式
│   ├── components/
│   │   ├── Island.jsx                 # 灵动岛主组件（状态路由）
│   │   ├── Island.css                 # 灵动岛所有状态的样式 + 动画
│   │   ├── IslandIdle.jsx             # idle 状态子组件
│   │   ├── IslandWorking.jsx          # working 状态子组件
│   │   ├── IslandDone.jsx             # done 状态子组件
│   │   └── IslandAttention.jsx        # attention 状态子组件
│   └── hooks/
│       └── useSession.js              # 会话状态管理 hook
│
├── hook-scripts/                      # Claude Code Hook 脚本（安装到用户机器）
│   └── hook-handler.js                # 被 Claude Code 调用，转发事件到 WebSocket
│
├── assets/
│   ├── icon.ico                       # 应用图标（系统托盘用）
│   └── icon.png                       # 256x256 PNG 图标
│
└── index.html                         # Electron renderer 入口 HTML
```

---

## 4. 通信协议

### 4.1 整体流程

```
Claude Code Hook 事件
    → hook-handler.js (读 stdin JSON, 50ms 内完成)
    → WebSocket Client 发消息到 ws://127.0.0.1:19823
    → Electron Main Process (ws-server.js 接收)
    → IPC 推送到 Renderer
    → React UI 状态切换 + 动画
```

### 4.2 Hook Handler → Electron 的 WebSocket 消息格式

```json
{
  "type": "claude-event",
  "event": "Stop | Notification | SubagentStop | SessionStart | UserPromptSubmit",
  "data": {
    "project": "my-project",
    "cwd": "C:/Users/xxx/projects/my-project",
    "sessionId": "abc-123",
    "message": "可选的消息文本",
    "notificationType": "permission_prompt | idle_prompt",
    "stopHookActive": false,
    "timestamp": 1700000000000
  }
}
```

### 4.3 Electron IPC → Renderer 的消息格式

```json
{
  "channel": "session-update",
  "payload": {
    "sessionId": "abc-123",
    "project": "my-project",
    "state": "idle | working | done | attention",
    "message": "可选的展示文本",
    "timestamp": 1700000000000
  }
}
```

### 4.4 状态机转换规则

```
                  UserPromptSubmit
        ┌────────────────────────────┐
        ▼                            │
     [idle] ──UserPromptSubmit──► [working]
        ▲                            │
        │  4秒后自动                  │ Stop / SubagentStop
        │                            ▼
     [idle] ◄──4秒后自动──────── [done]
        ▲                            
        │  用户切回终端后              
        │                            
     [idle] ◄──────────────────  [attention]
        ▲                            ▲
        │                            │
        └────── Notification(permission_prompt / idle_prompt)
                 从 working 跳转
```

**关键规则：**

- `UserPromptSubmit` → 进入 `working`（用户提交了新指令）
- `Stop` 且 `stopHookActive === false` → 从 `working` 进入 `done`
- `Stop` 且 `stopHookActive === true` → 忽略（防止循环）
- `Notification(permission_prompt)` → 进入 `attention`
- `Notification(idle_prompt)` → 进入 `done`（Claude 空闲等待输入）
- `done` 状态持续 **4 秒**后自动回到 `idle`
- `attention` 状态**不自动消失**，直到下一个 `UserPromptSubmit` 事件
- `SessionStart(startup)` → 短暂显示项目名后进入 `idle`
- 如果 Electron 应用启动时没有 WebSocket 消息，保持 `idle`

---

## 5. Electron 主进程详细设计

### 5.1 main.js — 窗口配置

```javascript
// 关键窗口参数
const win = new BrowserWindow({
  width: 220,            // idle 状态默认宽度
  height: 36,            // 药丸高度
  x: Math.round(screenWidth / 2 - 110),  // 屏幕顶部居中
  y: 12,                 // 距屏幕顶部 12px
  frame: false,          // 无边框
  transparent: true,     // 透明背景
  alwaysOnTop: true,     // 始终置顶
  skipTaskbar: true,     // 不显示在任务栏
  resizable: false,
  focusable: false,      // 不抢焦点（重要！不能打断用户输入）
  hasShadow: false,
  webPreferences: {
    preload: path.join(__dirname, 'preload.js'),
    contextIsolation: true,
    nodeIntegration: false,
  }
});

// 允许鼠标穿透 idle 状态（药丸区域外）
win.setIgnoreMouseEvents(true, { forward: true });
```

**窗口大小动态调整**（由 renderer 通过 IPC 请求）：

| 状态 | 宽度 | 高度 | 动画时长 |
|------|------|------|----------|
| idle | 220px | 36px | 300ms ease-out |
| working | 220px | 36px | 无变化 |
| done | 340px | 64px | 400ms cubic-bezier(0.34, 1.56, 0.64, 1) |
| attention | 300px | 56px | 350ms ease-out |

### 5.2 ws-server.js — WebSocket Server

```javascript
// 端口：19823（固定，不可配置）
// 协议：ws://127.0.0.1:19823
// 仅监听 127.0.0.1，不暴露到外网
// 
// 接收消息后：
// 1. 解析 JSON
// 2. 根据状态机规则计算新状态
// 3. 通过 IPC 发送 session-update 到 renderer
// 4. 调用 win.setBounds() 调整窗口大小
//
// 连接断开时不做任何处理（hook-handler.js 是短连接，发完即断）
```

### 5.3 tray.js — 系统托盘

托盘菜单项：

```
Claude Island
─────────────
✅ 开机自启          (toggle)
─────────────
重新安装 Hooks       (click → 执行 installer.js)
卸载 Hooks          (click → 移除 settings.json 中的 hooks)
─────────────
退出                 (click → app.quit())
```

### 5.4 installer.js — Hook 安装器

**安装逻辑**：读取 `~/.claude/settings.json`，合并写入以下 hooks：

```json
{
  "hooks": {
    "Stop": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "node \"{HANDLER_PATH}\"", "timeout": 5 }]
    }],
    "Notification": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "node \"{HANDLER_PATH}\"", "timeout": 5 }]
    }],
    "SubagentStop": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "node \"{HANDLER_PATH}\"", "timeout": 5 }]
    }],
    "UserPromptSubmit": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "node \"{HANDLER_PATH}\"", "timeout": 5 }]
    }],
    "SessionStart": [{
      "matcher": "startup",
      "hooks": [{ "type": "command", "command": "node \"{HANDLER_PATH}\"", "timeout": 5 }]
    }]
  }
}
```

其中 `{HANDLER_PATH}` 替换为 `hook-scripts/hook-handler.js` 的绝对路径，路径中的反斜杠替换为正斜杠。

**关键要求**：
- 安装前备份原 `settings.json` 为 `settings.json.backup_{timestamp}`
- 不覆盖用户已有的其他 hooks，只追加
- 如果检测到已安装（command 包含 `claude-island`），则更新路径而非重复添加
- 卸载时只移除包含 `claude-island` 的 hook 条目

### 5.5 auto-launch.js — 开机自启

使用 Electron 的 `app.setLoginItemSettings()` 实现，不依赖第三方库。

---

## 6. Hook Handler 脚本

### hook-scripts/hook-handler.js

这个文件会被 Claude Code 的 hooks 系统调用。它**不是** Electron 应用的一部分，而是一个独立的轻量 Node.js 脚本。

**要求**：
- 零依赖（不使用任何 npm 包，纯 Node.js 标准库）
- 读取 stdin 的 JSON
- 建立 WebSocket 连接到 `ws://127.0.0.1:19823`
- 发送格式化的消息
- 连接失败静默退出（Electron 应用可能没运行）
- 总执行时间 < 200ms
- 使用 Node.js 内置的 `http` 模块升级为 WebSocket（不用 ws 库）
  或者更简单：**使用 HTTP POST** 代替 WebSocket

**替代方案（推荐，更简单）**：用 HTTP POST 代替 WebSocket

```
hook-handler.js  →  HTTP POST http://127.0.0.1:19823/event  →  Electron Main Process
```

这样 hook-handler.js 不需要实现 WebSocket 握手，直接用 Node.js 内置的 `http.request` 即可。Electron 侧用内置的 `http.createServer` 接收。

**选择 HTTP 还是 WebSocket 由实现者决定，只要满足以下约束**：
1. hook-handler.js 零外部依赖
2. 连接失败时静默退出，不影响 Claude Code
3. 总执行时间 < 200ms
4. 消息格式符合 4.2 节定义

---

## 7. React 前端详细设计

### 7.1 Island.jsx — 主组件

```jsx
// 伪代码结构
function Island() {
  const { state, project, message } = useSession();

  return (
    <div className={`island island--${state}`}>
      {state === 'idle' && <IslandIdle />}
      {state === 'working' && <IslandWorking project={project} />}
      {state === 'done' && <IslandDone project={project} message={message} />}
      {state === 'attention' && <IslandAttention project={project} message={message} />}
    </div>
  );
}
```

### 7.2 各状态视觉设计

#### idle 状态
- 外观：黑色圆角药丸，220×36px，圆角 18px
- 内容：小圆点（灰色），无文字
- 交互：鼠标穿透（不响应点击）
- 透明度：0.6（低存在感）

#### working 状态
- 外观：黑色药丸，220×36px，与 idle 相同尺寸
- 内容：项目名（白色小字，13px）+ 呼吸脉冲动画
- 动画：药丸内部有一个从中心向外扩散的微光效果，2s 循环
  ```css
  @keyframes pulse {
    0%, 100% { box-shadow: inset 0 0 20px rgba(147, 130, 255, 0); }
    50% { box-shadow: inset 0 0 20px rgba(147, 130, 255, 0.3); }
  }
  ```
- 透明度：0.9

#### done 状态（核心动画）
- 过渡动画：药丸从 220×36 **平滑扩展**到 340×64，圆角从 18px 到 20px
- 扩展使用 `cubic-bezier(0.34, 1.56, 0.64, 1)` 弹性曲线，产生轻微回弹感
- 内容：左侧绿色圆点 + 项目名（白色 14px 加粗）+ "任务完成"（灰色 12px）
- 背景：黑色，透明度 0.95
- **4 秒后**平滑收回到 idle 尺寸

#### attention 状态
- 过渡动画：药丸扩展到 300×56
- 内容：左侧橙色闪烁圆点 + 项目名 + "需要确认"
- 背景：黑色带微弱橙色边框
- 边框动画：
  ```css
  @keyframes border-pulse {
    0%, 100% { border-color: rgba(255, 160, 50, 0.3); }
    50% { border-color: rgba(255, 160, 50, 0.8); }
  }
  ```
- **不自动消失**

### 7.3 useSession.js — 状态管理 Hook

```javascript
// 管理的状态：
// - state: 'idle' | 'working' | 'done' | 'attention'
// - project: string (当前项目名)
// - message: string (展示消息)
// - sessionId: string
//
// 监听 window.electronAPI.onSessionUpdate(callback)
// 根据 4.4 节状态机规则更新状态
// done 状态启动 4 秒定时器自动回到 idle
// 新事件到来时清除旧定时器
```

### 7.4 窗口大小同步

状态切换时，renderer 通过 IPC 通知 main process 调整窗口尺寸：

```javascript
// renderer 侧
window.electronAPI.resizeIsland({ width: 340, height: 64 });

// main process 侧（preload.js 暴露的 API）
ipcMain.on('resize-island', (event, bounds) => {
  // 使用 setBounds 配合动画
  // 保持窗口水平居中（x 需要根据新宽度重算）
  const { width: screenW } = screen.getPrimaryDisplay().workAreaSize;
  win.setBounds({
    x: Math.round(screenW / 2 - bounds.width / 2),
    y: 12,
    width: bounds.width,
    height: bounds.height,
  });
});
```

### 7.5 鼠标穿透逻辑

- idle 状态：完全穿透 `win.setIgnoreMouseEvents(true, { forward: true })`
- 其他状态：药丸区域可点击，外围穿透
- renderer 监听 mouseenter/mouseleave 动态切换：
  ```javascript
  // 在 island div 上
  onMouseEnter={() => window.electronAPI.setMousePassthrough(false)}
  onMouseLeave={() => window.electronAPI.setMousePassthrough(true)}
  ```

---

## 8. 首次启动流程

1. 用户首次打开 Electron 应用
2. 检测 `~/.claude/settings.json` 是否已包含 claude-island hooks
3. 如果没有，自动执行 installer.js 安装 hooks
4. 系统托盘显示图标
5. 屏幕顶部显示 idle 状态药丸
6. 如果安装成功，药丸短暂显示"✅ Hooks 已安装，请重启 Claude Code"（3 秒后收回）

---

## 9. 开发与调试

### 开发模式启动

```bash
npm install
npm run dev        # 启动 Vite dev server + Electron
```

### 模拟 Hook 事件（用于调试）

在另一个终端中运行：

```bash
# 模拟用户提交指令（进入 working）
curl -X POST http://127.0.0.1:19823/event -H "Content-Type: application/json" -d "{\"type\":\"claude-event\",\"event\":\"UserPromptSubmit\",\"data\":{\"project\":\"my-app\",\"sessionId\":\"test-1\",\"timestamp\":$(date +%s000)}}"

# 模拟任务完成（进入 done）
curl -X POST http://127.0.0.1:19823/event -H "Content-Type: application/json" -d "{\"type\":\"claude-event\",\"event\":\"Stop\",\"data\":{\"project\":\"my-app\",\"sessionId\":\"test-1\",\"stopHookActive\":false,\"timestamp\":$(date +%s000)}}"

# 模拟权限请求（进入 attention）
curl -X POST http://127.0.0.1:19823/event -H "Content-Type: application/json" -d "{\"type\":\"claude-event\",\"event\":\"Notification\",\"data\":{\"project\":\"my-app\",\"sessionId\":\"test-1\",\"notificationType\":\"permission_prompt\",\"message\":\"需要执行 npm install\",\"timestamp\":$(date +%s000)}}"
```

### 打包

```bash
npm run build       # 构建前端
npm run package     # 打包为 Windows 安装包
```

---

## 10. 不做的事（MVP 边界）

以下功能明确**不在 Phase 1 范围内**：

- ❌ 声音/提示音
- ❌ 任务耗时统计
- ❌ macOS / Linux 支持
- ❌ 自动更新
- ❌ TypeScript

---

## 11. 自定义主题系统

### 11.1 主题配置结构

用户可以在 `~/.claude/claude-island.json` 中配置主题：

```json
{
  "theme": {
    "name": "default | dark | light | custom",
    "colors": {
      "idleBg": "rgba(0, 0, 0, 0.6)",
      "workingBg": "rgba(0, 0, 0, 0.9)",
      "doneBg": "rgba(0, 0, 0, 0.95)",
      "attentionBg": "rgba(0, 0, 0, 0.9)",
      "attentionBorder": "rgba(255, 160, 50, 0.5)",
      "accent": "#937eff",
      "success": "#4ade80",
      "warning": "#ff9f32",
      "text": "#ffffff",
      "textMuted": "#888888"
    },
    "font": {
      "family": "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif",
      "size": 13
    },
    "animations": {
      "workingPulse": true,
      "attentionBlink": true,
      "doneExpand": true
    }
  }
}
```

### 11.2 内置主题

| 主题名 | 描述 |
|--------|------|
| default | 默认黑底紫/绿/橙色 |
| dark | 纯黑底，更高对比度 |
| light | 白底灰字，适合浅色背景 |

### 11.3 主题应用流程

1. Electron 启动时读取 `~/.claude/claude-island.json`
2. 如有 theme 配置，生成 CSS 变量注入到 Island.css
3. React 组件通过 CSS 变量使用主题色
4. 主题变更通过 IPC 通知 renderer 重新渲染

---

## 12. 点击跳转终端

### 12.1 功能描述

当用户点击灵动岛时，自动激活/聚焦 Claude Code 所在的终端窗口。

### 12.2 实现方案

使用 Windows API `SetForegroundWindow` + `ShowWindow` 配合 terminal 的类名/标题查找。

支持的终端（按优先级）：
1. Windows Terminal (`wt.exe`) — 查找 Claude Code 标签页
2. PowerShell 窗口
3. cmd.exe 窗口
4. 旧版 Windows Console (`conhost.exe`)

### 12.3 聚焦逻辑

```javascript
// main.js
function activateTerminal() {
  // 1. 尝试通过 PID 找到终端窗口（如果 Claude Code 子进程）
  // 2. 尝试通过窗口标题包含 "Claude Code" 或 "claude" 查找
  // 3. 尝试通过最近的活动窗口查找
  // 4. 如果都找不到，激活用户当前活动的终端
}
```

### 12.4 IPC 通道

```javascript
// preload.js 暴露
window.electronAPI = {
  activateTerminal: () => ipcRenderer.invoke('activate-terminal'),
  // ...
}
```

---

## 13. 灵动岛内审批按钮

### 13.1 功能描述

在 `attention` 状态下，灵动岛展开显示权限请求详情，并提供 Approve/Deny 按钮，用户可以直接在岛内审批，无需切换到终端。

### 13.2 UI 设计

```
┌─────────────────────────────────────┐
│ 🔐 需要确认                    [×]  │
├─────────────────────────────────────┤
│ npm install -g typescript           │
│                                      │
│    [拒绝]          [允许]           │
└─────────────────────────────────────┘
```

### 13.3 实现方案

使用 `PostToolUse` hook 拦截权限请求事件，通过 HTTP 携带 `permissionRequest` 类型通知 Electron。

```javascript
// hook-handler.js 解析 permissionRequest
if (notificationType === 'permission_request') {
  sendToElectron({
    type: 'permission_prompt',
    message: toolCommand,  // 如 "npm install -g typescript"
    sessionId,
    project,
    requiresApproval: true
  });
}
```

### 13.4 审批流程

1. attention 状态显示详情 + 按钮
2. 用户点击"允许" → HTTP POST approve → hook 允许执行
3. 用户点击"拒绝" → HTTP POST deny → hook 拒绝执行
4. Electron 收到结果后切换回 idle

### 13.5 API 端点

```
POST /permission/approve
POST /permission/deny
```

---

## 14. 多 Island 并排显示

### 14.1 功能描述

支持同时管理多个 Claude Code 会话，每个项目/子代理显示为独立的迷你岛，多个岛并排显示。

### 14.2 布局规则

```
[岛1: my-app] [岛2: docs] [岛3: agent-1]
```

- 每个岛最小宽度 180px，最大 280px
- 超过 5 个岛时显示 +N 溢出按钮
- 岛之间 8px 间距
- 总宽度超过屏幕 80% 时换行

### 14.3 数据结构

```javascript
// main.js
const sessions = new Map(); // sessionId -> sessionState
```

### 14.4 状态管理

- 每个 session 独立计时（done 状态 4 秒后消失）
- attention 状态不消失，直到用户操作
- 同一项目的多次事件更新同一个岛
- 新 session 追加到右侧

### 14.5 React 组件改造

```jsx
// IslandGrid.jsx
function IslandGrid() {
  const { sessions } = useSessions(); // Map<sessionId, sessionState>

  return (
    <div className="island-grid">
      {Array.from(sessions.values()).map(session => (
        <Island key={session.sessionId} {...session} />
      ))}
    </div>
  );
}
```

---

## 15. 任务历史

### 15.1 功能描述

记录已完成的任务历史，用户可以在灵动岛的历史面板中查看过去的任务记录。

### 15.2 数据存储

存储在 `~/.claude/claude-island-history.json`：

```json
{
  "history": [
    {
      "id": "abc-123",
      "project": "my-app",
      "sessionId": "sess-456",
      "command": "重构用户认证模块",
      "status": "success",
      "startTime": 1700000000000,
      "endTime": 1700000060000,
      "duration": 60000,
      "toolsUsed": ["Edit", "Bash", "Read"]
    },
    {
      "id": "def-789",
      "project": "docs",
      "sessionId": "sess-789",
      "command": "更新 API 文档",
      "status": "failed",
      "error": "权限被拒绝",
      "startTime": 1700000100000,
      "endTime": 1700000120000,
      "duration": 20000,
      "toolsUsed": ["Read"]
    }
  ],
  "lastId": "def-789"
}
```

### 15.3 历史面板

- 鼠标悬停到 idle 状态的岛 → 显示小三角
- 点击打开历史面板（下拉列表）
- 显示最近 20 条历史，按时间倒序
- 每条显示：项目名、任务描述、时间、状态图标
- 点击历史项可以查看详情

### 15.4 IPC 通道

```javascript
// 保存历史
ipcMain.handle('save-task-history', (event, task) => {
  // 追加到 claude-island-history.json
});

// 加载历史
ipcMain.handle('get-task-history', () => {
  // 读取并返回历史列表
});
```

---

## 16. 新功能任务列表
