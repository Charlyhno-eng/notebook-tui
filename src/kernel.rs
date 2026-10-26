use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};
pub struct Kernel {
    child: Child,
    stdin: ChildStdin,
    pub events: Receiver<Value>,
}
impl Kernel {
    pub fn start(python: &Path, cwd: &Path) -> Result<Self> {
        let mut child = Command::new(python)
            .args(["-u", "-c", include_str!("../python/bridge.py")])
            .arg(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Could not start {}", python.display()))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, events) = mpsc::channel();
        let errors = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str(&line) {
                    let _ = tx.send(v);
                }
            }
            let _ = tx.send(serde_json::json!({"event":"disconnected"}));
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = errors.send(serde_json::json!({"event":"diagnostic","message":line}));
            }
        });
        Ok(Self {
            child,
            stdin,
            events,
        })
    }
    /// A command sink for app lifecycle tests; this does not simulate Python execution.
    #[cfg(test)]
    pub fn test_channel() -> (Self, mpsc::Sender<Value>) {
        let mut child = Command::new("sh")
            .args([
                "-c",
                r#"while IFS= read -r line; do [ "$line" = '{"cmd":"shutdown"}' ] && exit 0; done"#,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let (sender, events) = mpsc::channel();
        (
            Self {
                child,
                stdin,
                events,
            },
            sender,
        )
    }
    pub fn send(&mut self, value: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &value)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}
impl Drop for Kernel {
    fn drop(&mut self) {
        let _ = self.send(serde_json::json!({"cmd":"shutdown"}));
        for _ in 0..30 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub fn environment(path: &Path, explicit: Option<PathBuf>) -> (PathBuf, PathBuf) {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(Path::new("."))
    };
    let root = dir
        .ancestors()
        .find(|p| p.join(".venv/bin/python").exists() || p.join("pyproject.toml").exists())
        .unwrap_or(dir)
        .to_owned();
    let python = explicit.unwrap_or_else(|| {
        let p = root.join(".venv/bin/python");
        if p.exists() {
            p
        } else if let Some(v) = std::env::var_os("VIRTUAL_ENV") {
            PathBuf::from(v).join("bin/python")
        } else {
            PathBuf::from("python3")
        }
    });
    (python, root)
}
