mod harness;

use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};

use harness::{Dir, Server, Upload};

fn query_method() -> Method {
    Method::from_bytes(b"QUERY").expect("QUERY is a valid method")
}

/// One endpoint call: the `type` in the query, the parameters in the JSON body.
async fn request(
    server: &Server,
    method: Method,
    endpoint: &str,
    typing: &str,
    body: Value,
) -> reqwest::Response {
    server
        .send_json(method, &format!("{endpoint}?type={typing}"), body)
        .await
}

/// `QUERY`, the read half of the endpoint set.
async fn query(server: &Server, endpoint: &str, typing: &str, body: Value) -> reqwest::Response {
    request(server, query_method(), endpoint, typing, body).await
}

/// A write on the `w` workspace's mount, whose URL carries the `type`.
async fn write(server: &Server, method: Method, typing: &str, body: Value) -> reqwest::Response {
    request(server, method, "/workspaces/w/fs", typing, body).await
}

/// `DELETE` is identified by its method alone, so only the body names the target.
async fn delete(server: &Server, body: Value) -> reqwest::Response {
    server
        .send_json(Method::DELETE, "/workspaces/w/fs", body)
        .await
}

/// A workspace holding the tree the read endpoints share: a text file, a text
/// file without a trailing newline, and a subdirectory holding one file.
async fn seeded(home: &Dir, root: &Dir) -> Server {
    let server = Server::start(home).await;
    server.register("w", root.path()).await;

    std::fs::write(root.path().join("notes.txt"), "hello\nworld\n").expect("seed notes.txt");
    std::fs::write(root.path().join("readme.md"), "line one\nline two")
        .expect("seed readme.md");
    std::fs::create_dir(root.path().join("sub")).expect("seed the subdirectory");
    std::fs::write(root.path().join("sub/deep.txt"), "deep").expect("seed deep.txt");

    server
}

/// The `path` of every entry a `list` or `glob` response carries.
fn entry_paths(body: &Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .expect("entries is an array")
        .iter()
        .map(|entry| entry["path"].as_str().expect("an entry path").to_owned())
        .collect()
}

fn sorted(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths
}

/// Runs a `list` or a `glob` on the `w` workspace and returns its entry paths.
async fn entries(server: &Server, typing: &str, body: Value) -> Vec<String> {
    let response = query(server, "/workspaces/w/fs", typing, body).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK, "QUERY {typing}");

    entry_paths(&response.json::<Value>().await.expect("decode the entries"))
}

/// Opens a `watch` stream, runs `mutate`, and returns the first event reported.
async fn watch_event(
    server: &Server,
    endpoint: &str,
    body: Value,
    mutate: impl FnOnce(),
) -> Value {
    let mut response = query(server, endpoint, "watch", body).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK, "QUERY watch");

    mutate();

    let mut pending = Vec::new();
    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(30), response.chunk())
            .await
            .expect("a watch event arrives")
            .expect("read the watch stream")
            .expect("the watch stream stays open");
        pending.extend_from_slice(&chunk);

        // One event per line, so the first newline ends the first event.
        if let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            return serde_json::from_slice(&pending[..end]).expect("decode a watch event");
        }
    }
}

mod mount {
    use super::*;

    #[tokio::test]
    async fn direct() {
        let home = Dir::new("mount-direct");
        let server = Server::start(&home).await;
        let target = home.path().join("notes.txt");

        let body = json!({ "path": target.to_string_lossy(), "content": "hello\n" });
        let response = request(&server, Method::PUT, "/fs", "content", body).await;

        assert!(response.status().is_success(), "PUT /fs");
        assert_eq!(
            std::fs::read(&target).expect("read the written file"),
            b"hello\n"
        );
    }

