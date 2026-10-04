mod harness;

use serde_json::{Value, json};

use harness::{
    Dir, ERROR, MAX_PAYLOAD_LEN, STDERR, STDOUT, Server, channel_bytes, decode_frames,
    terminal_status,
};

mod mount {
    use super::*;

    #[tokio::test]
    async fn global() {
        let home = Dir::new("mount-global");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({ "command": "pwd", "wait": 5000 }))
            .await;

        assert_eq!(result["status"], "exited");
        assert_eq!(result["exit_code"], 0);
        assert_eq!(
            result["stdout"].as_str().expect("stdout").trim_end(),
            home.path().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn workspace() {
        let home = Dir::new("mount-workspace-home");
        let root = Dir::new("mount-workspace-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        let response = server
            .post(
                "/workspaces/w/exec",
                json!({ "command": "pwd", "wait": 5000 }),
            )
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let result: Value = response.json().await.expect("decode exec result");

        assert_eq!(
            result["stdout"].as_str().expect("stdout").trim_end(),
            root.path().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn unknown_workspace() {
        let home = Dir::new("mount-unknown");
        let server = Server::start(&home).await;

        let response = server
            .post("/workspaces/missing/exec", json!({ "command": "true" }))
            .await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod request {
    use super::*;

    #[tokio::test]
    async fn exec() {
        let home = Dir::new("request-exec");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({ "command": "printf", "args": ["a b"], "wait": 5000 }))
            .await;

        assert_eq!(result["stdout"], "a b");
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn shell() {
        let home = Dir::new("request-shell");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({ "format": "shell", "script": "printf hi", "wait": 5000 }))
            .await;

        assert_eq!(result["stdout"], "hi");
    }

    #[tokio::test]
    async fn format_default() {
        let home = Dir::new("request-format-default");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({ "command": "printf", "args": ["hi"], "wait": 5000 }))
            .await;

        assert_eq!(result["stdout"], "hi");
    }

    #[tokio::test]
    async fn cwd() {
        let home = Dir::new("request-cwd-home");
        let dir = Dir::new("request-cwd-dir");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({
                "command": "pwd",
                "cwd": dir.path().to_string_lossy(),
                "wait": 5000,
            }))
            .await;

        assert_eq!(
            result["stdout"].as_str().expect("stdout").trim_end(),
            dir.path().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn env() {
        let home = Dir::new("request-env");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({
                "command": "sh",
                "args": ["-c", "printf %s \"$FOO\""],
                "env": { "FOO": "bar" },
                "wait": 5000,
            }))
            .await;

        assert_eq!(result["stdout"], "bar");
    }
}

mod direct {
    use super::*;

    #[tokio::test]
    async fn status() {
        let home = Dir::new("direct-status");
        let server = Server::start(&home).await;

        let within = server
            .exec(json!({ "command": "true", "wait": 5000 }))
            .await;
        assert_eq!(within.status(), reqwest::StatusCode::OK);

        let upgraded = server.exec(json!({ "command": "true", "wait": 0 })).await;
        assert_eq!(upgraded.status(), reqwest::StatusCode::OK);
    }

    #[tokio::test]
    async fn exited() {
        let home = Dir::new("direct-exited");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({ "command": "sh", "args": ["-c", "exit 3"], "wait": 5000 }))
            .await;

        assert_eq!(result["status"], "exited");
        assert_eq!(result["exit_code"], 3);
        assert!(result.get("signal").is_none());
    }

    #[tokio::test]
    async fn abnormal() {
        let home = Dir::new("direct-abnormal");
        let server = Server::start(&home).await;

        let result = server
            .exec_json(json!({
                "command": "sh",
                "args": ["-c", "kill -TERM $$"],
                "wait": 5000,
            }))
            .await;

        assert_eq!(result["status"], "signaled");
        assert_eq!(result["signal"], libc::SIGTERM);
        assert!(result.get("exit_code").is_none());
    }

    #[tokio::test]
    async fn timeout() {
        let home = Dir::new("direct-timeout");
        let server = Server::start(&home).await;

        let bytes = server
            .exec_bytes(json!({ "command": "sleep", "args": ["1"], "wait": 100 }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(terminal_status(&frames)["status"], "exited");
    }
}

mod stream {
    use super::*;

    #[tokio::test]
    async fn upgrades() {
        let home = Dir::new("stream-upgrades");
        let server = Server::start(&home).await;

        let bytes = server
            .exec_bytes(json!({ "command": "printf", "args": ["hi"], "wait": 0 }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(channel_bytes(&frames, STDOUT), b"hi");
        assert_eq!(terminal_status(&frames)["status"], "exited");
    }

    #[tokio::test]
    async fn separates() {
        let home = Dir::new("stream-separates");
        let server = Server::start(&home).await;

        let bytes = server
            .exec_bytes(json!({
                "format": "shell",
                "script": "printf out; printf err 1>&2",
                "wait": 0,
            }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(channel_bytes(&frames, STDOUT), b"out");
        assert_eq!(channel_bytes(&frames, STDERR), b"err");
    }

    #[tokio::test]
    async fn terminal() {
        let home = Dir::new("stream-terminal");
        let server = Server::start(&home).await;

        let bytes = server
            .exec_bytes(json!({ "command": "true", "wait": 0 }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(frames.iter().filter(|(id, _)| *id == ERROR).count(), 1);
        assert_eq!(frames.last().expect("a terminal frame").0, ERROR);
    }

    #[tokio::test]
    async fn chunk_limit() {
        let home = Dir::new("stream-chunk-limit");
        let server = Server::start(&home).await;

        let bytes = server
            .exec_bytes(json!({
                "format": "shell",
                "script": "head -c 5000000 /dev/zero | tr '\\0' a",
                "wait": 0,
            }))
            .await;
        let frames = decode_frames(&bytes);

        assert!(
            frames
                .iter()
                .all(|(_, payload)| payload.len() <= MAX_PAYLOAD_LEN)
        );
        assert_eq!(channel_bytes(&frames, STDOUT).len(), 5_000_000);
    }

    #[tokio::test]
    #[ignore = "receiver-side rule: the server never emits an unterminated stream"]
    async fn truncated() {
        let home = Dir::new("stream-truncated");
        let server = Server::start(&home).await;

        let bytes = server
            .exec_bytes(json!({ "command": "true", "wait": 0 }))
            .await;
        let frames = decode_frames(&bytes);

        assert!(frames.iter().any(|(id, _)| *id == ERROR));
    }
}
