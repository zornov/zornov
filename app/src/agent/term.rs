// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The terminal itself: raw keys, and an input box that edits a line the
//! way terminal apps do (arrows, history, Ctrl-A/E/U/W, Tab), with a menu
//! of commands under it when the line starts with `/`, a ghost of the rest
//! of a word, the fields a command takes, and a status line.

use std::io::Write;

/// Raw keys while alive: no echo, no line buffering, Ctrl-C as a key.
/// Output still turns `\n` into a new line.
#[cfg(unix)]
pub struct Raw {
    saved: libc::termios,
}

/// Windows: the console's own modes, keys coming in as the same escape
/// sequences a Unix terminal sends.
#[cfg(windows)]
pub struct Raw {
    saved_in: u32,
    saved_out: u32,
}

#[cfg(windows)]
impl Raw {
    pub fn on() -> Option<Raw> {
        use windows_sys::Win32::System::Console::*;
        unsafe {
            let (i, o) = (GetStdHandle(STD_INPUT_HANDLE), GetStdHandle(STD_OUTPUT_HANDLE));
            let (mut saved_in, mut saved_out) = (0, 0);
            if GetConsoleMode(i, &mut saved_in) == 0 {
                return None;
            }
            GetConsoleMode(o, &mut saved_out);
            SetConsoleMode(i, (saved_in & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT)) | ENABLE_VIRTUAL_TERMINAL_INPUT);
            SetConsoleMode(o, saved_out | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            Some(Raw { saved_in, saved_out })
        }
    }
}

#[cfg(windows)]
impl Drop for Raw {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::*;
        unsafe {
            SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), self.saved_in);
            SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), self.saved_out);
        }
    }
}

#[cfg(unix)]
impl Raw {
    pub fn on() -> Option<Raw> {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return None;
            }
            let saved = t;
            t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
            t.c_iflag &= !(libc::IXON | libc::ICRNL);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Some(Raw { saved })
        }
    }
}

#[cfg(unix)]
impl Drop for Raw {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.saved);
        }
    }
}

/// The terminal's width in columns.
#[cfg(windows)]
pub fn width() -> usize {
    use windows_sys::Win32::System::Console::*;
    unsafe {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) != 0 {
            let w = info.srWindow.Right - info.srWindow.Left + 1;
            if w > 0 {
                return w as usize;
            }
        }
        80
    }
}

/// Whether a key is waiting, within `ms`.
#[cfg(windows)]
pub fn ready(ms: i32) -> bool {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    unsafe { WaitForSingleObject(GetStdHandle(STD_INPUT_HANDLE), if ms < 0 { INFINITE } else { ms as u32 }) == WAIT_OBJECT_0 }
}

/// The terminal's width in columns.
#[cfg(unix)]
pub fn width() -> usize {
    unsafe {
        let mut w: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut w) == 0 && w.ws_col > 0 { w.ws_col as usize } else { 80 }
    }
}

/// Whether a key is waiting, within `ms`.
#[cfg(unix)]
pub fn ready(ms: i32) -> bool {
    let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    unsafe { libc::poll(&mut p, 1, ms) > 0 }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    KillLine,
    KillWord,
    Esc,
    CtrlC,
    CtrlD,
    CtrlL,
    Other,
}

/// One byte straight from the terminal: Rust's stdin buffers, which would
/// hide the rest of an escape sequence from `ready`.
fn byte() -> Option<u8> {
    let mut b = 0u8;
    loop {
        let n = unsafe { libc::read(0, (&mut b as *mut u8).cast(), 1) };
        match n {
            1 => return Some(b),
            -1 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
            _ => return None,
        }
    }
}

