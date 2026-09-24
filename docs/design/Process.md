# Process 设计

区分 exec 和 pty：

- exec 模仿 mcp 协议的 streamable-http，由服务端决定是否把响应升级成 http/2 流：如果命令在请求的 `timeout` 内结束则直接返回代表执行结果的响应；否则升级流，在流中多路返回 stdout 和 stderr，以及最终的 exit code；
- pty 提供专门的一套 session 管理 API，每个 session 通过 websocket 实现双向通信；

具体设计参考 k8s api server 和 docker 的 remote api 设计。

## 端点与寻址

exec、shell 与 pty 各占一个子路由：

- 既可挂在工作区路由下（`/workspaces/{id}/exec`、`/workspaces/{id}/shell`、`/workspaces/{id}/pty`，`cwd` 为工作区相对路径）；
- 也可直接挂载（`/exec`、`/shell`、`/pty`，`cwd` 为绝对路径）。

工作区模式下，服务端按 [Workspace.md](./Workspace.md) 的环境变量把当前工作区根目录注入命令环境。

## exec

exec 直接执行一个可执行文件，参数逐项传递，不经过 shell 解析：

| 参数      | 说明                                                               |
| --------- | ------------------------------------------------------------------ |
| `command` | 可执行文件的路径或名称                                             |
| `args`    | 参数数组，逐项组成 argv                                            |
| `cwd`     | 工作目录                                                           |
| `env`     | 环境变量                                                           |
| `timeout` | 等待命令结束以直接返回结果的上限（毫秒），超时则升级为流；缺省 500 |

### 响应

由服务端按 `timeout` 在两种形态间选择：

- 命令在 `timeout` 内结束，返回 JSON 结果（`Content-Type: application/json`）；stdout、stderr 按 UTF-8 解码，非法序列替换，需要精确字节时用帧流：

```json
{ "status": { "status": "exited", "code": 1 }, "stdout": "out", "stderr": "err" }
```

- 否则升级为帧流（`Content-Type: application/vnd.sandbox-toolkit.exec-stream`），在流中多路返回 stdout、stderr 与终态。

两种形态的 `status` 相同：

| `status`  | 附加字段       | 场景                       |
| --------- | -------------- | -------------------------- |
| `success` | —              | 退出码为 0                 |
| `exited`  | `code`（整数） | 以非零退出码结束           |
| `failed`  | `message`      | 无法启动，或未及退出被终止 |

### 帧格式

帧流是长度前缀的帧序列，单向（服务端 → 客户端）：

| 值  | 信道   | payload                    |
| --- | ------ | -------------------------- |
| `0` | stdout | 进程标准输出，原始字节     |
| `1` | stderr | 进程标准错误，原始字节     |
| `2` | error  | 终态 `status` 的 JSON 编码 |

- 每帧为 1 字节信道标识加 4 字节大端长度，再加 payload；
- 帧间无分隔符，按长度切分，payload 可含任意字节；
- 单帧 payload 上限 4 MiB，超出即拒绝；
- stdout 与 stderr 分属不同信道，不合并；
- 流以恰好一个 error 帧结束，缺少该帧表示流被截断。

## shell

shell 执行一段脚本，由 shell 解释器负责解析：

| 参数     | 说明                                             |
| -------- | ------------------------------------------------ |
| `script` | 要执行的脚本内容                                 |
| `cwd`    | 工作目录                                         |
| `env`    | 环境变量                                         |
| `shell`  | 可选，指定使用的 shell 解释器；不指定时使用 `sh` |

shell 与 exec 共用同一套执行和响应语义，`timeout` 含义同上。

## pty session

pty 提供独立的 session 管理 API：

- 先创建 session，服务端返回 session id 和用于连接的 websocket 端点，客户端再连接该端点；
- 一个 session 对应一条 websocket；
- session 的生命周期归服务端持有，客户端断开或发送关闭信号后服务端回收该 session；
- session 设有创建和空闲超时，超时自动回收，避免容器内进程逃逸。

session 创建时至少指定要执行的命令（通常是 shell），可选指定初始 cwd、env 和终端尺寸。

### 帧格式

每条 websocket 消息首字节为信道标识，其余为 payload；信道集合固定：

