use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

/// Render Markdown to terminal text. Raw HTML and images stay inert.
pub fn render(source: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut stack: Vec<Style> = vec![Style::default()];
    let mut list: Vec<Option<u64>> = Vec::new();
    let mut code = false;
    fn flush(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
        if !spans.is_empty() {
            lines.push(Line::from(std::mem::take(spans)));
        }
    }
    let mut events = Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    )
    .peekable();
    while let Some(event) = events.next() {
        let style = *stack.last().unwrap();
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush(&mut lines, &mut spans);
                    let marker = match level {
                        HeadingLevel::H1 => "",
                        HeadingLevel::H2 => "",
                        _ => "  ",
                    };
                    spans.push(Span::raw(marker));
                    stack.push(
                        style
                            .fg(Color::Rgb(116, 174, 255))
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::Emphasis => stack.push(style.add_modifier(Modifier::ITALIC)),
                Tag::Strong => stack.push(style.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => stack.push(style.add_modifier(Modifier::CROSSED_OUT)),
                Tag::CodeBlock(_) => {
                    flush(&mut lines, &mut spans);
                    code = true;
                    stack.push(style.fg(Color::Rgb(86, 211, 183)));
                }
                Tag::List(start) => {
                    flush(&mut lines, &mut spans);
                    list.push(start);
                }
                Tag::Item => {
                    flush(&mut lines, &mut spans);
                    let indent = "  ".repeat(list.len().saturating_sub(1));
                    let bullet = if let Some(Some(n)) = list.last_mut() {
                        let s = format!("{n}. ");
                        *n += 1;
                        s
                    } else {
                        "• ".into()
                    };
                    spans.push(Span::raw(format!("{indent}{bullet}")));
                }
                Tag::Link { .. } => stack.push(
                    style
                        .fg(Color::Rgb(116, 174, 255))
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Tag::BlockQuote(_) => {
                    flush(&mut lines, &mut spans);
                    spans.push(Span::styled("▎ ", Style::default().fg(Color::DarkGray)));
                }
                Tag::Table(_) => {
                    flush(&mut lines, &mut spans);
                    lines.extend(table(&mut events, width));
                    lines.push(Line::from(""));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) | TagEnd::CodeBlock => {
                    flush(&mut lines, &mut spans);
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    code = false;
                    lines.push(Line::from(""));
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                }
                TagEnd::Paragraph => {
                    flush(&mut lines, &mut spans);
                    if list.is_empty() {
                        lines.push(Line::from(""));
                    }
                }
                TagEnd::Item | TagEnd::TableRow | TagEnd::TableHead => {
                    flush(&mut lines, &mut spans)
                }
                TagEnd::List(_) => {
                    list.pop();
                }
                TagEnd::Table => {
                    flush(&mut lines, &mut spans);
                }
                _ => {}
            },
            Event::Text(text) => {
                let chunks: Vec<&str> = text.split('\n').collect();
                for (i, s) in chunks.iter().enumerate() {
                    if i > 0 {
                        flush(&mut lines, &mut spans);
                    }
                    if !s.is_empty() {
                        spans.push(Span::styled(s.to_string(), style));
                    }
                }
            }
            Event::Code(text) => spans.push(Span::styled(
                text.to_string(),
                style
                    .fg(Color::Rgb(86, 211, 183))
                    .bg(Color::Rgb(28, 37, 48)),
            )),
            Event::SoftBreak => spans.push(Span::raw(if code { "" } else { " " })),
            Event::HardBreak => flush(&mut lines, &mut spans),
            Event::Rule => {
                flush(&mut lines, &mut spans);
                lines.push(Line::from("────────────────────────"));
            }
            Event::TaskListMarker(done) => {
                spans.push(Span::raw(if done { "☑ " } else { "☐ " }))
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                spans.push(Span::styled(text.to_string(), style))
            }
            _ => {}
        }
    }
    flush(&mut lines, &mut spans);
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.into_iter().flat_map(|l| wrap(l, width)).collect()
}

/// Word wrapping with terminal display widths, retaining inline styles.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let chars: Vec<_> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    let mut result = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut used = 0;
        let mut space = None;
        while end < chars.len() {
            let c = chars[end].0;
            let w = c.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            if c.is_whitespace() && end > start {
                space = Some(end);
            }
            used += w;
            end += 1;
        }
        let mut next = end;
        if end < chars.len()
            && !chars[end].0.is_whitespace()
            && let Some(boundary) = space
        {
            end = boundary;
            next = boundary + 1;
        }
        if end == start {
            // A wide glyph cannot fit in a one-column viewport.
            start += 1;
            continue;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for &(c, style) in &chars[start..end] {
            if let Some(last) = spans.last_mut()
                && last.style == style
            {
                last.content.to_mut().push(c);
            } else {
                spans.push(Span::styled(c.to_string(), style));
            }
        }
        result.push(Line::from(spans).style(line.style));
        start = next;
        while start < chars.len() && chars[start].0.is_whitespace() {
            start += 1;
        }
    }
    if result.is_empty() {
        result.push(Line::from(""));
    }
    result
}

