# ONBOARD

本 crate 只提供进程 / PTY 句柄层，用来在 `crates/sandbox-toolkit/src/process/`
实现 `docs/design/Process.md` 的 pty 语义。下表是本 crate 相对该设计的已知缺口，
按"必须处理"到"记录即可"排列。

## 1. 信号终止的 `Status::Failed` 无法产出（硬缺口）

设计里终态区分 `exited{exit_code}` 与 `failed{message}`，被信号终止的进程应报
`failed`。但 `src/pty.rs` 的 wait 只取 `status.exit_code() as i32`，丢弃了
portable-pty `ExitStatus::signal()`。portable-pty 对被信号杀死的进程
`status.code()` 为 `None`，统一回退成 `code = 1`，因此 `SIGKILL`/`SIGTERM`
会和正常 `exit 1` 撞在一起，无法区分。

- 对比：exec 直接用 `std::process::ExitStatus`，能正确产出 `failed`。
- 处理方式二选一：
  - 改 crate，让退出上报携带信号（会偏离 vendored 上游，见 AGENTS.md）；
  - 接受降级：`failed` 只在服务端主动终止时合成，外部信号终止一律报
    `exited{exit_code:1}`。

## 2. 平台清理不一致

项目只支持 Linux 容器（见根目录 AGENTS.md），故可以忽略 macOS 的差异，只保留
Linux 相关的一条：

- PTY 后端没有设置 Linux `PR_SET_PDEATHSIG`（pipe 后端在 `pre_exec` 里设了）。
  服务进程崩溃时 PTY 子进程不会自动收到 SIGTERM。
- macOS 的成员 fallback 差异在 Linux-only 前提下不处理。

## 3. session / WebSocket 层需要新写（crate 不提供，属预期）

以下都不在 crate 职责内，需在 `src/process/` 落地：

- session 注册表（`session_id` → `ProcessHandle`）与 `201 {session_id,url}`；
- 由请求推导绝对基址拼 `url`；HTTP/2 CONNECT 由 vendored axum 支持；
- 30s 建连超时、5min 空闲回收、ping/pong 活动计时；
- 连接断开 / close / 异常 → `terminate()` 终止进程组并回收 session；
- 发送顺序 stdout → error → close；创建期错误映射（404/400/422）。

当前 `src/process/http.rs` 的 `create_pty` / `attach_pty` 仍是 stub。

## 4. 调用侧需复刻 exec 的解析语义

- `spawn_pty_process` 会 `env_clear()`，调用方要传**完整环境**：继承环境 +
  全部已注册工作区变量 + 请求 `env` 覆盖，并把物化二进制目录前置到 PATH
  （复用 `exec` 的 `search_path`）。
- `command` 解析（bare 名走 PATH、含 `/` 按路径、相对 `cwd`）、`args` 逐项、
  `cwd` 工作区边界，都需要复用 `exec::CommandSpec::resolve` / `resolve_cwd`
  （当前为私有，需重构暴露）。

## 5. 帧模型与设计不一致（在 sandbox-toolkit，非本 crate）

设计信道集合是 `{0 stdin, 1 stdout, 3 error, 4 resize}`，且"关闭使用
WebSocket close 帧……不设独立信道"。但 `src/process/pty.rs` 额外定义了
`Channel::Close = 255`。实现时要么去掉，要么明确它是客户端可选加速信号。

## 6. 次要

- resize 走 `ProcessHandle::resize` 直调，stdin 走 channel，二者无严格顺序保证
  （与 k8s 独立 resize 信道一致，通常可接受）。
- `spawn_pty_process` 返回的 `stderr_rx` 是立即闭合的空通道，不要当真实
  stderr 读；PTY 已把 stderr 合并进 stdout。
- crate 为 `#![cfg(unix)]`，非 Unix 平台编译为空；目标 Linux 容器无碍。
- 尚未在 `crates/sandbox-toolkit/Cargo.toml` 声明依赖 `pty`。
