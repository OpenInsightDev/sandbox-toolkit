# Exec 设计

## 概览

exec 通过 HTTP 暴露命令执行，模仿 MCP 的 Streamable HTTP：请求在 `wait` 阈值内结束则直接返回 JSON，否则在同一个响应上升级为帧流，多路返回 stdout、stderr 与终态。pty 见 [Pty 设计](./Pty.md)。

工作目录与环境取自 [Workspace 设计](./Workspace.md)。

## 挂载

exec 挂载在 `/exec`；无前缀解析到 global 工作区，`/workspaces/{workspace_id}` 前缀解析到指定工作区。pty 沿用同一挂载规则（见 [Pty 设计](./Pty.md)）。

命令在所属工作区内执行，`cwd` 缺省为工作区根目录（global 即用户家目录），并把工作区变量注入环境。

### 端点

| 路径 | 语义 |
| --- | --- |
| `/exec` | global 工作区 |
| `/workspaces/{workspace_id}/exec` | 指定工作区 |

| 状态码 | 语义 | 触发条件 |
| --- | --- | --- |
| 404 | 未找到 | `{workspace_id}` 不存在 |

### 测试

- `mount::global`：`/exec` 在 global 工作区内执行，`cwd` 缺省为家目录。
- `mount::workspace`：`/workspaces/{id}/exec` 作用于该 workspace。
- `mount::unknown_workspace`：访问不存在的 workspace 返回 `404`。

## 请求

请求体由 `format` 区分两种载荷，公共字段 `cwd`、`env`、`wait`。

| 字段 | 说明 |
| --- | --- |
| `format` | 载荷种类，`exec`（缺省）或 `shell` |
| `cwd` | 可选，工作目录，缺省为工作区根目录 |
| `env` | 可选，环境变量，覆盖继承环境与注入的工作区变量 |
| `wait` | 可选，等待命令结束以直接返回 JSON 的上限（毫秒）；缺省 `0` 表示不等待、直接流式 |

`format=exec` 的载荷：

| 字段 | 说明 |
| --- | --- |
| `command` | 必填，可执行程序 |
| `args` | 可选，参数数组，逐项组成 argv |

`format=shell` 的载荷：

| 字段 | 说明 |
| --- | --- |
| `script` | 必填，脚本内容 |
| `shell` | 可选，解释器名或路径，缺省 `sh` |

### 测试

- `request::exec`：`format=exec` 按 `command` 与 `args` 组成 argv 执行。
- `request::shell`：`format=shell` 用 `shell`（缺省 `sh`）执行 `script`。
- `request::format_default`：缺省 `format` 为 `exec`。
- `request::cwd`：`cwd` 决定命令的工作目录。
- `request::env`：`env` 覆盖继承环境与注入的工作区变量。

## 直接返回

两种应答均为 `200`，以 `Content-Type` 区分，不做 `101` 升级。命令在 `wait` 内结束时，响应直接返回 JSON。

终态由 `status` 判别，取值决定伴随的终态字段：

| `status` | 含义 | 终态字段 |
| --- | --- | --- |
| `exited` | 进程正常退出，有退出码 | `exit_code`，整数 |
| `signaled` | 进程被信号终止，无退出码 | `signal`，信号编号，整数 |

正常退出：

```json
{ "status": "exited", "exit_code": 1, "stdout": "out", "stderr": "err" }
```

异常退出：

```json
{ "status": "signaled", "signal": 9, "stdout": "out", "stderr": "err" }
```

终态字段（`exit_code` 或 `signal`）二者互斥，只随对应的 `status` 出现。

直接返回的 JSON 为终态 JSON 加上 `stdout`、`stderr`（捕获的文本）；终态 JSON 即帧流 [error 信道](#帧流)与 pty [error 信道](#pty) 承载的那份。

命令未在 `wait` 内结束时，同一个 `200` 响应以帧流承载。

### 测试

- `direct::status`：直接返回与升级帧流都返回 `200`。
- `direct::exited`：正常退出时 `status` 为 `exited`，含 `exit_code`、`stdout`、`stderr`，不含 `signal`。
- `direct::abnormal`：异常退出时 `status` 为 `signaled`，含 `signal`、`stdout`、`stderr`，不含 `exit_code`。
- `direct::timeout`：命令未在 `wait` 内结束，同一响应以帧流承载。

## 帧流

流式应答为长度前缀的帧序列，单向（服务端 → 客户端），信道编号与 pty 统一。exec 单向，不支持 stdin，信道 `0` 仅保留编号。

| 值 | 信道 | payload |
| --- | --- | --- |
| `0` | stdin | 保留，exec 不使用 |
| `1` | stdout | 进程标准输出，原始字节 |
| `2` | stderr | 进程标准错误，原始字节 |
| `3` | error | 终态 JSON：`{status, exit_code}` 或 `{status, signal}` |

- 每帧为 1 字节信道标识加 4 字节大端长度，再加 payload；
- 帧间无分隔符，按长度切分，payload 可含任意字节；
- 发送端按 4 MiB 上限切分帧，接收端遇 payload 超过 4 MiB 的帧报协议错误；
- stdout 与 stderr 分属不同信道，不合并；
- 流以恰好一个 error 帧结束，缺少该帧表示流被截断。

### 测试

- `stream::upgrades`：`wait` 缺省 `0` 时同一 `200` 响应立即以帧流承载。
- `stream::separates`：stdout 与 stderr 分别出现在信道 `1` 与 `2`，不合并。
- `stream::terminal`：流以恰好一个 error 帧结束，payload 为终态 JSON（`{status, exit_code}` 或 `{status, signal}`）。
- `stream::truncated`：缺少 error 帧的流被判定为截断。
- `stream::chunk_limit`：发送端按 4 MiB 上限切分帧，接收端对超过 4 MiB 的 payload 报协议错误。
