use std::io::{Read as _, Write};
use std::num::NonZeroU16;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui_kit::{
    App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, Font, FontWeight, HighlightStyle,
    Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke, MouseButton,
    ParentElement as _, Pixels, Render, ScrollEvent, SharedString, Styled as _,
    StyledText, Task, Window, canvas, div, font, prelude::FluentBuilder as _, px,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::theme::{MONO_FONT, Palette, c};

const LINE_HEIGHT_RATIO: f32 = 1.36;
const SCROLLBACK: usize = 2_000;
const PADDING: f32 = 8.;

struct Pty {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser>>,
    pty: Option<Pty>,
    focus: FocusHandle,
    size: (u16, u16),
    cell_width: Option<Pixels>,
    font_size: f32,
    scroll_remainder: f32,
    exited: bool,
    _pump: Option<Task<()>>,
}

impl Terminal {
    pub fn new(cwd: &Path, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let size = (24, 80);
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            NonZeroU16::new(size.0).unwrap(),
            NonZeroU16::new(size.1).unwrap(),
            SCROLLBACK,
        )));
        let mut this = Terminal {
            parser,
            pty: None,
            focus: cx.focus_handle(),
            size,
            cell_width: None,
            font_size: crate::settings::Settings::get(cx).terminal_font_size,
            scroll_remainder: 0.,
            exited: false,
            _pump: None,
        };
        if let Err(err) = this.spawn(cwd, cx) {
            this.exited = true;
            this.parser.lock().unwrap().process(format!("forge: failed to start shell: {err}\r\n").as_bytes());
        }
        this
    }

    fn spawn(&mut self, cwd: &Path, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let pair = native_pty_system().openpty(PtySize {
            rows: self.size.0,
            cols: self.size.1,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut command = CommandBuilder::new(shell);
        command.arg("-l");
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "Forge");
        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, rx) = flume::bounded::<()>(1);
        let parser = self.parser.clone();
        std::thread::Builder::new().name("forge-pty".into()).spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        parser.lock().unwrap().process(&buf[..n]);
                        let _ = tx.try_send(());
                    }
                }
            }
        })?;

        self._pump = Some(cx.spawn(async move |this, cx| {
            let mut first = true;
            while rx.recv_async().await.is_ok() {
                if std::mem::take(&mut first) {
                    crate::startup::mark("terminal: first shell output");
                }
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.exited = true;
                cx.notify();
            });
        }));
        self.pty = Some(Pty { writer, master: pair.master, child });
        Ok(())
    }

    pub fn restart(&mut self, cwd: &Path, cx: &mut Context<Self>) {
        self.kill();
        self.exited = false;
        {
            let mut parser = self.parser.lock().unwrap();
            *parser = vt100::Parser::new(
                NonZeroU16::new(self.size.0).unwrap(),
                NonZeroU16::new(self.size.1).unwrap(),
                SCROLLBACK,
            );
        }
        if let Err(err) = self.spawn(cwd, cx) {
            self.exited = true;
            self.parser.lock().unwrap().process(format!("forge: failed to start shell: {err}\r\n").as_bytes());
        }
        cx.notify();
    }

    fn kill(&mut self) {
        if let Some(mut pty) = self.pty.take() {
            let running = matches!(pty.child.try_wait(), Ok(None));
            if let Err(err) = pty.child.kill() {
                if running {
                    eprintln!("forge: cannot stop the shell: {err}");
                }
            }
        }
        self._pump = None;
    }

    fn write(&mut self, bytes: &[u8]) {
        if let Some(pty) = self.pty.as_mut() {
            if let Err(err) = pty.writer.write_all(bytes).and_then(|_| pty.writer.flush()) {
                eprintln!("forge: terminal write failed: {err}");
            }
        }
    }

    fn resize(&mut self, bounds: Bounds<Pixels>, cell_width: Pixels, cx: &mut Context<Self>) {
        let width = (bounds.size.width - px(PADDING * 2.)).max(px(0.));
        let height = (bounds.size.height - px(PADDING * 2.)).max(px(0.));
        let cols = ((width / cell_width).floor() as u16).max(10);
        let rows = ((height / px(self.line_height())).floor() as u16).max(2);
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap());
        if let Some(pty) = self.pty.as_ref() {
            let _ = pty.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.paste(&text);
                        cx.stop_propagation();
                    }
                }
                "c" => {
                    let contents = self.parser.lock().unwrap().screen().contents();
                    cx.write_to_clipboard(ClipboardItem::new_string(contents));
                    cx.stop_propagation();
                }
                "k" => {
                    self.parser.lock().unwrap().screen_mut().clear_scrollback();
                    self.write(b"\x0c");
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }
        let app_cursor = self.parser.lock().unwrap().screen().application_cursor();
        if let Some(bytes) = keystroke_bytes(keystroke, app_cursor) {
            self.parser.lock().unwrap().screen_mut().set_scrollback(0);
            self.write(&bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn line_height(&self) -> f32 {
        (self.font_size * LINE_HEIGHT_RATIO).round()
    }

    fn paste(&mut self, text: &str) {
        let bracketed = self.parser.lock().unwrap().screen().bracketed_paste();
        let text = paste_text(text, bracketed);
        if bracketed {
            self.write(b"\x1b[200~");
            self.write(text.as_bytes());
            self.write(b"\x1b[201~");
        } else {
            self.write(text.as_bytes());
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(self.line_height())).y;
        self.scroll_remainder += f32::from(delta) / self.line_height();
        let lines = self.scroll_remainder.trunc() as i64;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;
        let mut parser = self.parser.lock().unwrap();
        if parser.screen().alternate_screen() {
            drop(parser);
            let key: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            for _ in 0..lines.unsigned_abs().min(10) {
                self.write(key);
            }
            return;
        }
        let current = parser.screen().scrollback() as i64;
        parser.screen_mut().set_scrollback((current + lines).max(0) as usize);
        cx.notify();
    }

    fn cell_width(&mut self, window: &mut Window, cx: &App) -> Pixels {
        let font_size = crate::settings::Settings::get(cx).terminal_font_size;
        if font_size != self.font_size {
            self.font_size = font_size;
            self.cell_width = None;
            self.size = (0, 0);
        }
        if let Some(width) = self.cell_width {
            return width;
        }
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&mono_font());
        let width = text_system
            .advance(font_id, px(self.font_size), 'm')
            .map(|size| size.width)
            .unwrap_or(px(self.font_size * 0.6));
        self.cell_width = Some(width);
        width
    }

    fn render_rows(&self, focused: bool) -> Vec<StyledText> {
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let (rows, cols) = (self.size.0, self.size.1);
        let (cursor_row, cursor_col) = screen.cursor_position();
        let show_cursor = !screen.hide_cursor() && screen.scrollback() == 0 && !self.exited;

        (0..rows)
            .map(|row| {
                let mut text = String::with_capacity(cols as usize);
                let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else { break };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let start = text.len();
                    if cell.has_contents() {
                        text.push_str(cell.contents());
                    } else {
                        text.push(' ');
                    }
                    let is_cursor = show_cursor && row == cursor_row && col == cursor_col;
                    let style = cell_style(cell, is_cursor, focused);
                    if let Some(style) = style {
                        let range = start..text.len();
                        match highlights.last_mut() {
                            Some((last, prev)) if last.end == range.start && *prev == style => {
                                last.end = range.end;
                            }
                            _ => highlights.push((range, style)),
                        }
                    }
                }
                StyledText::new(SharedString::from(text)).with_highlights(highlights)
            })
            .collect()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cell_width = self.cell_width(window, cx);
        let line_height = self.line_height();
        let focused = self.focus.is_focused(window);
        let rows = self.render_rows(focused);
        let this = cx.entity();

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(Palette::panel_bg())
            .p(px(PADDING))
            .font_family(MONO_FONT)
            .text_size(px(self.font_size))
            .line_height(px(line_height))
            .text_color(Palette::text())
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        this.update(cx, |term, cx| term.resize(bounds, cell_width, cx));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(rows.into_iter().map(|row| div().h(px(line_height)).whitespace_nowrap().child(row)))
            .when(self.exited, |el| {
                el.child(
                    div()
                        .text_color(Palette::muted())
                        .child("[process exited — press the restart button]"),
                )
            })
    }
}