| 值    | 信道         | 方向            |
| ----- | ------------ | --------------- |
| `0`   | stdin        | 客户端 → 服务端 |
| `1`   | stdout       | 服务端 → 客户端 |
| `3`   | exit / error | 服务端 → 客户端 |
| `4`   | resize       | 客户端 → 服务端 |
| `255` | close        | 双向            |

- pty 已把 stderr 合并进 stdout；
- resize 的 payload 是终端尺寸，最小为宽高两个整数；
- 退出码由 exit / error 信道携带；
- 每条 websocket 消息本身就是一帧。

### 双向交互

- 客户端 → 服务端只发送 stdin 和 resize 两类消息，顺序即终端输入顺序；
- 服务端 → 客户端发送 stdout 数据流，进程结束后追加一条 exit 消息，随后关闭连接；
- 任一端发送 close 后连接关闭，session 进入回收流程；
- 连接异常中断时服务端终止 session 并回收其资源。

## 参考：k8s 与 Docker 的命令执行端点设计

### k8s api server

- **端点**：exec 作为 pod 的子资源，`POST /api/v1/namespaces/{ns}/pods/{name}/exec`；命令与流开关放在 query 参数：`command`（可重复出现，逐项组成 argv）、`stdin`、`stdout`、`stderr`、`tty`、`container`。
- **升级方式**：先发普通 HTTP 请求，服务端返回 `101 Switching Protocols` 升级为流。1.31 起由 SPDY 迁移到 WebSocket，协商靠子协议头：WebSocket 用 `Sec-WebSocket-Protocol: v5.channel.k8s.io`。
- **多路复用帧**：每条消息前缀 1 字节 channel ID 区分流：`0`=stdin、`1`=stdout、`2`=stderr、`3`=error、`4`=resize。退出状态不占独立信道，由流的 error channel / 连接结束语义携带。

### Docker remote api

- **两段式端点**：先 `POST /containers/{id}/exec` 创建 exec 实例（返回 `Id`），再 `POST /exec/{id}/start` 启动；tty 尺寸用 `POST /exec/{id}/resize?h=&w=` 另行调整；退出码通过 `GET /exec/{id}/inspect` 带外查询（`ExitCode`），不在流内传递。
- **hijack 升级**：`start` 请求带 `Connection: Upgrade` / `Upgrade` 头以提示代理做连接劫持；服务端有升级头则返回 `101`，否则降级返回 `200` 的裸流——同一端点可同时服务"升级"与"不升级"两种响应形态。
- **多路复用帧**：非 TTY 时 stdout/stderr 复用一条流，每帧为 8 字节头（1 字节流类型 `0`=stdin/`1`=stdout/`2`=stderr + 3 字节填充 + 4 字节大端长度）加 payload；Content-Type 区分 `application/vnd.docker.multiplexed-stream` 与 `raw-stream`（TTY 场景为原始 PTY 字节流）。exec 的 `start` 请求体用 `Detach`、`Tty` 控制行为。

### k8s TTY 会话

- **开启条件**：exec 请求同时带 `stdin=true` 与 `tty=true` 才建立交互式终端；`tty=true` 会把 stderr 合并进 stdout。
- **尺寸变更**：`v3.channel.k8s.io` 起支持终端 resize，客户端监听本地 `SIGWINCH` 后把新尺寸发到独立信道，服务端再转交容器运行时调整 pty。
- **信道复用与版本**：全部输入输出复用同一条连接，首字节标识目标信道：`0`=stdin、`1`=stdout、`2`=stderr、`3`=error、`4`=resize、`255`=close。其中 `v3` 加 resize、`v4` 加退出码（JSON error 走 error 信道）、`v5` 加 close 信号；resize 载荷为 `TerminalSize{Width, Height}`。
- **升级与协商**：SPDY 用 `X-Stream-Protocol-Version: v4.channel.k8s.io`（POST 升级），WebSocket 用 `Sec-WebSocket-Protocol: v5.channel.k8s.io`（GET 升级）。
- **会话生命周期**：会话从连接升级建立，到连接结束或收到 close 信号为止；服务端用连接跟踪缓存管理活动会话，并设有流创建超时（默认 30s）。连接中断时容器内进程仍继续运行，容易留下孤儿进程。
