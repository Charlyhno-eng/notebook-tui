use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[derive(Default, Clone)]
pub struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    undo: Vec<(Vec<String>, usize, usize)>,
    redo: Vec<(Vec<String>, usize, usize)>,
}
impl Editor {
    pub fn new(text: &str) -> Self {
        Self {
            lines: text.split('\n').map(str::to_owned).collect(),
            row: 0,
            col: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('z') => return self.restore(false),
                KeyCode::Char('y') => return self.restore(true),
                _ => return false,
            }
        }
        let modifies = matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Tab | KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete
        );
        if modifies {
            self.checkpoint();
        }
        let len = self.lines[self.row].chars().count();
        match key.code {
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.lines[self.row].chars().count();
                }
            }
            KeyCode::Right => {
                if self.col < len {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Up => {
                self.row = self.row.saturating_sub(1);
                self.col = self.col.min(self.lines[self.row].chars().count());
            }
            KeyCode::Down => {
                self.row = (self.row + 1).min(self.lines.len() - 1);
                self.col = self.col.min(self.lines[self.row].chars().count());
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = len,
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let i = self.byte();
                self.lines[self.row].insert(i, c);
                self.col += 1;
                return true;
            }
            KeyCode::Tab => {
                let i = self.byte();
                self.lines[self.row].insert_str(i, "    ");
                self.col += 4;
                return true;
            }
            KeyCode::Enter => {
                let i = self.byte();
                let rest = self.lines[self.row].split_off(i);
                let indent = self.lines[self.row]
                    .chars()
                    .take_while(|c| *c == ' ')
                    .count();
                self.lines
                    .insert(self.row + 1, format!("{}{rest}", " ".repeat(indent)));
                self.row += 1;
                self.col = indent;
                return true;
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    let end = self.byte();
                    self.col -= 1;
                    let start = self.byte();
                    self.lines[self.row].replace_range(start..end, "");
                } else if self.row > 0 {
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.lines[self.row].chars().count();
                    self.lines[self.row].push_str(&line);
                } else {
                    return false;
                }
                return true;
            }
            KeyCode::Delete => {
                if self.col < len {
                    let i = self.byte();
                    let end = i + self.lines[self.row][i..].chars().next().unwrap().len_utf8();
                    self.lines[self.row].replace_range(i..end, "");
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                } else {
                    return false;
                }
                return true;
            }
            _ => {}
        }
        false
    }
    fn byte(&self) -> usize {
        self.lines[self.row]
            .char_indices()
            .nth(self.col)
            .map_or(self.lines[self.row].len(), |(i, _)| i)
    }
    fn checkpoint(&mut self) {
        self.undo.push((self.lines.clone(), self.row, self.col));
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn restore(&mut self, redo: bool) -> bool {
        let snapshot = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some((lines, row, col)) = snapshot {
            let current = (
                std::mem::replace(&mut self.lines, lines),
                self.row,
                self.col,
            );
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.row = row;
            self.col = col;
            true
        } else {
            false
        }
    }
    pub fn paste(&mut self, text: &str) {
        self.checkpoint();
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let text: String = normalized
            .chars()
            .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
            .collect();
        let i = self.byte();
        let tail = self.lines[self.row].split_off(i);
        let mut parts = text.split('\n');
        self.lines[self.row].push_str(parts.next().unwrap_or(""));
        for part in parts {
            self.row += 1;
            self.lines.insert(self.row, part.to_owned());
        }
        self.col = self.lines[self.row].chars().count();
        self.lines[self.row].push_str(&tail);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_preserves_indentation_and_undo() {
        let mut e = Editor::new("    tail");
        e.col = 4;
        e.paste("if True:\n        print('café')\n");
        assert_eq!(e.text(), "    if True:\n        print('café')\ntail");
        e.key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "    tail");
        e.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "    if True:\n        print('café')\ntail");
    }
    #[test]
    fn unicode_edit() {
        let mut e = Editor::new("café\n世界");
        for code in [
            KeyCode::End,
            KeyCode::Backspace,
            KeyCode::Enter,
            KeyCode::Char('é'),
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::Backspace,
        ] {
            e.key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert_eq!(e.text(), "caf\né世界");
    }
}
