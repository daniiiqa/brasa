//! La tarjeta, armada como los avisos de arre: un recuadro con el look en una
//! cuadrícula de celdas (ratatui) que luego disco pasa a pixeles.

use crate::Kind;
use disco::look::Look;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Ancho de la tarjeta, en celdas.
pub const COLS: u16 = 36;

fn icon(kind: Kind, level: Option<u32>) -> &'static str {
    let l = level.unwrap_or(0);
    match (kind, level) {
        (Kind::Volume, None) => "󰝟",
        (Kind::Volume, _) if l == 0 => "󰝟",
        (Kind::Volume, _) if l <= 30 => "󰕿",
        (Kind::Volume, _) if l <= 60 => "󰖀",
        (Kind::Volume, _) => "󰕾",
        (Kind::Mic, None) => "󰍭",
        (Kind::Mic, _) => "󰍬",
        (Kind::Brightness, _) if l <= 30 => "󰃞",
        (Kind::Brightness, _) if l <= 70 => "󰃟",
        (Kind::Brightness, _) => "󰃠",
        (Kind::Kbd, _) => "󰌌",
    }
}

/// `frac`: lo que se ve de la barra ahora (va deslizándose a `level`).
pub fn card(kind: Kind, level: Option<u32>, title: &str, frac: f64, look: &Look) -> Buffer {
    let t = look.theme;
    let area = Rect::new(0, 0, COLS, 3);
    let mut buf = Buffer::empty(area);
    buf.set_style(area, Style::new().bg(t.base).fg(t.fg));
    let muted = level.is_none();
    let head = Span::styled(format!(" {} ", look.tx(title)), Style::new().fg(if muted { t.muted } else { t.primary }).bold());
    let block = look.block(false).title(head);
    let inner = block.inner(area);
    block.render(area, &mut buf);

    let mut spans = Vec::new();
    let mut used = 2; // un espacio a cada lado
    if look.icons {
        spans.push(Span::styled(format!("{} ", icon(kind, level)), Style::new().fg(if muted { t.red } else { t.primary })));
        used += 2;
    }
    let num = match level {
        None => format!(" {:>4}", "✕"),
        Some(l) => format!(" {l:>3}%"),
    };
    used += num.chars().count();
    let (done, rest) = look.bar(frac, (inner.width as usize).saturating_sub(used));
    spans.push(Span::styled(done, Style::new().fg(if muted { t.border } else { t.primary })));
    spans.push(Span::styled(rest, Style::new().fg(t.border)));
    // con boost (más de 100) el número en rojo, como aviso
    let hot = level.is_some_and(|l| l > 100);
    spans.push(Span::styled(num, Style::new().fg(if muted || hot { t.red } else { t.fg }).bold()));
    let line = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    Paragraph::new(Line::from(spans)).render(line, &mut buf);
    buf
}