/// The next key (blocks).
pub fn key() -> Option<Key> {
    let b = byte()?;
    Some(match b {
        b'\r' | b'\n' => Key::Enter,
        b'\t' => Key::Tab,
        127 | 8 => Key::Backspace,
        1 => Key::Home,
        5 => Key::End,
        21 => Key::KillLine,
        23 => Key::KillWord,
        3 => Key::CtrlC,
        4 => Key::CtrlD,
        12 => Key::CtrlL,
        2 => Key::Left,
        6 => Key::Right,
        16 => Key::Up,
        14 => Key::Down,
        27 => {
            if !ready(30) {
                return Some(Key::Esc);
            }
            match byte()? {
                b'[' | b'O' => {
                    let mut params = Vec::new();
                    loop {
                        let c = byte()?;
                        if (0x40..=0x7e).contains(&c) {
                            let p = String::from_utf8_lossy(&params).to_string();
                            break match (c, p.as_str()) {
                                (b'A', _) => Key::Up,
                                (b'B', _) => Key::Down,
                                (b'C', "1;3" | "1;9") => Key::WordRight,
                                (b'D', "1;3" | "1;9") => Key::WordLeft,
                                (b'C', _) => Key::Right,
                                (b'D', _) => Key::Left,
                                (b'H', _) => Key::Home,
                                (b'F', _) => Key::End,
                                (b'~', "3") => Key::Delete,
                                (b'~', "1" | "7") => Key::Home,
                                (b'~', "4" | "8") => Key::End,
                                _ => Key::Other,
                            };
                        }
                        params.push(c);
                    }
                }
                b'b' => Key::WordLeft,
                b'f' => Key::WordRight,
                127 => Key::KillWord,
                _ => Key::Other,
            }
        }
        c if c < 32 => Key::Other,
        c => {
            // UTF-8: as many bytes as the first one says.
            let n = if c >= 0xf0 { 3 } else if c >= 0xe0 { 2 } else if c >= 0xc0 { 1 } else { 0 };
            let mut buf = vec![c];
            for _ in 0..n {
                buf.push(byte()?);
            }
            String::from_utf8(buf).ok().and_then(|s| s.chars().next()).map_or(Key::Other, Key::Char)
        }
    })
}

/// What the box knows about the commands.
pub struct Catalog {
    /// Name and one line about it.
    pub commands: Vec<(String, String)>,
    /// The fields a command takes.
    pub fields: Box<dyn Fn(&str) -> Vec<String>>,
}

pub struct Paint {
    pub on: bool,
}

impl Paint {
    pub fn c(&self, code: &str, s: &str) -> String {
        if self.on { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_owned() }
    }
}

/// Where the box was drawn: its top line's width and the cursor's column
/// on the input line under it.
struct Drawn {
    top: usize,
    col: usize,
}

/// Styled pieces cut to `max` visible characters, as one line.
fn fit(pieces: &[(&str, String)], max: usize, p: &Paint) -> String {
    let mut out = String::new();
    let mut left = max;
    for (code, text) in pieces {
        if left == 0 {
            break;
        }
        let t: String = text.chars().take(left).collect();
        left -= t.chars().count();
        out.push_str(&if code.is_empty() { t } else { p.c(code, &t) });
    }
    out
}

pub enum Outcome {
    Line(String),
    Exit,
}

/// The input box with everything around it.
pub struct Editor {
    pub history: Vec<String>,
    history_file: Option<std::path::PathBuf>,
    buf: Vec<char>,
    cur: usize,
    hist_at: Option<usize>,
    draft: String,
    menu_sel: usize,
    /// What was drawn last time, to come back to it.
    drawn: Option<Drawn>,
}

