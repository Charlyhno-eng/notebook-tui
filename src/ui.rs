use crate::{
    Action, App, Confirm,
    execution::State,
    notebook::{multiline, source},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
const BG: Color = Color::Rgb(15, 20, 30);
const PANEL: Color = Color::Rgb(22, 29, 42);
const FG: Color = Color::Rgb(209, 219, 234);
const MUTED: Color = Color::Rgb(124, 142, 164);
const ACCENT: Color = Color::Rgb(86, 211, 183);
const BLUE: Color = Color::Rgb(116, 174, 255);
const RED: Color = Color::Rgb(255, 119, 139);
pub fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\r' {
            // A carriage return replaces the current progress line.
            if chars.peek() != Some(&'\n') {
                while !result.is_empty() && !result.ends_with('\n') {
                    result.pop();
                }
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            result.push(c);
        }
    }
    result
}
pub fn output_text(o: &Value) -> String {
    let text = match o["output_type"].as_str().unwrap_or("") {
        "stream" => multiline(&o["text"]),
        "error" => o["traceback"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| format!("{}: {}", o["ename"], o["evalue"])),
        _ => {
            let data = &o["data"];
            let mut s = multiline(&data["text/plain"]);
            if !data["image/png"].is_null() || !data["image/jpeg"].is_null() {
                s.push_str("\n▧ Image — terminal preview below • o: original size");
            } else if s.is_empty() && !data["text/html"].is_null() {
                s = "▤ HTML — o: open in browser".into();
            } else if s.is_empty() {
                s = format!(
                    "Rich output ({}) — preserved in the notebook",
                    data.as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default()
                );
            }
            s
        }
    };
    strip_ansi(&text)
}
fn line(s: impl Into<String>, color: Color) -> Line<'static> {
    Line::styled(s.into(), Style::default().fg(color))
}
fn button(
    f: &mut Frame,
    app: &mut App,
    x: &mut u16,
    y: u16,
    right: u16,
    specification: (&str, Action, Color, bool),
) {
    let (label, action, color, enabled) = specification;
    let width = label.width() as u16 + 2;
    if x.saturating_add(width) > right {
        return;
    }
    let area = Rect::new(*x, y, width, 1);
    f.render_widget(
        Paragraph::new(format!(" {label} ")).style(if enabled {
            Style::default().fg(color).bg(PANEL)
        } else {
            Style::default()
                .fg(MUTED)
                .bg(PANEL)
                .add_modifier(Modifier::DIM)
        }),
        area,
    );
    if enabled {
        app.hits.push((area, action));
    }
    *x += width + 1;
}
fn highlight(s: &str, kind: &str) -> Line<'static> {
    if kind == "markdown" {
        let trim = s.trim_start();
        return if trim.starts_with('#') {
            Line::styled(
                s.to_owned(),
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            )
        } else if trim.starts_with("```") {
            line(s, ACCENT)
        } else {
            line(s, FG)
        };
    }
    if kind != "code" {
        return line(s, FG);
    }
    let mut spans = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' {
            let rest: String = std::iter::once(c).chain(chars).collect();
            spans.push(Span::styled(rest, Style::default().fg(MUTED)));
            break;
        }
        let mut token = c.to_string();
        let color = if c == '\'' || c == '"' {
            let mut escaped = false;
            for next in chars.by_ref() {
                token.push(next);
                if next == c && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
            }
            ACCENT
        } else if c.is_alphabetic() || c == '_' {
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                token.push(chars.next().unwrap());
            }
            if matches!(
                token.as_str(),
                "import"
                    | "from"
                    | "as"
                    | "def"
                    | "class"
                    | "return"
                    | "if"
                    | "else"
                    | "elif"
                    | "for"
                    | "while"
                    | "in"
                    | "try"
                    | "except"
                    | "finally"
                    | "with"
                    | "async"
                    | "await"
                    | "yield"
                    | "raise"
                    | "pass"
                    | "not"
                    | "and"
                    | "or"
                    | "lambda"
                    | "True"
                    | "False"
                    | "None"
            ) {
                Color::Rgb(194, 151, 255)
            } else if matches!(
                token.as_str(),
                "print" | "len" | "range" | "str" | "int" | "list" | "dict" | "input"
            ) {
                BLUE
            } else {
                FG
            }
        } else if c.is_ascii_digit() {
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                token.push(chars.next().unwrap());
            }
            Color::Rgb(244, 191, 117)
        } else {
            FG
        };
        spans.push(Span::styled(token, Style::default().fg(color)));
    }
    Line::from(spans)
}
fn wrap_line(l: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut result = Vec::new();
    let mut current = Vec::new();
    let mut used = 0;
    for span in l.spans {
        let mut part = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width && !part.is_empty() {
                current.push(Span::styled(std::mem::take(&mut part), span.style));
            }
            if used + w > width {
                result.push(Line::from(std::mem::take(&mut current)));
                used = 0;
            }
            part.push(c);
            used += w;
        }
        if !part.is_empty() {
            current.push(Span::styled(part, span.style));
        }
    }
    result.push(Line::from(current));
    result
}
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w.min(area.width),
        h.min(area.height),
    )
}
fn popup(f: &mut Frame, title: &str, text: Text<'_>, width: u16, height: u16) {
    let area = centered(f.area(), width, height);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text)
            .style(Style::default().bg(PANEL).fg(FG))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(ACCENT)),
            ),
        area,
    );
}
pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    let area = f.area();
    f.render_widget(Block::new().style(Style::default().bg(BG).fg(FG)), area);
    if area.width < 42 || area.height < 10 {
        f.render_widget(
            Paragraph::new("Enlarge the terminal (42 × 10 minimum)"),
            area,
        );
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(area);
    let name = app
        .notebook
        .as_ref()
        .map(|n| {
            n.path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "Welcome".into());
    let dirty = if app.notebook.as_ref().is_some_and(|n| n.dirty) {
        " ●"
    } else {
        ""
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ◈ NOTEBOOK ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" / {name}{dirty}")),
        ]))
        .block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(PANEL)),
        ),
        parts[0],
    );
    let mut x = 1;
    // Keep Quit visible even when the terminal cannot fit every toolbar item.
    let right = area.right().saturating_sub(9);
    let y = parts[1].y;
    let busy = app.busy();
    let has_notebook = app.notebook.is_some();
    for (label, a, c, enabled) in [
        ("▶ Run F5", Action::Run, ACCENT, has_notebook && !busy),
        ("▶▶ All ⇧F5", Action::RunAll, ACCENT, has_notebook && !busy),
        ("■ Stop", Action::Interrupt, RED, busy && !app.interrupted),
        ("↻ Restart", Action::Restart, BLUE, has_notebook),
        ("Save", Action::Save, FG, has_notebook),
        ("? Help", Action::Help, MUTED, true),
    ] {
        button(f, app, &mut x, y, right, (label, a, c, enabled));
    }
    let mut quit_x = area.right().saturating_sub(8);
    button(
        f,
        app,
        &mut quit_x,
        y,
        area.right(),
        ("Quit", Action::Quit, FG, true),
    );
    let content = parts[2];
    let body = if app.sidebar && area.width >= 75 {
        let columns =
            Layout::horizontal([Constraint::Length(29), Constraint::Min(1)]).split(content);
        let title = if app.focus_files {
            " FILES • Enter to open "
        } else {
            " FILES • Tab "
        };
        let items: Vec<ListItem> = app
            .files
            .iter()
            .map(|p| {
                let name = p.strip_prefix(&app.root).unwrap_or(p).to_string_lossy();
                ListItem::new(format!(" {name}"))
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::new()
                    .borders(Borders::RIGHT)
                    .title(title)
                    .border_style(Style::default().fg(if app.focus_files {
                        ACCENT
                    } else {
                        MUTED
                    })),
            )
            .highlight_style(Style::default().bg(Color::Rgb(36, 52, 72)).fg(ACCENT))
            .highlight_symbol("▸");
        let mut state = ListState::default();
        if !app.files.is_empty() {
            state.select(Some(app.file_index));
        }
        f.render_stateful_widget(list, columns[0], &mut state);
        for i in state.offset()..app.files.len() {
            let y = columns[0].y + 1 + (i - state.offset()) as u16;
            if y >= columns[0].bottom() {
                break;
            }
            app.hits.push((
                Rect::new(columns[0].x, y, columns[0].width.saturating_sub(1), 1),
                Action::Open(i),
            ));
        }
        columns[1]
    } else {
        content
    };
    draw_notebook(f, app, body);
    let mode = if app.editor.is_some() {
        "EDIT"
    } else if app.focus_files {
        "FILES"
    } else if area.width < 80 {
        "NAV"
    } else {
        "NAVIGATION"
    };
    let count = app.notebook.as_ref().map_or(0, |n| n.cells().len());
    let activity = if let Some(index) = app.running {
        let label = app.executions[index].label();
        let label = if app.interrupted {
            label.replace("Running", "Stopping")
        } else {
            label
        };
        format!("Cell {}: {}", index + 1, label)
    } else {
        format!("Python: {}", app.kernel_status)
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(format!(" {mode} "), Style::default().bg(ACCENT).fg(BG)),
                Span::raw(format!(
                    "  {}  │  Cell {}/{}  │  Ctrl+S save • Ctrl+Q quit",
                    activity,
                    if count == 0 { 0 } else { app.selected + 1 },
                    count
                )),
            ]),
            line(format!(" {}", app.status), MUTED),
        ]),
        parts[3],
    );
    if app.help {
        let text = Text::from(vec![
            line(" A persistent Python kernel for your notebook", ACCENT),
            line("", FG),
            line(" Enter / i       Edit the selected cell", FG),
            line(" Esc             Return to navigation", FG),
            line(" F5 / Ctrl+Enter Run the selected cell", FG),
            line(" Shift+F5        Run all code cells", FG),
            line(" Shift+Enter     Run and select the next cell", FG),
            line(" Ctrl+C          Interrupt execution", FG),
            line(" R               Restart the kernel (confirmation)", FG),
            line(" Ctrl+S          Save the notebook", FG),
            line(" Ctrl+Z / Ctrl+Y Undo / redo while editing", FG),
            line(" Ctrl+L          Reload from disk", FG),
            line(" j/k or ↑/↓      Select a cell", FG),
            line(" PgUp/PgDn       Scroll through long cells", FG),
            line(" a / b           Insert a cell above / below", FG),
            line(" d               Delete the cell (confirmation)", FG),
            line(" m / y / r       Markdown / code / raw text", FG),
            line(" J / K           Move the cell down / up", FG),
            line(" x               Clear the selected cell outputs", FG),
            line(" o               Open rich outputs in the browser", FG),
            line(" Tab / Ctrl+B    Focus files / toggle sidebar", FG),
            line(" n               Create a notebook in this directory", FG),
            line(" q / Ctrl+Q      Quit (unsaved changes are protected)", FG),
            line(" Mouse           Click buttons / cells, scroll", FG),
            line("", FG),
            line(" F1 / ? / Esc: close this help", ACCENT),
        ]);
        popup(f, " KEYBOARD SHORTCUTS ", text, 76, 28);
    }
    if let Some(c) = app.confirm {
        let text = match c {
            Confirm::Delete => "Delete this cell and its outputs?",
            Confirm::Restart => "Restart the kernel? All variables will be lost.",
            _ => "This notebook has unsaved changes.",
        };
        let save = if matches!(c, Confirm::Delete | Confirm::Restart) {
            ""
        } else {
            "s: save and continue\n"
        };
        popup(
            f,
            " CONFIRMATION ",
            Text::from(format!("\n {text}\n\n {save}y: confirm   n / Esc: cancel")),
            68,
            9,
        );
    }
    if let Some((prompt, password, value)) = &app.input {
        let text = if *password {
            "•".repeat(value.chars().count())
        } else {
            value.clone()
        };
        popup(
            f,
            " PYTHON INPUT ",
            Text::from(format!(
                "\n {}\n\n {}█\n\n Enter: submit • Ctrl+C: interrupt",
                strip_ansi(prompt),
                text
            )),
            72,
            10,
        );
    }
}
/// Code has a closed frame and a separate background; output never shares it.
fn input_line(l: Line<'static>, gutter: usize, width: usize, border: Color) -> Line<'static> {
    let padding = width.saturating_sub(l.width() + 4);
    let mut spans = vec![
        Span::raw(" ".repeat(gutter)),
        Span::styled("│", Style::default().fg(border)),
        Span::styled(" ", Style::default().bg(PANEL)),
    ];
    spans.extend(l.spans.into_iter().map(|s| {
        let style = Style::default().bg(PANEL).patch(s.style);
        s.style(style)
    }));
    spans.push(Span::styled(
        format!("{} ", " ".repeat(padding)),
        Style::default().bg(PANEL),
    ));
    spans.push(Span::styled("│", Style::default().fg(border)));
    Line::from(spans)
}
fn draw_notebook(f: &mut Frame, app: &mut App, body: Rect) {
    let Some(n) = &app.notebook else {
        f.render_widget(Paragraph::new("\n  No notebook in this directory.\n\n  n: create a notebook\n\n  notebook-tui /path/to/notebooks"),body);
        return;
    };
    if n.cells().is_empty() {
        f.render_widget(Paragraph::new("\n  Empty notebook — b: add a cell"), body);
        return;
    }
    // Keep execution counters outside the input, like a notebook editor gutter.
    let gutter = 7usize.min(body.width.saturating_sub(8) as usize);
    let box_width = (body.width as usize).saturating_sub(gutter + 1).max(4);
    let width = box_width.saturating_sub(4).max(1);
    let mut lines = Vec::new();
    let mut positions = Vec::new();
    let mut ends = Vec::new();
    let mut cursor = None;
    let mut controls = Vec::new();
    for (i, cell) in n.cells().iter().enumerate() {
        positions.push(lines.len());
        let selected = i == app.selected;
        let kind = cell["cell_type"].as_str().unwrap_or("raw");
        let color = if selected {
            ACCENT
        } else {
            Color::Rgb(55, 72, 92)
        };
        let count = if app.running == Some(i) {
            "*".into()
        } else {
            cell["execution_count"]
                .as_i64()
                .map(|v| v.to_string())
                .unwrap_or(" ".into())
        };
        let src = source(cell);
        let edit = selected && app.editor.is_some();
        let rendered_markdown = kind == "markdown" && !edit;
        let label = match kind {
            "code" => "Python",
            "markdown" => "Markdown",
            _ => "Raw text",
        };
        // A contextual toolbar belongs to the input only; output has its own area.
        let mut header = if rendered_markdown {
            vec![
                Span::raw(" ".repeat(gutter)),
                Span::styled(
                    if selected {
                        "▎ Markdown ".to_string()
                    } else {
                        String::new()
                    },
                    Style::default().fg(MUTED),
                ),
            ]
        } else {
            vec![
                Span::raw(" ".repeat(gutter)),
                Span::styled(format!("╭─ {label} "), Style::default().fg(color)),
            ]
        };
        let mut header_width: usize = header.iter().map(Span::width).sum();
        if selected {
            let buttons = if kind == "code" {
                vec![
                    ("[▶ F5]", Action::Run),
                    ("[edit]", Action::Edit(i)),
                    ("[output o]", Action::Preview),
                ]
            } else {
                vec![("[edit]", Action::Edit(i))]
            };
            for (text, action) in buttons {
                if header_width + text.width() + 2 > gutter + box_width {
                    break;
                }
                let enabled = !matches!(action, Action::Run) || !app.busy();
                if enabled {
                    controls.push((lines.len(), header_width, text.width(), action));
                }
                header.push(Span::styled(
                    text,
                    Style::default().fg(if enabled { ACCENT } else { MUTED }),
                ));
                header.push(Span::raw(" "));
                header_width += text.width() + 1;
            }
        }
        if !rendered_markdown {
            header.push(Span::styled(
                format!(
                    "{}╮",
                    "─".repeat((gutter + box_width).saturating_sub(header_width + 1))
                ),
                Style::default().fg(color),
            ));
        }
        lines.push(Line::from(header));
        if rendered_markdown {
            for l in crate::markdown::render(&src, width) {
                let mut spans = vec![
                    Span::raw(" ".repeat(gutter)),
                    Span::styled(
                        if selected { "▎ " } else { "  " },
                        Style::default().fg(color),
                    ),
                ];
                spans.extend(l.spans);
                lines.push(Line::from(spans));
            }
        } else {
            // A little vertical padding makes input and prose visibly different.
            lines.push(input_line(Line::from(""), gutter, box_width, color));
            for (row, s) in src.split('\n').enumerate() {
                let mut wrapped = wrap_line(highlight(s, kind), width);
                if edit {
                    let e = app.editor.as_ref().unwrap();
                    if row == e.row {
                        let prefix: String = s.chars().take(e.col).collect();
                        let w = prefix.width();
                        if w / width >= wrapped.len() {
                            wrapped.push(Line::from(""));
                        }
                        cursor = Some((gutter + 2 + w % width, lines.len() + w / width));
                    }
                }
                for l in wrapped {
                    lines.push(input_line(l, gutter, box_width, color));
                }
            }
            lines.push(input_line(Line::from(""), gutter, box_width, color));
            let counter = if kind == "code" {
                format!("[{count}]")
            } else {
                String::new()
            };
            // Counts remain right-aligned even after thousands of executions.
            let counter: String = counter
                .chars()
                .rev()
                .take(gutter.saturating_sub(1))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>w$} ", counter, w = gutter.saturating_sub(1)),
                    Style::default().fg(if app.running == Some(i) {
                        ACCENT
                    } else {
                        MUTED
                    }),
                ),
                Span::styled(
                    format!("╰{}╯", "─".repeat(box_width - 2)),
                    Style::default().fg(color),
                ),
            ]));
        }
        if kind == "code"
            && let Some(execution) = app.executions.get(i)
        {
            let mut label = execution.label();
            if app.running == Some(i) && app.interrupted {
                label = label.replace("Running", "Stopping");
            }
            if !label.is_empty() {
                let status_color = match execution.state {
                    State::Succeeded => ACCENT,
                    State::Failed => RED,
                    State::Running => BLUE,
                    _ => MUTED,
                };
                lines.push(line(
                    format!("{}  {}", " ".repeat(gutter), label),
                    status_color,
                ));
            }
        }
        if let Some(outputs) = cell["outputs"].as_array()
            && !outputs.is_empty()
        {
            lines.push(Line::from(""));
            lines.push(line(format!("{}  Output", " ".repeat(gutter)), MUTED));
            for o in outputs {
                let error = o["output_type"] == "error" || o["name"] == "stderr";
                let text = output_text(o);
                // Markdown MIME data is formatted too, rather than shown as markup.
                let output_lines = if let Some(md) = o["data"]["text/markdown"].as_str() {
                    crate::markdown::render(md, width)
                } else {
                    text.lines()
                        .flat_map(|s| {
                            crate::markdown::wrap(line(s, if error { RED } else { FG }), width)
                        })
                        .collect()
                };
                for l in output_lines {
                    let mut spans = vec![Span::styled(
                        format!("{}{} ", " ".repeat(gutter), if error { "!" } else { "│" }),
                        Style::default().fg(if error { RED } else { MUTED }),
                    )];
                    spans.extend(l.spans);
                    lines.push(Line::from(spans));
                }
                if let Some(encoded) = o["data"]["image/png"]
                    .as_str()
                    .or_else(|| o["data"]["image/jpeg"].as_str())
                {
                    for image_line in app.images.render(encoded, width) {
                        let mut spans = vec![Span::raw(format!("{}  ", " ".repeat(gutter)))];
                        spans.extend(image_line.spans);
                        lines.push(Line::from(spans));
                    }
                }
            }
        }
        ends.push(lines.len());
        lines.push(Line::from(""));
        lines.push(Line::from(""));
    }
    app.cell_positions = positions.clone();
    let height = body.height as usize;
    if app.follow {
        let start = positions[app.selected];
        let end = ends[app.selected];
        let target = cursor.map(|(_, y)| y).unwrap_or(start);
        if target < app.scroll || target >= app.scroll + height {
            app.scroll = target.saturating_sub(if cursor.is_some() { height / 2 } else { 0 });
        } else if end.saturating_sub(start) <= height && end > app.scroll + height {
            app.scroll = end.saturating_sub(height);
        }
        app.follow = false;
    }
    app.scroll = app.scroll.min(lines.len().saturating_sub(height));
    // Slice instead of Paragraph::scroll(u16): very large notebook output stays navigable.
    let visible: Vec<Line> = lines.into_iter().skip(app.scroll).take(height).collect();
    f.render_widget(Paragraph::new(visible), body);
    for (i, &start) in positions.iter().enumerate() {
        let end = ends[i];
        if end <= app.scroll || start >= app.scroll + height {
            continue;
        }
        let y = body.y + start.saturating_sub(app.scroll) as u16;
        let h = (end.min(app.scroll + height) - start.max(app.scroll)) as u16;
        app.hits
            .push((Rect::new(body.x, y, body.width, h), Action::Select(i)));
    }
    // Register controls after cell targets so the buttons receive the click.
    for (row, x, width, action) in controls {
        if row >= app.scroll && row < app.scroll + height {
            app.hits.push((
                Rect::new(
                    body.x + x as u16,
                    body.y + (row - app.scroll) as u16,
                    width as u16,
                    1,
                ),
                action,
            ));
        }
    }
    if let Some((x, y)) = cursor
        && y >= app.scroll
        && y < app.scroll + height
        && x < body.width as usize
    {
        f.set_cursor_position((body.x + x as u16, body.y + (y - app.scroll) as u16));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_escape_sequences_are_removed() {
        assert_eq!(strip_ansi("\x1b[31mErreur\x1b[0m\n世界"), "Erreur\n世界");
        assert_eq!(strip_ansi("10%\r20%"), "20%");
    }
    #[test]
    fn narrow_unicode_wrapping() {
        let l = wrap_line(line("é世界abc", FG), 4);
        assert!(l.iter().all(|l| l.width() <= 4));
        assert_eq!(
            l.iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect::<String>(),
            "é世界abc"
        );
    }
}