    #[tokio::test]
    async fn workspace() {
        let home = Dir::new("mount-workspace-home");
        let root = Dir::new("mount-workspace-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        let body = json!({ "path": "notes.txt", "content": "hello\n" });
        let response = request(&server, Method::PUT, "/workspaces/w/fs", "content", body).await;

        assert!(response.status().is_success(), "PUT /workspaces/w/fs");
        assert_eq!(
            std::fs::read(root.path().join("notes.txt")).expect("read the written file"),
            b"hello\n"
        );
    }

    #[tokio::test]
    async fn unknown_workspace() {
        let home = Dir::new("mount-unknown");
        let server = Server::start(&home).await;

        let body = json!({ "path": "notes.txt", "content": "hello\n" });
        let response = request(
            &server,
            Method::PUT,
            "/workspaces/missing/fs",
            "content",
            body,
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod request {
    use super::*;

    #[tokio::test]
    async fn body() {
        let home = Dir::new("request-body-home");
        let root = Dir::new("request-body-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), b"hello\n").expect("seed the file");

        let carried = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "notes.txt" }),
        )
        .await;
        assert!(carried.status().is_success(), "parameters in the JSON body");

        // The same parameters in the query string are not the request's parameters.
        let misplaced = server
            .send_json(
                query_method(),
                "/workspaces/w/fs?type=metadata&path=notes.txt",
                json!({}),
            )
            .await;
        assert!(
            !misplaced.status().is_success(),
            "parameters in the query string"
        );
    }
}

mod content {
    use super::*;

    #[tokio::test]
    async fn text() {
        let home = Dir::new("content-text-home");
        let root = Dir::new("content-text-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\nworld\n").expect("seed the file");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "content",
            json!({ "path": "notes.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.expect("decode the content"),
            json!({ "path": "notes.txt", "content": "hello\nworld\n", "size": 12 })
        );
    }

    #[tokio::test]
    async fn not_utf8() {
        let home = Dir::new("content-not-utf8-home");
        let root = Dir::new("content-not-utf8-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.bin"), b"\xff\xfe").expect("seed the file");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "content",
            json!({ "path": "notes.bin" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn writes() {
        let home = Dir::new("content-writes-home");
        let root = Dir::new("content-writes-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let target = root.path().join("notes.txt");
        std::fs::write(&target, "before").expect("seed the file");

        let response = write(
            &server,
            Method::PUT,
            "content",
            json!({ "path": "notes.txt", "content": "after" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert_eq!(
            std::fs::read(&target).expect("read the written file"),
            b"after"
        );
    }
}

mod stream {
    use super::*;

    #[tokio::test]
    async fn read() {
        let home = Dir::new("stream-read-home");
        let root = Dir::new("stream-read-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let bytes = b"\x00\xff\xfe binary\n";
        std::fs::write(root.path().join("notes.bin"), bytes).expect("seed the file");

        let response =
            query(&server, "/workspaces/w/fs", "stream", json!({ "path": "notes.bin" })).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.bytes().await.expect("read the stream").to_vec(),
            bytes
        );
    }
}

mod metadata {
    use super::*;

    #[tokio::test]
    async fn file() {
        let home = Dir::new("metadata-file-home");
        let root = Dir::new("metadata-file-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\nworld\n").expect("seed the file");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "notes.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let metadata = response.json::<Value>().await.expect("decode the metadata");
        assert_eq!(metadata["kind"], "file");
        assert_eq!(metadata["size"], 12);
        assert!(
            metadata["modified_at"].is_string(),
            "modified_at is a timestamp"
        );
    }

    #[tokio::test]
    async fn directory() {
        let home = Dir::new("metadata-directory-home");
        let root = Dir::new("metadata-directory-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::create_dir(root.path().join("sub")).expect("seed the directory");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "sub" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let metadata = response.json::<Value>().await.expect("decode the metadata");
        assert_eq!(metadata["kind"], "directory");
    }

    #[tokio::test]
    async fn patched() {
        let home = Dir::new("metadata-patched-home");
        let root = Dir::new("metadata-patched-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\nworld\n").expect("seed the file");

        let patched = write(
            &server,
            Method::PATCH,
            "metadata",
            json!({ "path": "notes.txt", "mode": "600" }),
        )
        .await;
        assert_eq!(patched.status(), reqwest::StatusCode::NO_CONTENT);

        let metadata = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "notes.txt" }),
        )
        .await
        .json::<Value>()
        .await
        .expect("decode the metadata");

        assert_eq!(metadata["mode"], "600");
    }

    #[tokio::test]
    async fn follows_symlink() {
        let home = Dir::new("metadata-follows-home");
        let root = Dir::new("metadata-follows-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");
        std::os::unix::fs::symlink("notes.txt", root.path().join("link.txt"))
            .expect("seed the symlink");

        let patched = write(
            &server,
            Method::PATCH,
            "metadata",
            json!({ "path": "link.txt", "mode": "600" }),
        )
        .await;
        assert_eq!(patched.status(), reqwest::StatusCode::NO_CONTENT);

        let metadata = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "notes.txt" }),
        )
        .await
        .json::<Value>()
        .await
        .expect("decode the metadata");

        assert_eq!(metadata["mode"], "600", "the referent was rewritten");
    }
}

mod list {
    use super::*;

    #[tokio::test]
    async fn children() {
        let home = Dir::new("list-children-home");
        let root = Dir::new("list-children-root");
        let server = seeded(&home, &root).await;

        let body = json!({ "path": "", "depth": null, "offset": 0, "limit": null });
        let listed = entries(&server, "list", body).await;

        assert_eq!(sorted(listed), ["notes.txt", "readme.md", "sub"]);
    }

    #[tokio::test]
    async fn recursive() {
        let home = Dir::new("list-recursive-home");
        let root = Dir::new("list-recursive-root");
        let server = seeded(&home, &root).await;

        let body = json!({ "path": "", "depth": "infinity", "offset": 0, "limit": null });
        let listed = entries(&server, "list", body).await;

        assert_eq!(
            sorted(listed),
            ["notes.txt", "readme.md", "sub", "sub/deep.txt"]
        );
    }
}

mod glob {
    use super::*;

    #[tokio::test]
    async fn recursive() {
        let home = Dir::new("glob-recursive-home");
        let root = Dir::new("glob-recursive-root");
        let server = seeded(&home, &root).await;

        let body = json!({
            "path": "",
            "pattern": "**/*.txt",
            "exclude": [],
            "offset": 0,
            "limit": null,
        });
        let matched = entries(&server, "glob", body).await;

        assert_eq!(sorted(matched), ["notes.txt", "sub/deep.txt"]);
    }

    #[tokio::test]
    async fn single_component() {
        let home = Dir::new("glob-single-home");
        let root = Dir::new("glob-single-root");
        let server = seeded(&home, &root).await;

        let body = json!({
            "path": "",
            "pattern": "*.md",
            "exclude": [],
            "offset": 0,
            "limit": null,
        });
        let matched = entries(&server, "glob", body).await;

        assert_eq!(sorted(matched), ["readme.md"]);
    }

    #[tokio::test]
    async fn exclude() {
        let home = Dir::new("glob-exclude-home");
        let root = Dir::new("glob-exclude-root");
        let server = seeded(&home, &root).await;

        let body = json!({
            "path": "",
            "pattern": "**/*",
            "exclude": ["sub"],
            "offset": 0,
            "limit": null,
        });
        let matched = entries(&server, "glob", body).await;

        assert_eq!(sorted(matched), ["notes.txt", "readme.md"]);
    }
}

mod realpath {
    use super::*;

    #[tokio::test]
    async fn resolves() {
        let home = Dir::new("realpath-resolves-home");
        let root = Dir::new("realpath-resolves-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");
        std::os::unix::fs::symlink("notes.txt", root.path().join("link.txt"))
            .expect("seed the symlink");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "realpath",
            json!({ "path": "link.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let resolved = response.json::<Value>().await.expect("decode the path");
        let resolved = resolved["path"].as_str().expect("a path string");
        assert_eq!(resolved, root.path().join("notes.txt").to_string_lossy());
    }
}

mod readlink {
    use super::*;

    #[tokio::test]
    async fn targets() {
        let home = Dir::new("readlink-targets-home");
        let root = Dir::new("readlink-targets-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");
        std::os::unix::fs::symlink("notes.txt", root.path().join("link.txt"))
            .expect("seed the relative link");
        let missing = root.path().join("missing.txt");
        std::os::unix::fs::symlink(&missing, root.path().join("dangling.txt"))
            .expect("seed the dangling link");

        let relative = query(
            &server,
            "/workspaces/w/fs",
            "readlink",
            json!({ "path": "link.txt" }),
        )
        .await;
        assert_eq!(relative.status(), reqwest::StatusCode::OK);
        let relative = relative.json::<Value>().await.expect("decode the link");
        assert_eq!(relative["target"].as_str(), Some("notes.txt"));

        let absolute = query(
            &server,
            "/workspaces/w/fs",
            "readlink",
            json!({ "path": "dangling.txt" }),
        )
        .await;
        assert_eq!(absolute.status(), reqwest::StatusCode::OK);
        let absolute = absolute.json::<Value>().await.expect("decode the link");
        assert_eq!(
            absolute["target"].as_str(),
            Some(missing.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn not_a_link() {
        let home = Dir::new("readlink-not-a-link-home");
        let root = Dir::new("readlink-not-a-link-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "readlink",
            json!({ "path": "notes.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    }
}

mod access {
    use super::*;

    #[tokio::test]
    async fn probes() {
        let home = Dir::new("access-probes-home");
        let root = Dir::new("access-probes-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = query(
            &server,
            "/workspaces/w/fs",
            "access",
            json!({ "path": "notes.txt", "ok": true, "readable": true, "writable": true }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    }
}

mod lines {
    use super::*;

    #[tokio::test]
    async fn reads() {
        let home = Dir::new("lines-reads-home");
        let root = Dir::new("lines-reads-root");
        let server = seeded(&home, &root).await;

        let response = query(
            &server,
            "/workspaces/w/fs",
            "lines",
            json!({ "path": "notes.txt", "offset": 0, "limit": null }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.expect("decode the lines"),
            json!({ "lines": ["hello", "world"], "truncated": false })
        );
    }

    #[tokio::test]
    async fn trailing() {
        let home = Dir::new("lines-trailing-home");
        let root = Dir::new("lines-trailing-root");
        let server = seeded(&home, &root).await;

        let response = query(
            &server,
            "/workspaces/w/fs",
            "lines",
            json!({ "path": "readme.md", "offset": 0, "limit": null }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.expect("decode the lines"),
            json!({ "lines": ["line one", "line two"], "truncated": false })
        );
    }

    #[tokio::test]
    async fn pages() {
        let home = Dir::new("lines-pages-home");
        let root = Dir::new("lines-pages-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let content = (0..1500)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(root.path().join("many.txt"), content).expect("seed the file");

        let page = query(
            &server,
            "/workspaces/w/fs",
            "lines",
            json!({ "path": "many.txt", "offset": 0, "limit": null }),
        )
        .await;
        assert_eq!(page.status(), reqwest::StatusCode::OK);
        let page = page.json::<Value>().await.expect("decode the first page");
        let read = page["lines"].as_array().expect("lines is an array").len();
        assert_eq!(page["lines"][0], "line 0");
        assert_eq!(page["lines"][999], "line 999");
        assert_eq!(read, 1000);
        assert_eq!(page["truncated"], true);

        let rest = query(
            &server,
            "/workspaces/w/fs",
            "lines",
            json!({ "path": "many.txt", "offset": read, "limit": null }),
        )
        .await;
        let rest = rest.json::<Value>().await.expect("decode the second page");
        assert_eq!(rest["lines"][0], "line 1000");
        assert_eq!(rest["lines"].as_array().expect("lines is an array").len(), 500);
        assert_eq!(rest["truncated"], false);
    }
}

mod watch {
    use super::*;

    #[tokio::test]
    async fn create() {
        let home = Dir::new("watch-create-home");
        let root = Dir::new("watch-create-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let target = root.path().join("created.txt");

        let event = watch_event(
            &server,
            "/workspaces/w/fs",
            json!({ "path": "", "recursive": null }),
            move || std::fs::write(&target, "hi").expect("create the watched file"),
        )
        .await;

        assert_eq!(event, json!({ "event": "create", "path": "created.txt" }));
    }

    #[tokio::test]
    async fn update() {
        let home = Dir::new("watch-update-home");
        let root = Dir::new("watch-update-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let target = root.path().join("notes.txt");
        std::fs::write(&target, "one").expect("seed the watched file");

        let event = watch_event(
            &server,
            "/workspaces/w/fs",
            json!({ "path": "", "recursive": null }),
            move || std::fs::write(&target, "two").expect("update the watched file"),
        )
        .await;

        assert_eq!(event, json!({ "event": "update", "path": "notes.txt" }));
    }

    #[tokio::test]
    async fn remove() {
        let home = Dir::new("watch-remove-home");
        let root = Dir::new("watch-remove-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let target = root.path().join("doomed.txt");
        std::fs::write(&target, "x").expect("seed the watched file");

        let event = watch_event(
            &server,
            "/workspaces/w/fs",
            json!({ "path": "", "recursive": null }),
            move || std::fs::remove_file(&target).expect("remove the watched file"),
        )
        .await;

        assert_eq!(event, json!({ "event": "remove", "path": "doomed.txt" }));
    }

    #[tokio::test]
    async fn recursive() {
        let home = Dir::new("watch-recursive-home");
        let root = Dir::new("watch-recursive-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let target = root.path().join("tree/a/deep.txt");
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("seed the tree");

        let event = watch_event(
            &server,
            "/workspaces/w/fs",
            json!({ "path": "", "recursive": true }),
            move || std::fs::write(&target, "deep").expect("create the watched file"),
        )
        .await;

        assert_eq!(
            event,
            json!({ "event": "create", "path": "tree/a/deep.txt" })
        );
    }

    #[tokio::test]
    async fn subdirectory() {
        let home = Dir::new("watch-subdirectory-home");
        let root = Dir::new("watch-subdirectory-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let directory = root.path().join("sub");
        std::fs::create_dir(&directory).expect("seed the watched directory");
        let target = directory.join("here.txt");

        let event = watch_event(
            &server,
            "/workspaces/w/fs",
            json!({ "path": "sub", "recursive": null }),
            move || std::fs::write(&target, "hi").expect("create the watched file"),
        )
        .await;

        assert_eq!(event, json!({ "event": "create", "path": "sub/here.txt" }));
    }
}

mod directory {
    use super::*;

    #[tokio::test]
    async fn creates() {
        let home = Dir::new("directory-creates-home");
        let root = Dir::new("directory-creates-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        let response = write(
            &server,
            Method::PUT,
            "directory",
            json!({ "path": "made", "recursive": null }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert!(root.path().join("made").is_dir(), "the directory is on disk");
    }

    #[tokio::test]
    async fn recursive() {
        let home = Dir::new("directory-recursive-home");
        let root = Dir::new("directory-recursive-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        let response = write(
            &server,
            Method::PUT,
            "directory",
            json!({ "path": "a/b/c", "recursive": true }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert!(root.path().join("a/b/c").is_dir(), "the parents are created");
    }
}

mod symlink {
    use super::*;

    #[tokio::test]
    async fn creates() {
        let home = Dir::new("symlink-creates-home");
        let root = Dir::new("symlink-creates-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = write(
            &server,
            Method::PUT,
            "symlink",
            json!({ "path": "link.txt", "target": "notes.txt" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let metadata = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "link.txt" }),
        )
        .await
        .json::<Value>()
        .await
        .expect("decode the metadata");

        assert_eq!(metadata["kind"], "symlink");
    }

    #[tokio::test]
    async fn exists() {
        let home = Dir::new("symlink-exists-home");
        let root = Dir::new("symlink-exists-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = write(
            &server,
            Method::PUT,
            "symlink",
            json!({ "path": "notes.txt", "target": "elsewhere.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read_to_string(root.path().join("notes.txt")).expect("read the file"),
            "hello\n",
            "the file at `path` is left alone"
        );
    }
}

mod patch {
    use super::*;

    #[tokio::test]
    async fn applies() {
        let home = Dir::new("patch-applies-home");
        let root = Dir::new("patch-applies-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "one\ntwo\n").expect("seed the file");

        let response = write(
            &server,
            Method::PATCH,
            "patch",
            json!({
                "path": "notes.txt",
                "format": "unified",
                "patch": "@@ -1,2 +1,2 @@\n-one\n-two\n+ONE\n+TWO\n",
            }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        let content =
            std::fs::read_to_string(root.path().join("notes.txt")).expect("read the patched file");
        assert_eq!(content, "ONE\nTWO\n");
    }
}

mod truncate {
    use super::*;

    #[tokio::test]
    async fn extends() {
        let home = Dir::new("truncate-extends-home");
        let root = Dir::new("truncate-extends-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = write(
            &server,
            Method::PATCH,
            "truncate",
            json!({ "path": "notes.txt", "length": 12 }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let metadata = query(
            &server,
            "/workspaces/w/fs",
            "metadata",
            json!({ "path": "notes.txt" }),
        )
        .await
        .json::<Value>()
        .await
        .expect("decode the metadata");

        assert_eq!(metadata["size"], 12);
    }
}

mod copy {
    use super::*;

    #[tokio::test]
    async fn copies() {
        let home = Dir::new("copy-copies-home");
        let root = Dir::new("copy-copies-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");
        std::fs::write(root.path().join("readme.md"), "old\n").expect("seed the target");

        let copied = write(
            &server,
            Method::POST,
            "copy",
            json!({ "path": "notes.txt", "destination": "copied.txt" }),
        )
        .await;
        assert_eq!(copied.status(), reqwest::StatusCode::NO_CONTENT);

        let overwritten = write(
            &server,
            Method::POST,
            "copy",
            json!({ "path": "notes.txt", "destination": "readme.md" }),
        )
        .await;
        assert_eq!(overwritten.status(), reqwest::StatusCode::NO_CONTENT);

        for name in ["copied.txt", "readme.md"] {
            let content =
                std::fs::read_to_string(root.path().join(name)).expect("read the target");
            assert_eq!(content, "hello\n", "{name}");
        }
    }

    #[tokio::test]
    async fn directory() {
        let home = Dir::new("copy-directory-home");
        let root = Dir::new("copy-directory-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::create_dir_all(root.path().join("tree/nested")).expect("seed the tree");
        std::fs::write(root.path().join("tree/nested/deep.txt"), "deep")
            .expect("seed the file");

        let response = write(
            &server,
            Method::POST,
            "copy",
            json!({ "path": "tree", "destination": "copied" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        let copied = std::fs::read_to_string(root.path().join("copied/nested/deep.txt"))
            .expect("read the copy");
        assert_eq!(copied, "deep", "the subtree lands under the destination");
    }
}

mod r#move {
    use super::*;

    #[tokio::test]
    async fn renames() {
        let home = Dir::new("move-renames-home");
        let root = Dir::new("move-renames-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = write(
            &server,
            Method::POST,
            "move",
            json!({ "path": "notes.txt", "destination": "moved.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert!(!root.path().join("notes.txt").exists(), "the source is gone");
        let content =
            std::fs::read_to_string(root.path().join("moved.txt")).expect("read the target");
        assert_eq!(content, "hello\n");
    }
}

mod commit {
    use super::*;

    #[tokio::test]
    async fn places() {
        let home = Dir::new("commit-places-home");
        let root = Dir::new("commit-places-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let bytes = b"\x00\xff binary\n";
        let upload = server.upload(bytes).await;

        let response = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": upload.id, "path": "placed.bin" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        let placed = std::fs::read(root.path().join("placed.bin")).expect("read the placed file");
        assert_eq!(placed, bytes);
    }

    #[tokio::test]
    async fn consumes() {
        let home = Dir::new("commit-consumes-home");
        let root = Dir::new("commit-consumes-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let upload = server.upload(b"payload").await;

        let committed = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": upload.id, "path": "placed.bin" }),
        )
        .await;
        assert_eq!(committed.status(), reqwest::StatusCode::NO_CONTENT);

        assert_eq!(
            upload.head().await.status(),
            reqwest::StatusCode::NOT_FOUND,
            "the upload is terminated"
        );
        let again = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": upload.id, "path": "again.bin" }),
        )
        .await;
        assert_eq!(
            again.status(),
            reqwest::StatusCode::NOT_FOUND,
            "an upload is placed once"
        );
        assert!(
            !root.path().join("again.bin").exists(),
            "the second commit places nothing"
        );
    }

    #[tokio::test]
    async fn incomplete() {
        let home = Dir::new("commit-incomplete-home");
        let root = Dir::new("commit-incomplete-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let upload = Upload::open(&server, 8).await;

        let refused = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": upload.id, "path": "placed.bin" }),
        )
        .await;
        assert_eq!(refused.status(), reqwest::StatusCode::CONFLICT);
        assert!(!root.path().join("placed.bin").exists(), "nothing is placed");

        // The refusal leaves the upload alone, so its remaining bytes still go in.
        assert_eq!(
            upload.patch(0, b"abcd").await.status(),
            reqwest::StatusCode::NO_CONTENT
        );
        assert_eq!(
            upload.patch(4, b"efgh").await.status(),
            reqwest::StatusCode::NO_CONTENT
        );

        let committed = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": upload.id, "path": "placed.bin" }),
        )
        .await;
        assert_eq!(committed.status(), reqwest::StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("commit-unknown-home");
        let root = Dir::new("commit-unknown-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        let response = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": "missing", "path": "placed.bin" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn overwrites() {
        let home = Dir::new("commit-overwrites-home");
        let root = Dir::new("commit-overwrites-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "old\n").expect("seed the target");
        let bytes = b"\x00\xff binary\n";
        let upload = server.upload(bytes).await;

        let response = write(
            &server,
            Method::POST,
            "commit",
            json!({ "upload": upload.id, "path": "notes.txt" }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        let placed = std::fs::read(root.path().join("notes.txt")).expect("read the target");
        assert_eq!(placed, bytes);
    }
}

mod delete {
    use super::*;

    #[tokio::test]
    async fn removes() {
        let home = Dir::new("delete-removes-home");
        let root = Dir::new("delete-removes-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), "hello\n").expect("seed the file");

        let response = delete(
            &server,
            json!({ "path": "notes.txt", "recursive": null, "force": null }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert!(!root.path().join("notes.txt").exists(), "the file is gone");
    }

    #[tokio::test]
    async fn recursive() {
        let home = Dir::new("delete-recursive-home");
        let root = Dir::new("delete-recursive-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::create_dir_all(root.path().join("tree/nested")).expect("seed the tree");
        std::fs::write(root.path().join("tree/nested/deep.txt"), "deep").expect("seed the file");

        let response =
            delete(&server, json!({ "path": "tree", "recursive": true, "force": null })).await;

        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        assert!(!root.path().join("tree").exists(), "the tree is gone");
    }

    #[tokio::test]
    async fn non_empty() {
        let home = Dir::new("delete-non-empty-home");
        let root = Dir::new("delete-non-empty-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::create_dir(root.path().join("tree")).expect("seed the directory");
        std::fs::write(root.path().join("tree/deep.txt"), "deep").expect("seed the file");

        let response =
            delete(&server, json!({ "path": "tree", "recursive": null, "force": null })).await;

        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        assert!(
            root.path().join("tree/deep.txt").exists(),
            "the tree is left alone"
        );
    }
}