fn words_of(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

impl Editor {
    pub fn new(history_file: Option<std::path::PathBuf>) -> Editor {
        let history = history_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default();
        Editor { history, history_file, buf: Vec::new(), cur: 0, hist_at: None, draft: String::new(), menu_sel: 0, drawn: None }
    }

    fn text(&self) -> String {
        self.buf.iter().collect()
    }

    fn set(&mut self, s: &str) {
        self.buf = s.chars().collect();
        self.cur = self.buf.len();
    }

    /// Commands matching what is typed after `/`, best first: names that
    /// start with it, then names that hold it; with nothing typed yet, the
    /// everyday ones.
    fn menu<'a>(&self, cat: &'a Catalog) -> Vec<&'a (String, String)> {
        let t = self.text();
        // Browsing history: the arrows stay with it.
        if self.hist_at.is_some() {
            return Vec::new();
        }
        let Some(q) = t.strip_prefix('/').filter(|q| !q.contains(' ')) else { return Vec::new() };
        let mut hits: Vec<&(String, String)> = if q.is_empty() {
            ["help", "see", "check", "summary", "text", "card", "import", "export", "undo", "new"].iter().filter_map(|n| cat.commands.iter().find(|(c, _)| c == n)).collect()
        } else {
            let mut h: Vec<&(String, String)> = cat.commands.iter().filter(|(n, _)| n.starts_with(q)).collect();
            h.sort_by_key(|(n, _)| n.len());
            h.extend(cat.commands.iter().filter(|(n, _)| !n.starts_with(q) && n.contains(q)));
            h
        };
        hits.truncate(8);
        hits
    }

    /// The rest of the word being typed, when only one thing fits.
    fn ghost(&self, cat: &Catalog) -> String {
        if self.cur != self.buf.len() {
            return String::new();
        }
        let t = self.text();
        let t = t.trim_start_matches('/');
        if t.is_empty() || t.ends_with(' ') {
            return String::new();
        }
        let ws = words_of(t);
        if ws.len() == 1 {
            let w = ws[0];
            let fits: Vec<&String> = cat.commands.iter().map(|(n, _)| n).filter(|n| n.starts_with(w)).collect();
            return match fits.as_slice() {
                [one] if one.len() > w.len() => one[w.len()..].to_owned(),
                _ => String::new(),
            };
        }
        let last = ws[ws.len() - 1];
        if last.contains('=') {
            return String::new();
        }
        let fields = (cat.fields)(ws[0]);
        let fits: Vec<&String> = fields.iter().filter(|f| f.starts_with(last) && !t.contains(&format!("{f}="))).collect();
        match fits.as_slice() {
            [one] => format!("{}=", &one[last.len()..]),
            _ => String::new(),
        }
    }

    /// The line under the box: the fields of the command being typed.
    fn hint(&self, cat: &Catalog) -> Option<Vec<(&'static str, String)>> {
        let t = self.text();
        let t = t.trim_start_matches('/');
        let ws = words_of(t);
        let cmd = *ws.first()?;
        let (_, about) = cat.commands.iter().find(|(n, _)| n == cmd)?;
        let fields = (cat.fields)(cmd);
        if fields.is_empty() {
            return Some(vec![("2", format!("  {about}"))]);
        }
        let given: Vec<&str> = ws[1..].iter().filter_map(|w| w.split_once('=').map(|(k, _)| k)).collect();
        let mut v = vec![("", "  ".to_owned())];
        for f in fields {
            v.push((if given.contains(&f.as_str()) { "2;9" } else { "36" }, f));
            v.push(("", " ".to_owned()));
        }
        v.push(("2", " · Tab completes".to_owned()));
        Some(v)
    }

    /// Up to the top of what was drawn last time, measured at the width the
    /// terminal has now (a narrower one has wrapped it).
    fn back_to_top(&self, w: usize) -> String {
        match &self.drawn {
            Some(d) => {
                let up = d.top.div_ceil(w).max(1) - 1 + 1 + d.col / w;
                format!("\r\x1b[{up}A\x1b[J")
            }
            None => "\r\x1b[J".to_owned(),
        }
    }

    fn draw(&mut self, cat: &Catalog, status: &str, p: &Paint) {
        let w = width().max(24);
        // Nothing reaches the last column, so nothing wraps.
        let bw = w - 1;
        let mut out = self.back_to_top(w);
        let room = bw - 6;
        out.push_str(&p.c("2", &format!("╭{}╮", "─".repeat(bw - 2))));
        out.push_str("\r\n");
        let start = self.cur.saturating_sub(room.saturating_sub(1));
        let shown: String = self.buf.iter().skip(start).take(room).collect();
        let n_shown = shown.chars().count();
        let ghost: String = self.ghost(cat).chars().take(room - n_shown).collect();
        let placeholder: String = if self.buf.is_empty() { "say what you want made, or / for commands".chars().take(room).collect() } else { String::new() };
        let used = n_shown + ghost.chars().count() + placeholder.chars().count();
        out.push_str(&format!("{} {} {}{}{}{}{}", p.c("2", "│"), p.c("36", "›"), shown, p.c("2", &ghost), p.c("2", &placeholder), " ".repeat(room - used), p.c("2", " │")));
        out.push_str("\r\n");
        out.push_str(&p.c("2", &format!("╰{}╯", "─".repeat(bw - 2))));
        // Under the box: the command menu, the fields, or the status.
        let mut rows: Vec<String> = Vec::new();
        let menu = self.menu(cat);
        if !menu.is_empty() {
            self.menu_sel = self.menu_sel.min(menu.len() - 1);
            for (i, (n, d)) in menu.iter().enumerate() {
                let name = format!("  /{n:<15} ");
                let line = if i == self.menu_sel { fit(&[("7", format!("{name}{d}"))], bw, p) } else { fit(&[("36", name), ("2", d.clone())], bw, p) };
                rows.push(line);
            }
        } else if let Some(h) = self.hint(cat) {
            rows.push(fit(&h.iter().map(|(c, t)| (*c, t.clone())).collect::<Vec<_>>(), bw, p));
        } else {
            rows.push(fit(&[("2", format!("  {status}"))], bw, p));
        }
        for r in &rows {
            out.push_str("\r\n");
            out.push_str(r);
        }
        // Back up to the cursor on the input line.
        let col = 4 + self.cur - start;
        out.push_str(&format!("\x1b[{}A\r\x1b[{col}C", rows.len() + 1));
        self.drawn = Some(Drawn { top: bw, col });
        print!("{out}");
        let _ = std::io::stdout().flush();
    }

    /// Clears the box and leaves the line as plain text in the scrollback.
    fn settle(&mut self, line: &str, p: &Paint) {
        let w = width().max(24);
        print!("{}{} {}\r\n", self.back_to_top(w), p.c("36", "›"), line);
        let _ = std::io::stdout().flush();
        self.drawn = None;
    }

    fn remember(&mut self, line: &str) {
        if line.trim().is_empty() || self.history.last().is_some_and(|l| l == line) {
            return;
        }
        self.history.push(line.to_owned());
        if let Some(f) = &self.history_file {
            let keep: Vec<&String> = self.history.iter().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect();
            let _ = std::fs::write(f, keep.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n") + "\n");
        }
    }

    /// Reads one line.
    pub fn read(&mut self, cat: &Catalog, status: &str, p: &Paint) -> Outcome {
        let Some(_raw) = Raw::on() else {
            // Not a terminal after all: a plain line.
            let mut s = String::new();
            return if std::io::stdin().read_line(&mut s).unwrap_or(0) == 0 { Outcome::Exit } else { Outcome::Line(s.trim().to_owned()) };
        };
        self.buf.clear();
        self.cur = 0;
        self.hist_at = None;
        self.menu_sel = 0;
        // Start the box on a fresh line.
        print!("\r\n");
        self.draw(cat, status, p);
        let mut ctrl_c = false;
        loop {
            let Some(k) = key() else {
                self.settle("", p);
                return Outcome::Exit;
            };
            if k != Key::CtrlC {
                ctrl_c = false;
            }
            let menu_len = self.menu(cat).len();
            match k {
                Key::Enter => {
                    let menu = self.menu(cat);
                    if let Some((n, _)) = menu.get(self.menu_sel) {
                        let typed = self.text();
                        if typed.trim_start_matches('/') != n.as_str() && !menu.is_empty() {
                            self.set(&format!("/{n} "));
                            self.draw(cat, status, p);
                            continue;
                        }
                    }
                    let line = self.text();
                    self.settle(&line, p);
                    self.remember(&line);
                    return Outcome::Line(line);
                }
                Key::Tab => {
                    let menu = self.menu(cat);
                    if let Some((n, _)) = menu.get(self.menu_sel) {
                        self.set(&format!("/{n} "));
                    } else {
                        let g = self.ghost(cat);
                        if !g.is_empty() {
                            let t = self.text() + &g;
                            let t = if g.ends_with('=') { t } else { t + " " };
                            self.set(&t);
                        }
                    }
                }
                Key::Char(c) => {
                    self.buf.insert(self.cur, c);
                    self.cur += 1;
                    self.menu_sel = 0;
                }
                Key::Backspace if self.cur > 0 => {
                    self.cur -= 1;
                    self.buf.remove(self.cur);
                    self.menu_sel = 0;
                }
                Key::Delete if self.cur < self.buf.len() => {
                    self.buf.remove(self.cur);
                }
                Key::Left => self.cur = self.cur.saturating_sub(1),
                Key::Right if self.cur < self.buf.len() => self.cur += 1,
                Key::Right => {
                    let g = self.ghost(cat);
                    if !g.is_empty() {
                        let t = self.text() + &g;
                        self.set(&t);
                    }
                }
                Key::Home => self.cur = 0,
                Key::End => self.cur = self.buf.len(),
                Key::WordLeft => {
                    while self.cur > 0 && self.buf[self.cur - 1] == ' ' {
                        self.cur -= 1;
                    }
                    while self.cur > 0 && self.buf[self.cur - 1] != ' ' {
                        self.cur -= 1;
                    }
                }
                Key::WordRight => {
                    while self.cur < self.buf.len() && self.buf[self.cur] == ' ' {
                        self.cur += 1;
                    }
                    while self.cur < self.buf.len() && self.buf[self.cur] != ' ' {
                        self.cur += 1;
                    }
                }
                Key::KillLine => {
                    self.buf.drain(..self.cur);
                    self.cur = 0;
                }
                Key::KillWord => {
                    let end = self.cur;
                    while self.cur > 0 && self.buf[self.cur - 1] == ' ' {
                        self.cur -= 1;
                    }
                    while self.cur > 0 && self.buf[self.cur - 1] != ' ' {
                        self.cur -= 1;
                    }
                    self.buf.drain(self.cur..end);
                }
                Key::Up if menu_len > 0 => self.menu_sel = (self.menu_sel + menu_len - 1) % menu_len,
                Key::Down if menu_len > 0 => self.menu_sel = (self.menu_sel + 1) % menu_len,
                Key::Up if !self.history.is_empty() => {
                    let i = match self.hist_at {
                        None => {
                            self.draft = self.text();
                            self.history.len() - 1
                        }
                        Some(i) => i.saturating_sub(1),
                    };
                    self.hist_at = Some(i);
                    let h = self.history[i].clone();
                    self.set(&h);
                }
                Key::Down => match self.hist_at {
                    Some(i) if i + 1 < self.history.len() => {
                        self.hist_at = Some(i + 1);
                        let h = self.history[i + 1].clone();
                        self.set(&h);
                    }
                    Some(_) => {
                        self.hist_at = None;
                        let d = self.draft.clone();
                        self.set(&d);
                    }
                    None => {}
                },
                Key::Esc => {
                    self.buf.clear();
                    self.cur = 0;
                }
                Key::CtrlC => {
                    if self.buf.is_empty() {
                        if ctrl_c {
                            self.settle("", p);
                            return Outcome::Exit;
                        }
                        ctrl_c = true;
                        self.draw(cat, "press Ctrl-C again to leave", p);
                        continue;
                    }
                    self.buf.clear();
                    self.cur = 0;
                }
                Key::CtrlD if self.buf.is_empty() => {
                    self.settle("", p);
                    return Outcome::Exit;
                }
                Key::CtrlL => {
                    print!("\x1b[2J\x1b[H\r\n");
                    self.drawn = None;
                }
                _ => {}
            }
            self.draw(cat, status, p);
        }
    }
}
