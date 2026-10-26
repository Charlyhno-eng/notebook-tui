mod editor;
mod execution;
mod images;
mod kernel;
mod markdown;
mod notebook;
mod ui;
use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyModifiers, MouseEventKind,
    },
    execute,
};
use execution::{Execution, State};
use notebook::{Notebook, new_cell, set_source, source};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    path::PathBuf,
    time::Duration,
};

#[derive(Parser)]
#[command(version, about = "Jupyter notebooks in your terminal — Rust + Ratatui")]
struct Args {
    /// Notebook .ipynb file or directory to browse
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Python interpreter with jupyter_client and ipykernel installed
    #[arg(long)]
    python: Option<PathBuf>,
    /// Validate notebooks without opening the UI or executing code
    #[arg(long)]
    check: bool,
}
#[derive(Clone)]
pub enum Action {
    Run,
    RunAll,
    Save,
    Interrupt,
    Restart,
    Help,
    Open(usize),
    Select(usize),
    Edit(usize),
    Preview,
    Quit,
}
#[derive(Clone, Copy)]
pub enum Confirm {
    Quit,
    Open(usize),
    Reload,
    Delete,
    Restart,
}
pub struct App {
    pub images: images::Images,
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub file_index: usize,
    pub notebook: Option<Notebook>,
    pub selected: usize,
    pub sidebar: bool,
    pub focus_files: bool,
    pub editor: Option<editor::Editor>,
    pub scroll: usize,
    pub follow: bool,
    pub status: String,
    pub kernel_status: String,
    pub ready: bool,
    pub running: Option<usize>,
    pub queue: VecDeque<usize>,
    pub kernel: Option<kernel::Kernel>,
    pub python: Option<PathBuf>,
    pub help: bool,
    pub confirm: Option<Confirm>,
    pub input: Option<(String, bool, String)>,
    pub quit: bool,
    pub hits: Vec<(ratatui::layout::Rect, Action)>,
    pub cell_positions: Vec<usize>,
    pub displays: HashMap<String, Vec<(usize, usize)>>,
    pub failed: bool,
    pub interrupted: bool,
    pub executions: Vec<Execution>,
}
impl App {
    fn new(args: Args) -> Result<Self> {
        let path = args
            .path
            .canonicalize()
            .with_context(|| format!("Path not found: {}", args.path.display()))?;
        let root = if path.is_dir() {
            path.clone()
        } else {
            path.parent().unwrap().to_owned()
        };
        let mut files = notebook::discover(&root);
        if path.is_file() && !files.contains(&path) {
            files.push(path.clone());
        }
        let index = files.iter().position(|p| *p == path).unwrap_or(0);
        let mut app = Self {
            images: images::Images::default(),
            root,
            files,
            file_index: index,
            notebook: None,
            selected: 0,
            sidebar: path.is_dir(),
            focus_files: false,
            editor: None,
            scroll: 0,
            follow: true,
            status: "Enter: edit • F5: run • Ctrl+B: files • ?: help".into(),
            kernel_status: "Not started".into(),
            ready: false,
            running: None,
            queue: VecDeque::new(),
            kernel: None,
            python: args.python,
            help: false,
            confirm: None,
            input: None,
            quit: false,
            hits: Vec::new(),
            cell_positions: Vec::new(),
            displays: HashMap::new(),
            failed: false,
            interrupted: false,
            executions: Vec::new(),
        };
        if !app.files.is_empty() {
            app.open(index)?;
        } else {
            app.status = "No notebooks found. Press n to create one.".into();
            app.focus_files = true;
        }
        Ok(app)
    }
    fn open(&mut self, index: usize) -> Result<()> {
        let next = Notebook::load(&self.files[index])?;
        self.kernel = None;
        self.ready = false;
        self.running = None;
        self.queue.clear();
        self.displays.clear();
        self.executions = next
            .cells()
            .iter()
            .map(|cell| Execution {
                state: if cell["cell_type"] == "code"
                    && (!cell["execution_count"].is_null()
                        || cell["outputs"].as_array().is_some_and(|o| !o.is_empty()))
                {
                    State::Previous
                } else {
                    State::Idle
                },
                ..Execution::default()
            })
            .collect();
        self.interrupted = false;
        self.input = None;
        self.notebook = Some(next);
        self.selected = 0;
        self.file_index = index;
        self.scroll = 0;
        self.follow = true;
        self.focus_files = false;
        self.editor = None;
        self.kernel_status = "Not started".into();
        self.status = "Notebook opened — press F5 to run".into();
        Ok(())
    }
    fn dirty(&self) -> bool {
        self.notebook.as_ref().is_some_and(|n| n.dirty)
    }
    fn commit_editor(&mut self) {
        if let (Some(e), Some(n)) = (&self.editor, &mut self.notebook) {
            let text = e.text();
            if source(&n.cells()[self.selected]) != text {
                set_source(&mut n.cells_mut()[self.selected], &text);
                self.executions[self.selected].edited = true;
            }
        }
    }
    fn save(&mut self) -> Result<()> {
        self.commit_editor();
        if let Some(n) = &mut self.notebook {
            n.save()?;
            self.status = "Notebook saved".into();
        }
        Ok(())
    }
    fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }
    fn clear_queue(&mut self) {
        self.queue.clear();
        for execution in &mut self.executions {
            execution.queued = false;
        }
    }
    fn finish_running(&mut self, state: State) {
        if let Some(index) = self.running.take() {
            self.executions[index].finish(state);
        }
        self.input = None;
    }
    fn kernel_failed(&mut self, message: &str) {
        self.finish_running(State::Failed);
        self.clear_queue();
        self.ready = false;
        self.kernel_status = "Stopped".into();
        self.status = format!("Kernel: {message}");
        self.kernel = None;
    }
    fn ensure_kernel(&mut self) -> Result<()> {
        if self.kernel.is_none() {
            let n = self.notebook.as_ref().context("No notebook")?;
            let (python, cwd) = kernel::environment(&n.path, self.python.clone());
            self.kernel = Some(kernel::Kernel::start(&python, &cwd)?);
            self.kernel_status = "Starting…".into();
            self.ready = false;
            self.status = format!(
                "Python: {} • directory: {}",
                python.display(),
                cwd.display()
            );
        }
        Ok(())
    }
    fn run(&mut self, all: bool) -> Result<()> {
        self.commit_editor();
        self.editor = None;
        if self.busy() {
            self.status = "Execution already in progress".into();
            return Ok(());
        }
        let Some(n) = &self.notebook else {
            return Ok(());
        };
        self.queue = if all {
            n.cells()
                .iter()
                .enumerate()
                .filter(|(_, c)| c["cell_type"] == "code")
                .map(|(i, _)| i)
                .collect()
        } else if n
            .cells()
            .get(self.selected)
            .is_some_and(|c| c["cell_type"] == "code")
        {
            VecDeque::from([self.selected])
        } else {
            VecDeque::new()
        };
        if self.queue.is_empty() {
            self.status = "No code cells to run".into();
            return Ok(());
        }
        if let Err(e) = self.ensure_kernel() {
            self.queue.clear();
            return Err(e);
        }
        self.interrupted = false;
        for &index in &self.queue {
            self.executions[index].queued = true;
        }
        self.next()
    }
    fn next(&mut self) -> Result<()> {
        if !self.ready || self.running.is_some() {
            return Ok(());
        }
        if let Some(i) = self.queue.pop_front() {
            let code = source(&self.notebook.as_ref().unwrap().cells()[i]);
            if let Err(error) = self
                .kernel
                .as_mut()
                .context("No kernel")?
                .send(json!({"cmd":"execute","cell":i,"code":code}))
            {
                self.executions[i].queued = false;
                self.executions[i].state = State::Failed;
                self.kernel_failed(&format!("{error:#}"));
                return Err(error);
            }
            let n = self.notebook.as_mut().unwrap();
            n.cells_mut()[i]["outputs"] = json!([]);
            n.cells_mut()[i]["execution_count"] = Value::Null;
            for entries in self.displays.values_mut() {
                entries.retain(|(cell, _)| *cell != i);
            }
            self.running = Some(i);
            self.executions[i].start();
            self.failed = false;
            self.interrupted = false;
            self.kernel_status = "Busy".into();
            self.status = format!("Running cell {} • {} queued", i + 1, self.queue.len());
        }
        Ok(())
    }
    fn poll_kernel(&mut self) -> Result<()> {
        let events: Vec<Value> = self
            .kernel
            .as_ref()
            .map(|k| k.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            self.handle_kernel_event(event);
            if self.kernel.is_none() {
                break;
            }
        }
        self.next()
    }
    fn handle_kernel_event(&mut self, event: Value) {
        let kind = event["event"].as_str().unwrap_or("");
        let index = event["cell"].as_u64().unwrap_or(0) as usize;
        if matches!(kind, "started" | "count" | "clear" | "output" | "done")
            && self.running != Some(index)
        {
            return;
        }
        match kind {
            "ready" => {
                self.ready = true;
                self.running = None;
                self.kernel_status = "Ready".into();
                self.status = format!(
                    "Kernel ready: {}",
                    event["python"].as_str().unwrap_or("Python")
                );
            }
            "fatal" | "disconnected" => {
                self.kernel_failed(
                    event["message"]
                        .as_str()
                        .unwrap_or("Kernel connection closed"),
                );
            }
            "diagnostic" => {
                self.status = event["message"].as_str().unwrap_or("").to_owned();
            }
            "input" => {
                if self.running.is_none() {
                    return;
                }
                self.input = Some((
                    event["prompt"].as_str().unwrap_or("Input: ").into(),
                    event["password"].as_bool().unwrap_or(false),
                    String::new(),
                ))
            }
            "count" => {
                if let Some(n) = &mut self.notebook {
                    n.cells_mut()[index]["execution_count"] = event["count"].clone();
                }
            }
            "clear" => {
                if let Some(n) = &mut self.notebook {
                    n.cells_mut()[index]["outputs"] = json!([]);
                }
                for entries in self.displays.values_mut() {
                    entries.retain(|(cell, _)| *cell != index);
                }
            }
            "output" => {
                if event["output"]["output_type"] == "error" {
                    self.failed = true;
                    if event["output"]["ename"] == "KeyboardInterrupt" {
                        self.interrupted = true;
                    }
                }
                if let Some(n) = &mut self.notebook {
                    let outputs = n.cells_mut()[index]["outputs"].as_array_mut().unwrap();
                    let output = event["output"].clone();
                    let merge = output["output_type"] == "stream"
                        && outputs.last().is_some_and(|o| {
                            o["output_type"] == "stream" && o["name"] == output["name"]
                        });
                    if merge {
                        let last = outputs.last_mut().unwrap();
                        last["text"] = json!(format!(
                            "{}{}",
                            notebook::multiline(&last["text"]),
                            notebook::multiline(&output["text"])
                        ));
                    } else {
                        outputs.push(output);
                    }
                    if let Some(id) = event["display_id"].as_str() {
                        self.displays
                            .entry(id.into())
                            .or_default()
                            .push((index, outputs.len() - 1));
                    }
                }
            }
            "update" => {
                if let (Some(n), Some(id)) = (&mut self.notebook, event["display_id"].as_str())
                    && let Some(entries) = self.displays.get(id)
                {
                    for &(cell, output) in entries {
                        if cell == index
                            && let Some(o) = n.cells_mut()[cell]["outputs"].get_mut(output)
                        {
                            o["data"] = event["data"].clone();
                            o["metadata"] = event["metadata"].clone();
                        }
                    }
                }
            }
            "done" => {
                let outcome = if self.interrupted {
                    State::Interrupted
                } else if self.failed {
                    State::Failed
                } else {
                    State::Succeeded
                };
                self.finish_running(outcome);
                self.kernel_status = "Ready".into();
                if self.interrupted {
                    self.clear_queue();
                    self.status = "Execution interrupted".into();
                } else if self.failed {
                    self.clear_queue();
                    self.status = "Python error — execution stopped; see output".into();
                } else if self.queue.is_empty() {
                    self.status = "Execution finished".into();
                }
            }
            _ => {}
        }
    }
    fn select(&mut self, i: usize) {
        self.commit_editor();
        self.editor = None;
        if let Some(n) = &self.notebook {
            self.selected = i.min(n.cells().len().saturating_sub(1));
        }
        self.follow = true;
        self.focus_files = false;
    }
    fn edit(&mut self) {
        if let Some(cell) = self
            .notebook
            .as_ref()
            .and_then(|n| n.cells().get(self.selected))
        {
            self.editor = Some(editor::Editor::new(&source(cell)));
            self.follow = true;
            self.focus_files = false;
        }
    }
    fn request(&mut self, c: Confirm) -> Result<()> {
        self.commit_editor();
        if matches!(c, Confirm::Restart) && self.kernel.is_some() && !self.ready {
            self.status = "Wait for the kernel to become ready before restarting".into();
            return Ok(());
        }
        if self.busy() && matches!(c, Confirm::Open(_) | Confirm::Reload | Confirm::Delete) {
            self.status = "Stop execution before this action".into();
            return Ok(());
        }
        if (self.dirty() && matches!(c, Confirm::Quit | Confirm::Open(_) | Confirm::Reload))
            || matches!(c, Confirm::Delete | Confirm::Restart)
        {
            self.confirm = Some(c);
            return Ok(());
        }
        self.confirmed(c)
    }
    fn confirmed(&mut self, c: Confirm) -> Result<()> {
        self.confirm = None;
        match c {
            Confirm::Quit => self.quit = true,
            Confirm::Open(i) => self.open(i)?,
            Confirm::Reload => {
                let path = &self.notebook.as_ref().context("No notebook")?.path;
                let index = self
                    .files
                    .iter()
                    .position(|p| p == path)
                    .context("Notebook not found")?;
                self.open(index)?;
            }
            Confirm::Delete => {
                self.editor = None;
                if let Some(n) = &mut self.notebook
                    && !n.cells().is_empty()
                {
                    n.cells_mut().remove(self.selected);
                    self.executions.remove(self.selected);
                    self.selected = self.selected.min(n.cells().len().saturating_sub(1));
                    self.displays.clear();
                    self.follow = true;
                }
            }
            Confirm::Restart => {
                self.clear_queue();
                self.finish_running(State::Interrupted);
                if let Some(k) = &mut self.kernel {
                    k.send(json!({"cmd":"restart"}))?;
                    self.ready = false;
                    self.running = None;
                    self.kernel_status = "Restarting…".into();
                } else {
                    self.ensure_kernel()?;
                }
            }
        }
        Ok(())
    }
    fn action(&mut self, a: Action) -> Result<()> {
        match a {
            Action::Run => self.run(false)?,
            Action::RunAll => self.run(true)?,
            Action::Save => self.save()?,
            Action::Interrupt => {
                if !self.busy() {
                    return Ok(());
                }
                self.clear_queue();
                if self.running.is_some() {
                    if !self.interrupted {
                        self.kernel
                            .as_mut()
                            .context("No kernel")?
                            .send(json!({"cmd":"interrupt"}))?;
                        self.interrupted = true;
                        self.status = "Interrupt requested — waiting for the kernel".into();
                    }
                } else {
                    self.status = "Queued execution cancelled".into();
                }
            }
            Action::Quit => self.request(Confirm::Quit)?,
            Action::Restart => self.request(Confirm::Restart)?,
            Action::Help => self.help = !self.help,
            Action::Open(i) => self.request(Confirm::Open(i))?,
            Action::Select(i) => self.select(i),
            Action::Edit(i) => {
                self.select(i);
                self.edit();
            }
            Action::Preview => self.preview()?,
        }
        Ok(())
    }
    fn preview(&mut self) -> Result<()> {
        let Some(cell) = self
            .notebook
            .as_ref()
            .and_then(|n| n.cells().get(self.selected))
        else {
            return Ok(());
        };
        if cell["outputs"]
            .as_array()
            .is_none_or(|outputs| outputs.is_empty())
        {
            self.status = "No outputs to preview".into();
            return Ok(());
        }
        let html = output_html(cell);
        let mut file = tempfile::Builder::new()
            .prefix("notebook-output-")
            .suffix(".html")
            .tempfile()?;
        file.write_all(html.as_bytes())?;
        let (_, path) = file.keep()?;
        std::process::Command::new(if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        })
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
        self.status = format!("Outputs opened: {}", path.display());
        Ok(())
    }
    fn insert(&mut self, before: bool) -> Result<()> {
        if self.busy() {
            self.status = "Stop execution before changing notebook structure".into();
            return Ok(());
        }
        self.commit_editor();
        self.editor = None;
        if let Some(n) = &mut self.notebook {
            let i = if n.cells().is_empty() {
                0
            } else if before {
                self.selected
            } else {
                self.selected + 1
            };
            n.cells_mut().insert(i, new_cell("code"));
            self.executions.insert(i, Execution::default());
            self.selected = i;
            self.displays.clear();
            self.follow = true;
        }
        Ok(())
    }
    fn key(&mut self, k: KeyEvent) -> Result<()> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.help {
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1)) {
                self.help = false;
            }
            return Ok(());
        }
        if let Some(c) = self.confirm {
            match k.code {
                KeyCode::Char('y') => self.confirmed(c)?,
                KeyCode::Char('s')
                    if self.dirty() && !matches!(c, Confirm::Delete | Confirm::Restart) =>
                {
                    self.save()?;
                    self.confirmed(c)?;
                }
                KeyCode::Esc | KeyCode::Char('n') => self.confirm = None,
                _ => {}
            }
            return Ok(());
        }
        if ctrl && k.code == KeyCode::Char('q') {
            return self.request(Confirm::Quit);
        }
        if let Some((_, _, text)) = &mut self.input {
            match k.code {
                KeyCode::Enter => {
                    let value = text.clone();
                    self.kernel
                        .as_mut()
                        .unwrap()
                        .send(json!({"cmd":"input","value":value}))?;
                    self.input = None;
                }
                KeyCode::Char('c') if ctrl => {
                    self.input = None;
                    self.action(Action::Interrupt)?;
                }
                KeyCode::Char(c) if !ctrl => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                _ => {}
            }
            return Ok(());
        }
        match k.code {
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Char('q') if ctrl => return self.request(Confirm::Quit),
            KeyCode::Char('c') if ctrl => return self.action(Action::Interrupt),
            KeyCode::Char('l') if ctrl => return self.request(Confirm::Reload),
            KeyCode::Char('b') if ctrl => {
                self.sidebar = !self.sidebar;
                self.focus_files = false;
                return Ok(());
            }
            KeyCode::F(1) => {
                self.help = true;
                return Ok(());
            }
            KeyCode::F(5) => return self.run(k.modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::Enter if ctrl || k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.run(false)?;
                if k.modifiers.contains(KeyModifiers::SHIFT) {
                    self.select(self.selected + 1);
                }
                return Ok(());
            }
            _ => {}
        }
        if let Some(e) = &mut self.editor {
            if k.code == KeyCode::Esc {
                self.commit_editor();
                self.editor = None;
            } else {
                let changed = e.key(k);
                if changed {
                    self.commit_editor();
                }
                self.follow = true;
            }
            return Ok(());
        }
        if self.focus_files {
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.file_index = self.file_index.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.file_index = (self.file_index + 1).min(self.files.len().saturating_sub(1))
                }
                KeyCode::Enter if !self.files.is_empty() => {
                    self.request(Confirm::Open(self.file_index))?
                }
                KeyCode::Tab | KeyCode::Esc => self.focus_files = false,
                KeyCode::Char('q') => self.request(Confirm::Quit)?,
                KeyCode::Char('?') => self.help = true,
                KeyCode::Char('n') => self.create()?,
                _ => {}
            }
            return Ok(());
        }
        match k.code {
            KeyCode::Char('q') => self.request(Confirm::Quit)?,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Tab => {
                self.focus_files = true;
                self.sidebar = true;
            }
            KeyCode::Up | KeyCode::Char('k') => self.select(self.selected.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.select(self.selected + 1),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(usize::MAX),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(12);
                self.follow = false;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(12);
                self.follow = false;
            }
            KeyCode::Enter | KeyCode::Char('i') => self.edit(),
            KeyCode::Char('a') => self.insert(true)?,
            KeyCode::Char('b') => self.insert(false)?,
            KeyCode::Char('d') if !self.busy() => self.request(Confirm::Delete)?,
            KeyCode::Char('m' | 'y' | 'r') if !self.busy() => {
                if let Some(n) = &mut self.notebook
                    && !n.cells().is_empty()
                {
                    let kind = match k.code {
                        KeyCode::Char('m') => "markdown",
                        KeyCode::Char('r') => "raw",
                        _ => "code",
                    };
                    if kind != "markdown" && n.cells()[self.selected].get("attachments").is_some() {
                        bail!("This cell has attachments: keep its Markdown type to preserve them");
                    }
                    let c = &mut n.cells_mut()[self.selected];
                    c["cell_type"] = json!(kind);
                    if kind == "code" {
                        c["outputs"] = json!([]);
                        c["execution_count"] = Value::Null;
                    } else {
                        c.as_object_mut().unwrap().remove("outputs");
                        c.as_object_mut().unwrap().remove("execution_count");
                    }
                    self.executions[self.selected] = Execution::default();
                    self.follow = true;
                }
            }
            KeyCode::Char('J' | 'K') if !self.busy() => {
                if let Some(n) = &mut self.notebook {
                    let target = if k.code == KeyCode::Char('J') {
                        self.selected + 1
                    } else {
                        self.selected.saturating_sub(1)
                    };
                    if target < n.cells().len() {
                        n.cells_mut().swap(self.selected, target);
                        self.executions.swap(self.selected, target);
                        self.selected = target;
                        self.displays.clear();
                        self.follow = true;
                    }
                }
            }
            KeyCode::Char('x') if !self.busy() => {
                if let Some(n) = &mut self.notebook
                    && n.cells()
                        .get(self.selected)
                        .is_some_and(|c| c["cell_type"] == "code")
                {
                    let c = &mut n.cells_mut()[self.selected];
                    c["outputs"] = json!([]);
                    c["execution_count"] = Value::Null;
                    self.executions[self.selected] = Execution::default();
                    self.displays.clear();
                }
            }
            KeyCode::Char('R') => self.request(Confirm::Restart)?,
            KeyCode::Char('o') => self.preview()?,
            KeyCode::Char('n') => self.create()?,
            _ => {}
        }
        Ok(())
    }
    fn create(&mut self) -> Result<()> {
        if self.busy() || self.dirty() {
            self.status =
                "Save the notebook and wait for execution to finish before creating another".into();
            return Ok(());
        }
        let mut i = 1;
        let path = loop {
            let p = self.root.join(format!("Untitled-{i}.ipynb"));
            if !p.exists() {
                break p;
            }
            i += 1;
        };
        let data = json!({"nbformat":4,"nbformat_minor":5,"metadata":{"kernelspec":{"display_name":"Python 3","language":"python","name":"python3"},"language_info":{"name":"python"}},"cells":[new_cell("code")]});
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(serde_json::to_string_pretty(&data)?.as_bytes())?;
        self.files.push(path);
        self.files.sort();
        let i = self
            .files
            .iter()
            .position(|p| p.file_name().unwrap() == format!("Untitled-{i}.ipynb").as_str())
            .unwrap();
        self.open(i)
    }
}
fn output_html(cell: &Value) -> String {
    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\"><title>Notebook outputs</title><style>body{background:#101827;color:#e2e8f0;font:16px monospace;padding:32px}pre{white-space:pre-wrap}table{border-collapse:collapse}td,th{padding:8px;border:1px solid #475569}img{max-width:100%;background:white}</style>",
    );
    if let Some(outputs) = cell["outputs"].as_array() {
        for o in outputs {
            let data = &o["data"];
            if let Some(s) = data["image/png"].as_str() {
                html.push_str(&format!(
                    "<img src=\"data:image/png;base64,{}\">",
                    escape(s)
                ));
            } else if let Some(s) = data["image/jpeg"].as_str() {
                html.push_str(&format!(
                    "<img src=\"data:image/jpeg;base64,{}\">",
                    escape(s)
                ));
            } else if !data["text/html"].is_null() {
                html.push_str(&notebook::multiline(&data["text/html"]));
            } else {
                html.push_str(&format!("<pre>{}</pre>", escape(&ui::output_text(o))));
            }
        }
    }
    html
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.check {
        let files = if args.path.is_file() {
            vec![args.path]
        } else {
            notebook::discover(&args.path)
        };
        if files.is_empty() {
            bail!("No notebooks found");
        }
        for p in &files {
            let n = Notebook::load(p)?;
            println!("OK  {}  ({} cells)", p.display(), n.cells().len());
        }
        println!("{} valid notebooks", files.len());
        return Ok(());
    }
    let mut app = App::new(args)?;
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        while !app.quit {
            if let Err(e) = app.poll_kernel() {
                app.kernel_failed(&format!("{e:#}"));
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(35))? {
                let result = match event::read()? {
                    Event::Key(k) if k.kind != event::KeyEventKind::Release => app.key(k),
                    Event::Paste(text) => {
                        if let Some((_, _, input)) = &mut app.input {
                            input.push_str(&text);
                        } else if let Some(e) = &mut app.editor {
                            e.paste(&text);
                            app.commit_editor();
                            app.follow = true;
                        }
                        Ok(())
                    }
                    Event::Mouse(m)
                        if !app.help && app.confirm.is_none() && app.input.is_none() =>
                    {
                        match m.kind {
                            MouseEventKind::Down(event::MouseButton::Left) => {
                                let hit = app
                                    .hits
                                    .iter()
                                    .rev()
                                    .find(|(r, _)| r.contains((m.column, m.row).into()))
                                    .map(|(_, a)| a.clone());
                                if let Some(a) = hit {
                                    app.action(a)
                                } else {
                                    Ok(())
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                app.scroll = app.scroll.saturating_add(3);
                                app.follow = false;
                                Ok(())
                            }
                            MouseEventKind::ScrollUp => {
                                app.scroll = app.scroll.saturating_sub(3);
                                app.follow = false;
                                Ok(())
                            }
                            _ => Ok(()),
                        }
                    }
                    _ => Ok(()),
                };
                if let Err(e) = result {
                    app.status = format!("Error: {e:#}");
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();
    drop(app);
    result
}

#[cfg(test)]
mod app_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn fixture() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("lesson.ipynb");
        let mut md = new_cell("markdown");
        set_source(&mut md, "# My lesson\n\nA **readable** introduction.");
        let mut code = new_cell("code");
        set_source(&mut code, "print('hello')\nx = 42");
        std::fs::write(&p,serde_json::to_vec(&json!({"nbformat":4,"nbformat_minor":5,"metadata":{"custom":"preserve"},"cells":[md,code]})).unwrap()).unwrap();
        let app = App::new(Args {
            path: dir.path().to_owned(),
            python: None,
            check: false,
        })
        .unwrap();
        (dir, app)
    }
    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }
    fn attach_kernel(app: &mut App) -> std::sync::mpsc::Sender<Value> {
        let (kernel, sender) = kernel::Kernel::test_channel();
        app.kernel = Some(kernel);
        app.ready = true;
        sender
    }
    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn click(app: &mut App, target: impl Fn(&Action) -> bool) {
        let (area, _) = app.hits.iter().find(|(_, action)| target(action)).unwrap();
        let action = app
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains((area.x, area.y).into()))
            .unwrap()
            .1
            .clone();
        app.action(action).unwrap();
    }

    #[test]
    fn execution_timer_success_error_interrupt_and_controls() {
        let (_dir, mut app) = fixture();
        app.sidebar = false;
        app.select(1);
        let events = attach_kernel(&mut app);
        screen(&mut app, 120, 36);
        assert!(!app.hits.iter().any(|(_, a)| matches!(a, Action::Interrupt)));
        click(&mut app, |a| matches!(a, Action::Run));
        assert_eq!(app.running, Some(1));
        app.executions[1].started = Some(std::time::Instant::now() - Duration::from_millis(2200));
        let text = screen(&mut app, 120, 36);
        assert!(text.contains("Running · 2.2s"));
        assert!(text.contains("Cell 2:"));
        assert!(
            !app.hits
                .iter()
                .any(|(_, a)| matches!(a, Action::Run | Action::RunAll))
        );
        assert!(app.hits.iter().any(|(_, a)| matches!(a, Action::Interrupt)));
        events
            .send(json!({"event":"count","cell":1,"count":1}))
            .unwrap();
        for text in ["first", " second"] {
            events.send(json!({"event":"output","cell":1,"output":{"output_type":"stream","name":"stdout","text":text}})).unwrap();
        }
        events.send(json!({"event":"done","cell":1})).unwrap();
        app.poll_kernel().unwrap();
        assert_eq!(app.executions[1].state, State::Succeeded);
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[1]["outputs"][0]["text"],
            "first second"
        );
        assert!(screen(&mut app, 120, 36).contains("✓ Completed · 2.2s"));
        assert!(!app.hits.iter().any(|(_, a)| matches!(a, Action::Interrupt)));
        app.save().unwrap();
        let saved = Notebook::load(&app.notebook.as_ref().unwrap().path).unwrap();
        assert_eq!(saved.cells()[1]["execution_count"], 1);
        click(&mut app, |a| matches!(a, Action::Run));
        assert_eq!(app.executions[1].elapsed, None);
        app.handle_kernel_event(json!({"event":"output","cell":1,"output":{"output_type":"error","ename":"ValueError","evalue":"bad","traceback":["ValueError: bad"]}}));
        app.handle_kernel_event(json!({"event":"done","cell":1}));
        assert_eq!(app.executions[1].state, State::Failed);
        assert!(screen(&mut app, 120, 36).contains("× Error"));
        click(&mut app, |a| matches!(a, Action::Run));
        screen(&mut app, 120, 36);
        click(&mut app, |a| matches!(a, Action::Interrupt));
        assert!(
            app.running.is_some(),
            "Stop waits for kernel acknowledgement"
        );
        assert!(screen(&mut app, 120, 36).contains("Stopping"));
        assert!(!app.hits.iter().any(|(_, a)| matches!(a, Action::Interrupt)));
        app.handle_kernel_event(json!({"event":"output","cell":1,"output":{"output_type":"error","ename":"KeyboardInterrupt","evalue":"","traceback":[]}}));
        app.handle_kernel_event(json!({"event":"done","cell":1}));
        assert_eq!(app.executions[1].state, State::Interrupted);
        assert!(screen(&mut app, 120, 36).contains("■ Interrupted"));
    }

    #[test]
    fn run_all_queue_cancellation_error_and_kernel_disconnect() {
        let (_dir, mut app) = fixture();
        app.select(1);
        app.insert(false).unwrap();
        let _events = attach_kernel(&mut app);
        // Preserve the last successful run if a queued rerun is cancelled.
        app.executions[2].state = State::Succeeded;
        app.executions[2].elapsed = Some(Duration::from_secs(3));
        app.ready = false;
        app.action(Action::Restart).unwrap();
        assert!(
            app.confirm.is_none(),
            "Do not queue a restart during startup"
        );
        app.run(true).unwrap();
        assert!(app.executions[1].queued && app.executions[2].queued);
        assert!(screen(&mut app, 120, 36).contains("Queued"));
        app.action(Action::Interrupt).unwrap();
        assert!(!app.busy());
        assert!(!app.executions[2].queued);
        assert_eq!(app.executions[2].elapsed, Some(Duration::from_secs(3)));
        app.handle_kernel_event(json!({"event":"ready"}));
        app.run(true).unwrap();
        assert_eq!(app.running, Some(1));
        assert!(app.executions[2].queued);
        app.handle_kernel_event(json!({"event":"done","cell":1}));
        app.next().unwrap();
        assert_eq!(app.running, Some(2));
        app.handle_kernel_event(json!({"event":"output","cell":2,"output":{"output_type":"error","ename":"ZeroDivisionError","traceback":[]}}));
        app.handle_kernel_event(json!({"event":"done","cell":2}));
        assert!(!app.busy());
        assert_eq!(app.executions[1].state, State::Succeeded);
        assert_eq!(app.executions[2].state, State::Failed);
        app.run(true).unwrap();
        app.handle_kernel_event(json!({"event":"disconnected"}));
        assert!(!app.busy());
        assert!(app.kernel.is_none());
        assert_eq!(app.executions[1].state, State::Failed);
        assert!(!app.executions[2].queued);
        assert_eq!(app.kernel_status, "Stopped");
        // Stale events from an old run must never alter another cell's state.
        app.handle_kernel_event(json!({"event":"done","cell":999}));
        app.handle_kernel_event(json!({"event":"count","cell":999,"count":42}));
        app.handle_kernel_event(json!({"event":"input","prompt":"stale"}));
        assert!(app.input.is_none());
    }

    #[test]
    fn restart_input_and_output_display_lifecycle() {
        let (_dir, mut app) = fixture();
        app.select(1);
        let _events = attach_kernel(&mut app);
        app.run(false).unwrap();
        app.handle_kernel_event(json!({"event":"input","prompt":"Name? ","password":true}));
        assert!(screen(&mut app, 120, 36).contains("PYTHON INPUT"));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.input.as_ref().unwrap().2, "a");
        press(&mut app, KeyCode::Enter);
        assert!(app.input.is_none());
        app.handle_kernel_event(json!({"event":"output","cell":1,"display_id":"figure","output":{"output_type":"display_data","data":{"text/plain":"before"},"metadata":{}}}));
        app.handle_kernel_event(json!({"event":"update","cell":1,"display_id":"figure","data":{"text/plain":"after"},"metadata":{"updated":true}}));
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[1]["outputs"][0]["data"]["text/plain"],
            "after"
        );
        app.handle_kernel_event(json!({"event":"clear","cell":1}));
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[1]["outputs"],
            json!([])
        );
        app.action(Action::Restart).unwrap();
        assert!(matches!(app.confirm, Some(Confirm::Restart)));
        press(&mut app, KeyCode::Char('y'));
        assert!(!app.ready);
        assert!(!app.busy());
        assert_eq!(app.executions[1].state, State::Interrupted);
        assert!(app.input.is_none());
        app.handle_kernel_event(json!({"event":"ready"}));
        assert!(app.ready);
        assert_eq!(app.kernel_status, "Ready");
    }

    #[test]
    fn execution_history_follows_cell_structure_and_clear() {
        let (_dir, mut app) = fixture();
        app.select(1);
        app.executions[1].state = State::Succeeded;
        app.executions[1].elapsed = Some(Duration::from_secs(5));
        app.insert(true).unwrap();
        assert_eq!(app.executions[2].state, State::Succeeded);
        app.select(2);
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.executions[1].state, State::Succeeded);
        app.edit();
        press(&mut app, KeyCode::Char('#'));
        press(&mut app, KeyCode::Esc);
        assert!(app.executions[1].edited);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.executions[1].state, State::Idle);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[1]["cell_type"],
            "markdown"
        );
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[1]["cell_type"],
            "raw"
        );
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[1]["cell_type"],
            "code"
        );
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            app.executions.len(),
            app.notebook.as_ref().unwrap().cells().len()
        );
        app.save().unwrap();
        app.request(Confirm::Reload).unwrap();
        assert!(app.executions.iter().all(|e| e.state == State::Idle));
    }

    #[test]
    fn quit_button_protects_edits_and_remains_visible_at_small_widths() {
        let (_dir, mut app) = fixture();
        for width in [42, 75, 80, 120] {
            assert!(screen(&mut app, width, 24).contains("Quit"));
            assert!(app.hits.iter().any(|(_, a)| matches!(a, Action::Quit)));
        }
        app.edit();
        press(&mut app, KeyCode::Char('!'));
        screen(&mut app, 120, 36);
        click(&mut app, |a| matches!(a, Action::Quit));
        assert!(!app.quit);
        assert!(matches!(app.confirm, Some(Confirm::Quit)));
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.quit);
        screen(&mut app, 120, 36);
        click(&mut app, |a| matches!(a, Action::Quit));
        press(&mut app, KeyCode::Char('s'));
        assert!(app.quit);
        assert!(!app.dirty());
        assert!(
            source(
                &Notebook::load(&app.notebook.as_ref().unwrap().path)
                    .unwrap()
                    .cells()[0]
            )
            .starts_with('!')
        );
    }

    #[test]
    fn browser_preview_preserves_rich_output_and_escapes_plain_text() {
        let html = output_html(&json!({"outputs":[
            {"output_type":"stream","text":"<script>alert('test')</script>"},
            {"output_type":"display_data","data":{"text/html":"<table><tr><td>hello</td></tr></table>"}},
            {"output_type":"display_data","data":{"image/png":"png-data"}}
        ]}));
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("<table>"));
        assert!(html.contains("data:image/png;base64,png-data"));
    }

    #[test]
    fn file_navigation_creation_reload_and_attachments() {
        let (dir, mut app) = fixture();
        press(&mut app, KeyCode::Tab);
        assert!(app.focus_files && app.sidebar);
        press(&mut app, KeyCode::Enter);
        assert!(!app.focus_files);
        app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(!app.sidebar);
        app.create().unwrap();
        assert!(dir.path().join("Untitled-1.ipynb").exists());
        assert_eq!(app.executions.len(), 1);
        press(&mut app, KeyCode::Char('m'));
        app.notebook.as_mut().unwrap().cells_mut()[0]["attachments"] =
            json!({"plot.png":{"image/png":"data"}});
        assert!(
            app.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
                .is_err()
        );
        app.save().unwrap();
        app.request(Confirm::Open(0)).unwrap();
        assert_eq!(
            app.executions.len(),
            app.notebook.as_ref().unwrap().cells().len()
        );
    }
    #[test]
    fn course_notebooks() {
        let Some(root) = std::env::var_os("NOTEBOOK_TUI_TEST_NOTEBOOKS") else {
            return;
        };
        let files = notebook::discover(&PathBuf::from(root));
        assert!(!files.is_empty());
        let temp = tempfile::tempdir().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        for original in files {
            let copy = temp.path().join(original.file_name().unwrap());
            std::fs::copy(&original, &copy).unwrap();
            let data = Notebook::load(&copy).unwrap().data.clone();
            let mut app = App::new(Args {
                path: copy.clone(),
                python: None,
                check: false,
            })
            .unwrap();
            for i in 0..app.notebook.as_ref().unwrap().cells().len() {
                app.select(i);
                terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            }
            app.save().unwrap();
            assert_eq!(Notebook::load(&copy).unwrap().data, data);
        }
    }
    #[test]
    fn edit_save_structure_and_quit_protection() {
        let (_dir, mut app) = fixture();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char('é'));
        press(&mut app, KeyCode::Esc);
        assert!(app.dirty());
        app.save().unwrap();
        assert!(!app.dirty());
        let p = app.notebook.as_ref().unwrap().path.clone();
        assert!(source(&Notebook::load(&p).unwrap().cells()[1]).starts_with("print('hello')é"));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.selected, 2);
        assert_eq!(app.notebook.as_ref().unwrap().cells().len(), 3);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(
            app.notebook.as_ref().unwrap().cells()[2]["cell_type"],
            "markdown"
        );
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.selected, 1);
        press(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.confirm, Some(Confirm::Quit)));
        assert!(!app.quit);
        press(&mut app, KeyCode::Esc);
        assert!(app.confirm.is_none());
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.confirm, Some(Confirm::Delete)));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.notebook.as_ref().unwrap().cells().len(), 2);
    }
    #[test]
    fn notebook_layout_separates_input_output_and_markdown() {
        let (_dir, mut app) = fixture();
        app.sidebar = false;
        // Put the output-bearing input first, followed by rendered prose.
        app.notebook.as_mut().unwrap().cells_mut().swap(0, 1);
        let n = app.notebook.as_mut().unwrap();
        n.cells_mut()[0]["execution_count"] = json!(3);
        n.cells_mut()[0]["outputs"] = json!([{"output_type":"stream","name":"stdout","text":"An AI agent produces a response."}]);
        set_source(
            &mut n.cells_mut()[1],
            "# The four components\n\nA **readable** lesson with `state`.\n\n| Component | Meaning |\n|---|---|\n| State | Data being passed |\n| Node | Python function | ",
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 38)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = buffer
            .content()
            .chunks(100)
            .map(|row| row.iter().map(|c| c.symbol()).collect())
            .collect();
        println!("{}", rows.join("\n"));
        let find = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap();
        assert!(find("print(") < find("[3]"));
        assert!(find("[3]") < find("Output"));
        assert!(find("Output") < find("An AI agent"));
        assert!(find("An AI agent") < find("The four components"));
        assert!(rows[find("[3]")].contains('╰'));
        assert!(!rows[find("An AI agent")].contains('╯'));
        assert!(!rows[find("The four components")].contains('│'));
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("**") || r.contains("`state`"))
        );
        assert!(rows.iter().any(|r| r.contains('┬')));
        // Buttons must receive clicks before the enclosing cell selection target.
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, a)| matches!(a, Action::Edit(0)))
            .unwrap();
        let hit = app
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains((rect.x, rect.y).into()))
            .unwrap();
        assert!(matches!(hit.1, Action::Edit(0)));
        app.edit();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let pos = terminal.get_cursor_position().unwrap();
        assert_eq!(terminal.backend().buffer()[(pos.x, pos.y)].symbol(), "p");
    }

    #[test]
    fn responsive_render_and_click_targets() {
        let (_dir, mut app) = fixture();
        for (w, h) in [(120, 36), (80, 24), (42, 10), (20, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            if w >= 42 {
                assert!(text.contains("NOTEBOOK"));
                assert!(app.hits.iter().any(|(_, a)| matches!(a, Action::Run)));
            }
            for (rect, _) in &app.hits {
                assert!(rect.right() <= w);
                assert!(rect.bottom() <= h);
            }
        }
    }
    #[test]
    fn help_and_confirmation_render() {
        let (_dir, mut app) = fixture();
        let mut terminal = Terminal::new(TestBackend::new(100, 36)).unwrap();
        app.help = true;
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("KEYBOARD SHORTCUTS"));
        app.help = false;
        app.confirm = Some(Confirm::Quit);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("CONFIRMATION"));
    }
}