fn paste_text(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { text.replace("\x1b[201~", "").replace("\x1b[200~", "") } else { text }
}

fn mono_font() -> Font {
    font(MONO_FONT)
}

fn cell_style(cell: &vt100::Cell, is_cursor: bool, focused: bool) -> Option<HighlightStyle> {
    let mut fg = color(cell.fgcolor(), true);
    let mut bg = color(cell.bgcolor(), false);
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
        fg = fg.or(Some(Palette::panel_bg()));
        bg = bg.or(Some(Palette::text()));
    }
    if is_cursor {
        if focused {
            bg = Some(c(0xd0d0d0));
            fg = Some(Palette::panel_bg());
        } else {
            bg = Some(Palette::faint());
        }
    }
    if cell.dim() {
        fg = Some(fg.unwrap_or(Palette::text()).opacity(0.6));
    }
    let bold = cell.bold();
    let italic = cell.italic();
    let underline = cell.underline();
    if fg.is_none() && bg.is_none() && !bold && !italic && !underline {
        return None;
    }
    Some(HighlightStyle {
        color: fg,
        background_color: bg,
        font_weight: bold.then_some(FontWeight::BOLD),
        font_style: italic.then_some(gpui_kit::FontStyle::Italic),
        underline: underline.then(|| gpui_kit::UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        }),
        ..Default::default()
    })
}

