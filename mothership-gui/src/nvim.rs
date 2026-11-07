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
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

const NVIM_DEFAULT_PATH: &str = r"C:\Program Files\Neovim\bin\nvim.exe";
const SHELL_PROGRAM: &str = "powershell.exe";
pub const MAX_OPEN_BYTES: u64 = 500 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;
const BLOCKED_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "bin", "obj", "o", "lib", "pdb", "class", "jar", "wasm",
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "zip", "gz", "tar", "7z", "rar",
    "pdf", "mp3", "mp4", "wav", "mov", "avi", "ttf", "otf", "woff", "woff2",
];

struct NvimSession {
    /// Empty for plain shell sessions, which have no Neovim RPC pipe.
    pipe: String,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
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

/// Starts `cmd` in a new ConPTY and streams its output as `nvim-output` events.
fn start_session(
    app: AppHandle,
    sessions: &NvimSessions,
    id: String,
    cmd: CommandBuilder,
    pipe: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    if sessions.sessions.lock().unwrap().contains_key(&id) {
        return Err(format!("session '{id}' already exists"));
    }

    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("failed to open ConPTY: {e}"))?;

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to start process: {e}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to read process output: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("failed to write to process: {e}"))?;
    let killer = child.clone_killer();

    let session = Arc::new(NvimSession {
        pipe,
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
        killer: Mutex::new(killer),
    });
    sessions.sessions.lock().unwrap().insert(id.clone(), session);

    spawn_reader(app, sessions.sessions.clone(), id, reader, child);
    Ok(())
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
    line: Option<u32>,
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

    let mut cmd = CommandBuilder::new(nvim_program());
    cmd.arg("--listen");
    cmd.arg(&pipe);
    cmd.arg("--cmd");
    cmd.arg("set autoread backupcopy=yes noswapfile");
    if let Some(jump) = line_arg(line) {
        cmd.arg(jump);
    }
    if let Some(f) = &file {
        cmd.arg(f);
    }
    if let Some(dir) = &work_dir {
        cmd.cwd(dir);
    }

    start_session(app, &sessions, id.clone(), cmd, pipe, cols, rows)?;
    Ok(id)
}

/// The Neovim argument that opens a file at a line (`+136`). Lines start at 1, so anything else means no jump.
fn line_arg(line: Option<u32>) -> Option<String> {
    line.filter(|l| *l >= 1).map(|l| format!("+{l}"))
}

/// Moves a running session's cursor to a line, for a file that is already open in a tab
#[tauri::command]
pub async fn nvim_goto(sessions: State<'_, NvimSessions>, id: String, line: u32) -> Result<(), String> {
    let pipe = sessions
        .sessions
        .lock()
        .unwrap()
        .get(&id)
        .map(|s| s.pipe.clone())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("nvim session '{id}' is not running"))?;
    if line < 1 {
        return Err("line numbers start at 1".to_string());
    }
    let keys = format!(r"<C-\><C-N>:{line}<CR>zz");
    let program = nvim_program();
    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = Command::new(&program);
        cmd.args(["--server", &pipe, "--remote-send", &keys]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match cmd.output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod line_arg_tests {
    use super::line_arg;

    #[test]
    fn jumps_only_for_real_line_numbers() {
        assert_eq!(line_arg(Some(136)), Some("+136".to_string()));
        assert_eq!(line_arg(Some(1)), Some("+1".to_string()));
        assert_eq!(line_arg(Some(0)), None);
        assert_eq!(line_arg(None), None);
    }
}

/// Opens an interactive PowerShell in `cwd` (or the user profile when omitted).
#[tauri::command]
pub fn terminal_open(
    app: AppHandle,
    sessions: State<'_, NvimSessions>,
    id: String,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let work_dir = match cwd {
        Some(dir) => {
            let path = PathBuf::from(&dir);
            if !path.is_dir() {
                return Err(format!("{dir} is not a folder"));
            }
            Some(path)
        }
        None => None,
    };

    let mut cmd = CommandBuilder::new(SHELL_PROGRAM);
    cmd.arg("-NoLogo");
    if let Some(dir) = &work_dir {
        cmd.cwd(dir);
    }

    start_session(app, &sessions, id.clone(), cmd, String::new(), cols, rows)?;
    Ok(id)
}

/// Kills a shell started by `terminal_open`. Neovim sessions are refused here
/// because they must be closed through `nvim_close` so unsaved changes are checked.
#[tauri::command]
pub fn terminal_close(sessions: State<'_, NvimSessions>, id: String) -> Result<(), String> {
    let mut map = sessions.sessions.lock().unwrap();
    let Some(session) = map.get(&id).cloned() else {
        return Ok(());
    };
    if !session.pipe.is_empty() {
        return Err(format!("session '{id}' is a Neovim session, not a terminal"));
    }
    map.remove(&id);
    drop(map);
    let _ = session.killer.lock().unwrap().kill();
    Ok(())
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

/// Quits nvim by typing `:qa` (or `:qa!` when `force` is set) into its terminal.
/// Returns `true` once the session has ended. Without `force`, nvim refuses to quit
/// while buffers have unsaved changes, so `false` means the caller should ask before
/// discarding. The process is never killed here.
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
    // Ctrl-\ Ctrl-N switches to normal mode before the quit command.
    let keys = if force { "\x1c\x0e:qa!\r" } else { "\x1c\x0e:qa\r" };
    {
        let mut writer = session.writer.lock().unwrap();
        writer
            .write_all(keys.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|e| format!("write to nvim failed: {e}"))?;
    }
    let map = sessions.sessions.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for _ in 0..40 {
            if !map.lock().unwrap().contains_key(&id) {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Ok(false)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Normal mode, then enable `autoread` and `backupcopy=yes` so unmodified buffers reload silently.
/// `checktime` re-reads files changed on disk. Buffers with unsaved edits keep them,
/// and Neovim reports the conflict instead of overwriting.
const RELOAD_KEYS: &str = r"<C-\><C-N>:set autoread backupcopy=yes<CR>:silent! checktime<CR>";

/// Asks every open Neovim session to re-read files changed on disk, such as after a
/// rift switch. Keys go through each session's RPC pipe, so the user's terminal input
/// and cursor are left alone.
#[tauri::command]
pub async fn nvim_reload_files(sessions: State<'_, NvimSessions>) -> Result<(), String> {
    let pipes: Vec<String> = sessions
        .sessions
        .lock()
        .unwrap()
        .values()
        .filter(|s| !s.pipe.is_empty())
        .map(|s| s.pipe.clone())
        .collect();
    if pipes.is_empty() {
        return Ok(());
    }
    let program = nvim_program();
    tauri::async_runtime::spawn_blocking(move || {
        let mut failures = Vec::new();
        for pipe in pipes {
            let mut cmd = Command::new(&program);
            cmd.args(["--server", &pipe, "--remote-send", RELOAD_KEYS]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd.output() {
                Ok(out) if out.status.success() => {}
                Ok(out) => failures.push(String::from_utf8_lossy(&out.stderr).trim().to_string()),
                Err(e) => failures.push(e.to_string()),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!("could not refresh {} editor session(s): {}", failures.len(), failures.join("; ")))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn nvim_check_file(path: String) -> Result<(), String> {
    check_openable(Path::new(&path))
}
