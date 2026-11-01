//! Embedded Neovim sessions. Each session is a real `nvim` process running in a
//! Windows ConPTY. Output is streamed to the frontend as base64 `nvim-output`
//! events, and the process exit is reported as `nvim-exit`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

const NVIM_DEFAULT_PATH: &str = r"C:\Program Files\Neovim\bin\nvim.exe";
pub const MAX_OPEN_BYTES: u64 = 500 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;
const BLOCKED_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "bin", "obj", "o", "lib", "pdb", "class", "jar", "wasm",
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "zip", "gz", "tar", "7z", "rar",
    "pdf", "mp3", "mp4", "wav", "mov", "avi", "ttf", "otf", "woff", "woff2",
];

struct NvimSession {
    pipe: String,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
}

#[derive(Default)]
pub struct NvimSessions {
    sessions: Arc<Mutex<HashMap<String, Arc<NvimSession>>>>,
}

impl NvimSessions {
    fn get(&self, id: &str) -> Result<Arc<NvimSession>, String> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no nvim session '{id}'"))
    }
}

#[derive(Clone, Serialize)]
struct OutputPayload {
    id: String,
    data: String,
}

#[derive(Clone, Serialize)]
struct ExitPayload {
    id: String,
    code: Option<u32>,
}

fn nvim_program() -> PathBuf {
    let bundled = PathBuf::from(NVIM_DEFAULT_PATH);
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from("nvim")
    }
}

/// Runs a short-lived helper process (e.g. `nvim --server`) without a console window.
fn helper_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Rejects directories, oversized files, known binary extensions, and files containing NUL bytes.
pub fn check_openable(path: &Path) -> Result<(), String> {
    let name = path.display();
    let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    if meta.len() > MAX_OPEN_BYTES {
        return Err(format!(
            "{name} is {} KB; files over {} KB are not opened in the editor",
            meta.len() / 1024,
            MAX_OPEN_BYTES / 1024
        ));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if BLOCKED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("{name} looks like a binary file (.{ext}) and is not opened"));
    }
    let mut head = vec![0u8; BINARY_SNIFF_BYTES];
    let read = File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| format!("{name}: {e}"))?;
    if head[..read].contains(&0) {
        return Err(format!("{name} contains NUL bytes and looks binary; not opened"));
    }
    Ok(())
}

#[tauri::command]
pub fn nvim_open(
    app: AppHandle,
    sessions: State<'_, NvimSessions>,
    id: String,
    file: Option<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    if sessions.sessions.lock().unwrap().contains_key(&id) {
        return Err(format!("nvim session '{id}' already exists"));
    }
    let work_dir = match &file {
        Some(f) => {
            let path = Path::new(f);
            check_openable(path)?;
            path.parent().map(Path::to_path_buf)
        }
        None => cwd.map(PathBuf::from),
    };

    let pipe = format!(r"\\.\pipe\mothership-{}-{}", std::process::id(), id);

    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("failed to open ConPTY: {e}"))?;

    let mut cmd = CommandBuilder::new(nvim_program());
    cmd.arg("--listen");
    cmd.arg(&pipe);
    if let Some(f) = &file {
        cmd.arg(f);
    }
    if let Some(dir) = &work_dir {
        cmd.cwd(dir);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to start nvim: {e}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to read nvim output: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("failed to write to nvim: {e}"))?;

    let session = Arc::new(NvimSession {
        pipe,
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
    });
    sessions.sessions.lock().unwrap().insert(id.clone(), session);

    let map = sessions.sessions.clone();
    spawn_reader(app, map, id.clone(), reader, child);
    Ok(id)
}

fn spawn_reader(
    app: AppHandle,
    map: Arc<Mutex<HashMap<String, Arc<NvimSession>>>>,
    id: String,
    mut reader: Box<dyn Read + Send>,
    child: Box<dyn Child + Send + Sync>,
) {
    // Exit is detected from the process itself. On Windows the ConPTY output pipe can
    // stay open after nvim has exited, so EOF on `reader` is not a reliable exit signal.
    let child = Arc::new(Mutex::new(child));
    {
        let app = app.clone();
        let map = map.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            let code = loop {
                let status = child.lock().unwrap().try_wait();
                match status {
                    Ok(Some(s)) => break Some(s.exit_code()),
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => break None,
                }
            };
            map.lock().unwrap().remove(&id);
            let _ = app.emit("nvim-exit", ExitPayload { id, code });
        });
    }
    std::thread::spawn(move || {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = app.emit(
                        "nvim-output",
                        OutputPayload { id: id.clone(), data: engine.encode(&buf[..n]) },
                    );
                }
            }
        }
    });
}

#[tauri::command]
pub fn nvim_write(sessions: State<'_, NvimSessions>, id: String, data: String) -> Result<(), String> {
    let session = sessions.get(&id)?;
    let mut writer = session.writer.lock().unwrap();
    writer
        .write_all(data.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|e| format!("write to nvim failed: {e}"))
}

#[tauri::command]
pub fn nvim_resize(
    sessions: State<'_, NvimSessions>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let session = sessions.get(&id)?;
    let master = session.master.lock().unwrap();
    master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("resize failed: {e}"))
}

/// Counts loaded buffers with unsaved changes. nvim writes the count to a temp file
/// because `--remote-expr` stdout is not reliable on Windows.
#[tauri::command]
pub async fn nvim_unsaved_buffers(sessions: State<'_, NvimSessions>, id: String) -> Result<u32, String> {
    let pipe = sessions.get(&id)?.pipe.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let out_path = std::env::temp_dir().join(format!(
            "mothership-nvim-{}-{}-modified.txt",
            std::process::id(),
            id
        ));
        let expr = format!(
            "writefile([string(len(filter(getbufinfo(), 'v:val.changed')))], '{}')",
            out_path.display()
        );
        let status = helper_command(&nvim_program())
            .args(["--server", &pipe, "--remote-expr", &expr])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("could not query nvim: {e}"))?;
        let result = if status.success() {
            std::fs::read_to_string(&out_path)
                .map_err(|e| format!("could not read nvim reply: {e}"))
                .and_then(|s| {
                    s.trim()
                        .parse::<u32>()
                        .map_err(|e| format!("unexpected nvim reply: {e}"))
                })
        } else {
            Err("nvim did not answer the query".to_string())
        };
        let _ = std::fs::remove_file(&out_path);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Asks nvim to quit (`:qa`, or `:qa!` when `force` is set). Returns `true` once the
/// session has ended, or `false` if nvim is still running after a short wait
/// (for example because of unsaved changes). The process is never killed here.
#[tauri::command]
pub async fn nvim_close(
    sessions: State<'_, NvimSessions>,
    id: String,
    force: bool,
) -> Result<bool, String> {
    let session = match sessions.get(&id) {
        Ok(s) => s,
        Err(_) => return Ok(true),
    };
    let pipe = session.pipe.clone();
    let map = sessions.sessions.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let keys = if force { r"<C-\><C-n>:qa!<CR>" } else { r"<C-\><C-n>:qa<CR>" };
        let _ = helper_command(&nvim_program())
            .args(["--server", &pipe, "--remote-send", keys])
            .stdin(std::process::Stdio::null())
            .output();
        for _ in 0..40 {
            if !map.lock().unwrap().contains_key(&id) {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(false)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn nvim_check_file(path: String) -> Result<(), String> {
    check_openable(Path::new(&path))
}