fn table<'a>(events: &mut impl Iterator<Item = Event<'a>>, width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Vec<Line<'static>>> = Vec::new();
    let mut row = Vec::new();
    let mut cell = Vec::new();
    let mut styles = vec![Style::default()];
    for event in events.by_ref() {
        let style = *styles.last().unwrap();
        match event {
            Event::End(TagEnd::Table) => break,
            Event::Start(Tag::TableHead) => styles.push(style.add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Strong) => styles.push(style.add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Emphasis) => styles.push(style.add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strikethrough) => {
                styles.push(style.add_modifier(Modifier::CROSSED_OUT))
            }
            Event::Start(Tag::Link { .. }) => styles.push(style.add_modifier(Modifier::UNDERLINED)),
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                styles.pop();
            }
            Event::Text(t) => cell.push(Span::styled(t.to_string(), style)),
            Event::Code(t) => cell.push(Span::styled(
                t.to_string(),
                style
                    .fg(Color::Rgb(86, 211, 183))
                    .bg(Color::Rgb(28, 37, 48)),
            )),
            Event::SoftBreak | Event::HardBreak => cell.push(Span::raw(" ")),
            Event::End(TagEnd::TableCell) => row.push(Line::from(std::mem::take(&mut cell))),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                rows.push(std::mem::take(&mut row));
                if matches!(event, Event::End(TagEnd::TableHead)) {
                    styles.pop();
                }
            }
            _ => {}
        }
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return Vec::new();
    }
    // On narrow terminals stack the fields instead of destroying the table.
    if width < columns * 4 + 1 {
        let headers = &rows[0];
        return rows
            .iter()
            .skip(1)
            .flat_map(|row| {
                let mut lines = Vec::new();
                for (i, value) in row.iter().enumerate() {
                    let mut spans = headers.get(i).cloned().unwrap_or_default().spans;
                    spans.push(Span::raw(": "));
                    spans.extend(value.spans.clone());
                    lines.extend(wrap(Line::from(spans), width));
                }
                lines.push(Line::from(""));
                lines
            })
            .collect();
    }
    let mut widths = vec![1; columns];
    for row in &rows {
        for (i, value) in row.iter().enumerate() {
            widths[i] = widths[i].max(value.width());
        }
    }
    let budget = width.saturating_sub(columns * 3 + 1);
    while widths.iter().sum::<usize>() > budget {
        let (i, largest) = widths.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
        if *largest <= 1 {
            break;
        }
        widths[i] -= 1;
    }
    let border = |left: &str, joint: &str, right: &str| {
        Line::styled(
            format!(
                "{}{}{}",
                left,
                widths
                    .iter()
                    .map(|w| "─".repeat(w + 2))
                    .collect::<Vec<_>>()
                    .join(joint),
                right
            ),
            Style::default().fg(Color::Rgb(65, 84, 103)),
        )
    };
    let mut lines = vec![border("┌", "┬", "┐")];
    for (index, row) in rows.iter().enumerate() {
        let cells: Vec<_> = widths
            .iter()
            .enumerate()
            .map(|(i, w)| wrap(row.get(i).cloned().unwrap_or_default(), *w))
            .collect();
        for y in 0..cells.iter().map(Vec::len).max().unwrap_or(1) {
            let mut spans = vec![Span::raw("│")];
            for (i, cell) in cells.iter().enumerate() {
                let l = cell.get(y).cloned().unwrap_or_default();
                let padding = widths[i].saturating_sub(l.width());
                spans.push(Span::raw(" "));
                spans.extend(l.spans);
                spans.push(Span::raw(format!("{} │", " ".repeat(padding))));
            }
            lines.push(Line::from(spans));
        }
        if index + 1 < rows.len() {
            lines.push(border("├", "┼", "┤"));
        }
    }
    lines.push(border("└", "┴", "┘"));
    lines
}
#[cfg(test)]
mod tests {
    use super::*;
    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn tables_keep_columns_styles_and_unicode_at_different_widths() {
        let source = "| Component | Meaning | Example |\n|---|---|---|\n| State (`state`) | Data carried during the workflow | Question, documents, response |\n| **Node** | A Python function performing a step | Call GLM |";
        for width in [100, 54, 25, 10] {
            let lines = render(source, width);
            assert!(
                lines.iter().all(|l| l.width() <= width),
                "{width}: {lines:?}"
            );
            let contents = lines.iter().map(text).collect::<Vec<_>>().join("\n");
            assert!(!contents.contains("`state`"));
            assert!(!contents.contains("**"));
            assert!(contents.contains("GLM"));
            if width >= 25 {
                assert!(contents.contains('┬'));
                let table_rows: Vec<_> =
                    lines.iter().filter(|l| text(l).starts_with('│')).collect();
                assert!(table_rows.len() >= 3);
                let expected = table_rows[0].width();
                for row in table_rows {
                    assert_eq!(row.width(), expected);
                    assert_eq!(text(row).matches('│').count(), 4);
                }
            }
        }
    }

    #[test]
    fn wrapping_preserves_words_and_styles() {
        let lines = render("**A sentence produced by GLM**, in English.", 23);
        assert_eq!(text(&lines[0]), "A sentence produced by");
        assert_eq!(text(&lines[1]), "GLM, in English.");
        assert!(
            lines[1].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn markdown_styles_and_lists() {
        let lines = render(
            "# Title\n\n**bold** et `code`\n\n- item\n\n<script>bad</script>",
            80,
        );
        let text = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();
        assert!(text.contains("Title"));
        assert!(text.contains("• item"));
        assert!(!text.contains("**"));
        assert!(!text.contains("<script>"));
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }
}