const ANSI: [u32; 16] = [
    0x1f1f24, 0xff6b6b, 0x5fc27a, 0xd9a54a, 0x4f9dff, 0xc678dd, 0x56b6c2, 0xd0d0d0,
    0x6c6c72, 0xff8787, 0x7ee09a, 0xffcb6b, 0x82b1ff, 0xd7a1f0, 0x89ddff, 0xffffff,
];

fn color(color: vt100::Color, _foreground: bool) -> Option<Hsla> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Rgb(r, g, b) => Some(c(((r as u32) << 16) | ((g as u32) << 8) | b as u32)),
        vt100::Color::Idx(i) => Some(c(xterm_256(i))),
    }
}

fn xterm_256(i: u8) -> u32 {
    match i {
        0..=15 => ANSI[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        232..=255 => {
            let v = 8 + (i as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}

fn keystroke_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = &keystroke.modifiers;
    let arrow = |code: u8| -> Vec<u8> {
        if mods.alt || mods.shift || mods.control {
            let m = 1 + (mods.shift as u8) + (mods.alt as u8) * 2 + (mods.control as u8) * 4;
            format!("\x1b[1;{m}{}", code as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', code]
        } else {
            vec![0x1b, b'[', code]
        }
    };
    let bytes = match keystroke.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" if mods.alt => b"\x1b\x7f".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "tab" if mods.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => arrow(b'A'),
        "down" => arrow(b'B'),
        "right" => arrow(b'C'),
        "left" => arrow(b'D'),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "space" if mods.control => vec![0],
        "space" => b" ".to_vec(),
        key if mods.control && key.len() == 1 => {
            let ch = key.as_bytes()[0].to_ascii_lowercase();
            match ch {
                b'a'..=b'z' => vec![ch - b'a' + 1],
                b'[' => vec![0x1b],
                b'\\' => vec![0x1c],
                b']' => vec![0x1d],
                _ => return None,
            }
        }
        _ => {
            let text = keystroke.key_char.as_deref().or_else(|| {
                (keystroke.key.chars().count() == 1).then_some(keystroke.key.as_str())
            })?;
            let mut bytes = Vec::new();
            if mods.alt && keystroke.key_char.is_none() {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_cannot_be_terminated_by_the_payload() {
        let hostile = "echo hi\x1b[201~rm -rf ~\n";
        assert_eq!(paste_text(hostile, true), "echo hirm -rf ~\r");
        assert_eq!(paste_text("a\r\nb\nc", false), "a\rb\rc");
    }

    #[test]
    fn xterm_cube_and_gray() {
        assert_eq!(xterm_256(16), 0x000000);
        assert_eq!(xterm_256(231), 0xffffff);
        assert_eq!(xterm_256(232), 0x080808);
    }
}
