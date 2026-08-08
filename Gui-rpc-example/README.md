# Onemore RPC GUI

> Onemore Coding Agent 的桌面 GUI 客户端，基于 Tauri 2 + React 19 + Tailwind v4。

## 项目结构

```
Gui-rpc-example/
  README.md            本文档
  rpc-example.md       Onemore JSONL RPC 集成教学（协议速查、工程要点、排错）
  desktop-cc-gui/      参考项目（cc-gui，仅做 UI/UX 参考，不参与构建）
  onemoreGui/         ← 实际的 GUI 项目
    package.json
    tsconfig.json
    vite.config.ts
    index.html
    src/
      main.tsx
      lib/
        utils.ts           cn() helper
      styles/
        app.css            全局样式 + cc-gui 暗色主题令牌（zinc/oklch）
      rpc/
        protocol.ts        协议 view types（TS discriminated unions）
        client.ts          Tauri invoke + 事件订阅（唯一 transport 入口）
        reducer.ts         snapshot 权威 + progress 增量 reducer
      app/
        App.tsx            三栏布局入口
        store.ts           Zustand store（RPC + workspace + session + config + git）
        types.ts           前端类型
        util.ts            格式化工具
      components/
        Sidebar.tsx        左栏：工作区列表 + 搜索 + 会话列表 + 设置入口
        ChatArea.tsx       中栏：欢迎界面 + 状态栏 + transcript + composer
        RightPanel.tsx     右栏：文件树 / Git / 计划（tab 切换）
        SettingsModal.tsx  config.toml 编辑
        ApprovalDialog.tsx 审批 modal
    src-tauri/
      Cargo.toml
      tauri.conf.json
      capabilities/
        default.json
      src/
        main.rs            注册全部 Tauri command
        error.rs           GuiError
        state.rs           RpcState
        config.rs          读写 roaming/onemore/config.toml
        workspace.rs       持久化工作区列表
        session.rs         扫描 sessions/*.db SQLite 列出会话
        local.rs           git status（git2）+ 文件树（ignore crate）
        rpc/
          mod.rs
          process.rs       子进程生命周期 + pending map + 安全 shutdown
          reader.rs        增量 JSONL reader
          writer.rs        LF 单 writer
          types.rs         严格入站 DTO
          events.rs        GUI 事件 DTO
```

## 快速开始

```bash
cd onemoreGui
npm install
npm run tauri dev
```

## 功能

| 功能 | 说明 |
|---|---|
| RPC 连接 | 复用完整 JSONL 协议栈，不经 shell 启动 `onemore --rpc` 子进程 |
| 工作区管理 | 持久化工作区列表，左栏展示/添加/移除，点击即连接 |
| 会话管理 | 扫描 `roaming/onemore/sessions/*.db`，跨 workspace 列出历史会话 |
| 对话 | 流式 transcript（snapshot 权威 + progress 增量）、steer / abort |
| 审批 | 阻塞式 modal，Allow Once / Allow Session / Deny |
| config.toml | 设置弹窗直接编辑 `roaming/onemore/config.toml` |
| Git 状态 | 右栏显示分支、ahead/behind、文件变更列表 |
| 文件树 | 右栏展示工作区文件树（尊重 .gitignore） |
| 计划 | 右栏展示当前 session 的执行计划与进度 |

## 三栏布局

- **左栏**：品牌头 → 工作区列表 → 搜索框 → 会话列表 → 新建会话
- **中栏**：未连接时显示欢迎界面（工作区卡片），连接后显示状态栏 + transcript + composer
- **右栏**：文件树 / Git / 计划 三个 tab

## 协议

RPC 协议的完整说明见 [`rpc-example.md`](./rpc-example.md)。

## 致谢

前端 UI/UX 基于 [cc-gui](https://github.com/zhukunpenglinyutong/desktop-cc-gui) 的暗色主题令牌（zinc/oklch）复刻简化。
