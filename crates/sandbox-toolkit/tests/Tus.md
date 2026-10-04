# Tus 设计

## 概览

`/tus` 是到 tusd 的反向代理，用于可续传的文件上传。tusd 由 sbxtkt 在启动时拉起，双方通过 UNIX socket 通信。tus 协议本身见 tusd 文档，本章只记代理与进程的约定。

## 挂载

`/tus` 与 `/tus/{*rest}` 都交给同一个代理，只有这一个入口，不随工作区前缀变化。

### 测试

- `mount::create`：`POST /tus` 到达上游，返回 `201`，`Location` 是指向 `/tus/{id}` 的绝对 URL。
- `mount::targets`：`/tus/{id}` 上的请求到达上游。

## 转发

方法、路径与查询串原样转发；逐跳头（`connection`、`transfer-encoding`、`upgrade` 等）剔除，其余头与状态码原样回传；`Host` 换成上游，`X-Forwarded-Host` 与 `X-Forwarded-Proto` 描述客户端看到的请求，上游据它们生成 `Location`。

请求体与响应体都不缓冲，代理不施加服务端默认的 2 MiB 请求体上限。

### 测试

- `proxy::resume`：两段 `PATCH` 后 `HEAD` 报出的 `Upload-Offset` 等于已传字节数。
- `proxy::download`：`GET /tus/{id}` 返回上传的原始字节。
- `proxy::terminate`：`DELETE /tus/{id}` 返回 `204`，其后 `HEAD` 返回 `404`。
- `proxy::status`：上游的状态码原样回传：未知 id 的 `HEAD` 是 `404`，没有 `Tus-Resumable` 的 `POST` 是 `412`。
- `proxy::unlimited`：单次 4 MiB 的请求体上传成功。

## 进程

启动时把嵌入的 `tusd` 物化后拉起，以 `-unix-sock $CACHE/sandbox-toolkit/tus.sock -base-path /tus -behind-proxy -upload-dir $CACHE/sandbox-toolkit/tus` 启动，不监听 TCP；服务在它开始接受请求后才对外服务。

服务收到 `SIGTERM` 或 `SIGINT` 时先停对外服务，再让 tusd 退出并等它结束：给它 10 秒自行退出，超时则强杀，服务随即退出。上游连不上时回答 `502`。

### 测试

- `process::ready`：服务开始接受请求时 socket 已在监听。
- `process::exits`：服务收到 `SIGTERM` 退出后，socket 不再可连。
- `process::unreachable`：上游不可达时 `/tus` 返回 `502`。
