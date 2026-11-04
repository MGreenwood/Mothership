//! Profiling fixtures for the CLI. The tests are `#[ignore]`d so they stay out of normal runs:
//!
//! cargo test -p mothership-cli profile_ --release -- --ignored --nocapture

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Temp directory that is removed when dropped.
pub(crate) struct TempTree(pub PathBuf);

impl TempTree {
    pub(crate) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("mothership-profile-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates `project_count` projects under `root`. Each one has a `.mothership/project.json`
/// and a `node_modules` directory with `dep_packages` package directories, like a real workspace.
/// Returns the project names.
pub(crate) fn build_project_tree(root: &Path, project_count: usize, dep_packages: usize) -> Vec<String> {
    let mut names = Vec::with_capacity(project_count);
    for i in 0..project_count {
        let name = format!("project-{i:03}");
        let dir = root.join(&name);
        let meta_dir = dir.join(".mothership");
        std::fs::create_dir_all(&meta_dir).unwrap();
        let meta = serde_json::json!({
            "project_id": uuid::Uuid::new_v4().to_string(),
            "project_name": name,
            "created_at": "2026-01-01T00:00:00Z",
            "mothership_url": "http://localhost",
        });
        std::fs::write(meta_dir.join("project.json"), meta.to_string()).unwrap();
        for p in 0..dep_packages {
            let pkg = dir.join("node_modules").join(format!("pkg-{p:04}"));
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(pkg.join("index.js"), "module.exports = {};").unwrap();
        }
        names.push(name);
    }
    names
}

/// Number of entries a depth-limited walk visits under `root`.
pub(crate) fn walk_entry_count(root: &Path, max_depth: usize) -> usize {
    walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .count()
}

/// Local HTTP/1.1 server that answers every request on a keep-alive connection.
pub(crate) async fn spawn_keepalive_server() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok";
                    if socket.write_all(response).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test]
#[ignore = "profiling: run with --ignored --nocapture"]
async fn profile_http_client_reuse() {
    const CALLS: usize = 200;
    let addr = spawn_keepalive_server().await;
    let url = format!("http://{addr}/ping");

    // Current pattern: a new client (and connection pool) for every API call.
    let start = Instant::now();
    for _ in 0..CALLS {
        reqwest::Client::new().get(&url).send().await.unwrap().bytes().await.unwrap();
    }
    let fresh = start.elapsed();

    // Proposed pattern: one shared client that reuses pooled connections.
    let shared = crate::http_client();
    let start = Instant::now();
    for _ in 0..CALLS {
        shared.get(&url).send().await.unwrap().bytes().await.unwrap();
    }
    let reused = start.elapsed();

    eprintln!(
        "[profile] http client: {CALLS} calls, fresh client per call = {:?} ({:?}/call), shared client = {:?} ({:?}/call)",
        fresh,
        fresh / CALLS as u32,
        reused,
        reused / CALLS as u32,
    );
}
