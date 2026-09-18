# @sandbox-toolkit/sdk

An [Effect](https://effect.website)-based TypeScript client for the
sandbox-toolkit HTTP server. It calls the server's ordinary HTTP endpoints
directly, so browser code and other services can use the same operations the
MCP endpoint exposes as tools.

## Usage

```ts
import { Effect } from "effect";
import { SandboxToolkit } from "@sandbox-toolkit/sdk";

const program = Effect.gen(function* () {
  const client = yield* SandboxToolkit;

  const tools = yield* client.listTools;
  const window = yield* client.readFile({
    path: "/etc/hosts",
    offset: 0,
    limit: 20,
  });

  return { tools, window };
});

Effect.runPromise(
  program.pipe(Effect.provide(SandboxToolkit.layer({ baseUrl: "http://127.0.0.1:8000" }))),
);
```

`SandboxToolkit.layer` wires the platform `fetch` implementation. To reuse the
client over a custom transport (or in tests), use `SandboxToolkit.layerNoDeps`,
which requires an `HttpClient` instead.

## Operations

| Client method          | HTTP request           |
| ---------------------- | ---------------------- |
| `health`               | `GET /health`          |
| `listTools`            | `GET /tools`           |
| `describeTool(params)` | `POST /tools/describe` |
| `readFile(params)`     | `POST /fs/readFile`    |

Request and response types are the generated wire types (`ReadFileParams`,
`ReadFileResult`, `TextLine`, `DescribeToolParams`, `DescribeToolResult`,
`ListToolsResult`), re-exported from the package root.

## Errors

Every operation fails with `SandboxToolkitError`, which carries a `reason`:

| Reason                | Cause                                        |
| --------------------- | -------------------------------------------- |
| `InvalidRequestError` | HTTP 400 — bad parameters                    |
| `NotFoundError`       | HTTP 404 — unknown tool or missing file      |
| `ServerError`         | any other non-2xx status (includes `status`) |
| `TransportError`      | network failure or undecodable response body |

Handle a specific reason with `Effect.catchReason`:

```ts
client
  .readFile({ path })
  .pipe(Effect.catchReason("SandboxToolkitError", "NotFoundError", () => Effect.succeed(null)));
```

## Generated types

`src/generated` is produced by [ts-rs](https://github.com/Aleph-Alpha/ts-rs)
from the Rust wire types in `crates/server/src/model.rs`. Do not edit these
files by hand — regenerate them from the server crate:

```sh
cd crates/server && cargo test
```

Only types annotated with `#[ts(export)]` are emitted, and `TS_RS_EXPORT_DIR`
in `.cargo/config.toml` points at this directory.

## Testing

```sh
vp test     # hermetic: a stubbed fetch covers requests, decoding and errors
vp check    # format, lint and type check
```

The integration suite runs the same client against a real server and is skipped
unless `SANDBOX_TOOLKIT_BASE_URL` is set:

```sh
SANDBOX_TOOLKIT_BASE_URL=http://127.0.0.1:8000 vp test
```
