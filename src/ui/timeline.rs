//! A lista de dias/eventos, desenhada à mão para ficar com cara de iOS.

use std::collections::HashMap;

use chrono::{Duration, Local};
use eframe::egui::{
    self, epaint::Shadow, pos2, text::LayoutJob, vec2, Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense,
    Stroke, StrokeKind, Ui,
};

use super::anim::{ease, progress};
use super::fmt;
use super::theme::{fade, font, font_named, rgb, tint, Pal, MEDIUM, SEMIBOLD};
use crate::model::{Cache, Event};

pub struct View<'a> {
    pub pal: Pal,
    pub alpha: f32,
    pub now: f64,
    pub opened: f64,
    pub first_seen: &'a HashMap<String, f64>,
    pub cache: &'a Cache,
    pub days: u32,
}

const TIME_W: f32 = 44.0;
const RAIL_GAP: f32 = 12.0;
const CARD_GAP: f32 = 16.0;
const ROW_GAP: f32 = 10.0;

struct Cols {
    rail: f32,
    card: f32,
    right: f32,
}

impl View<'_> {
    /// Opacidade e deslocamento vertical de uma linha (entrada escalonada).
    fn row_anim(&self, row: usize, key: Option<&str>) -> (f32, f32) {
        let seen = key.and_then(|k| self.first_seen.get(k)).copied().unwrap_or(self.opened);
        let (base, delay) = if seen > self.opened {
            (seen, 0.0) // chegou numa sincronização depois de abrir: só um fade
        } else {
            (self.opened, 0.22 + row.min(14) as f64 * 0.045)
        };
        let p = ease(progress(self.now, base, delay, 0.48));
        (p * self.alpha, (1.0 - p) * 18.0)
    }
}

fn layout(ui: &Ui, text: &str, font_id: egui::FontId, color: Color32, width: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple(text.to_owned(), font_id, color, width.max(10.0));
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = rows == 1;
    ui.painter().layout_job(job)
}

pub fn show(ui: &mut Ui, v: &View) {
    let today = Local::now().date_naive();
    let now = Local::now();
    let full = ui.available_rect_before_wrap();
    let cols = Cols {
        rail: full.left() + TIME_W + RAIL_GAP,
        card: full.left() + TIME_W + RAIL_GAP + CARD_GAP,
        right: full.right() - 2.0,
    };
    let mut row = 0usize;

    for d in 0..v.days.max(1) {
        let day = today + Duration::days(d as i64);
        let events = v.cache.events_on(day);
        let (all_day, timed): (Vec<&Event>, Vec<&Event>) = events.iter().partition(|e| e.is_all_day());

        if d > 0 {
            ui.add_space(14.0);
        }
        section_header(ui, v, row, &fmt::day_title(day, today), &fmt::short_date(day));
        row += 1;

        if !all_day.is_empty() {
            all_day_pills(ui, v, row, &cols, &all_day);
            row += 1;
        }

        if timed.is_empty() {
            if all_day.is_empty() {
                empty_day(ui, v, row, &cols);
                row += 1;
            }
            continue;
        }

        let mut now_drawn = d != 0;
        for (i, e) in timed.iter().enumerate() {
            if !now_drawn && e.local_start() > now {
                now_line(ui, v, row, &cols);
                row += 1;
                now_drawn = true;
            }
            let last = i + 1 == timed.len() && now_drawn;
            event_row(ui, v, row, &cols, e, last);
            row += 1;
        }
        if !now_drawn {
            now_line(ui, v, row, &cols);
            row += 1;
        }
    }
    ui.add_space(24.0);
}

fn section_header(ui: &mut Ui, v: &View, row: usize, title: &str, date: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let (a, dy) = v.row_anim(row, None);
    let p = ui.painter();
    let t = layout(ui, title, font_named(21.0, SEMIBOLD), fade(v.pal.primary, a), 300.0, 1);
    let base_y = rect.bottom() - 10.0 + dy;
    p.galley(pos2(rect.left(), base_y - t.size().y), t, Color32::PLACEHOLDER);
    let s = layout(ui, date, font(13.5), fade(v.pal.secondary, a), 200.0, 1);
    p.galley(pos2(rect.right() - s.size().x - 2.0, base_y - s.size().y - 2.0), s, Color32::PLACEHOLDER);
}

fn all_day_pills(ui: &mut Ui, v: &View, row: usize, c: &Cols, events: &[&Event]) {
    const H: f32 = 28.0;
    const GAP: f32 = 6.0;
    let max_w = c.right - c.card;
    // calcula as posições (quebra de linha automática)
    let mut items = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for e in events {
        let g = layout(ui, &e.title, font_named(13.5, MEDIUM), Color32::PLACEHOLDER, max_w - 34.0, 1);
        let w = (g.size().x + 34.0).min(max_w);
        if x > 0.0 && x + w > max_w {
            x = 0.0;
            y += H + GAP;
        }
        items.push((x, y, w, g, *e));
        x += w + GAP;
    }
    let total_h = y + H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h + ROW_GAP), Sense::hover());
    let (a, dy) = v.row_anim(row, None);
    let p = ui.painter();

    let label = layout(ui, "dia todo", font(11.5), fade(v.pal.secondary, a), TIME_W + 8.0, 1);
    p.galley(pos2(c.rail - RAIL_GAP - label.size().x, rect.top() + dy + (H - label.size().y) / 2.0), label, Color32::PLACEHOLDER);

    for (x, y, w, g, e) in items {
        let r = Rect::from_min_size(pos2(c.card + x, rect.top() + y + dy), vec2(w, H));
        let col = rgb(e.color);
        p.rect_filled(r, CornerRadius::same((H / 2.0) as u8), fade(tint(v.pal.card, col, if v.pal.dark { 0.28 } else { 0.16 }), a));
        p.circle_filled(pos2(r.left() + 14.0, r.center().y), 3.5, fade(col, a));
        p.galley(pos2(r.left() + 24.0, r.center().y - g.size().y / 2.0), g, fade(v.pal.primary, a));
    }
}

fn empty_day(ui: &mut Ui, v: &View, row: usize, c: &Cols) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0 + ROW_GAP), Sense::hover());
    let (a, dy) = v.row_anim(row, None);
    let p = ui.painter();
    let r = Rect::from_min_max(pos2(c.card, rect.top() + dy), pos2(c.right, rect.top() + dy + 64.0));
    p.rect_filled(r, CornerRadius::same(14), fade(v.pal.card, a * 0.7));
    let t = layout(ui, "Dia livre", font_named(15.0, MEDIUM), fade(v.pal.primary, a), r.width(), 1);
    let s = layout(ui, "Nada marcado por aqui.", font(13.0), fade(v.pal.secondary, a), r.width(), 1);
    let ty = r.center().y - (t.size().y + s.size().y + 2.0) / 2.0;
    p.galley(pos2(r.left() + 16.0, ty), t.clone(), Color32::PLACEHOLDER);
    p.galley(pos2(r.left() + 16.0, ty + t.size().y + 2.0), s, Color32::PLACEHOLDER);
    p.circle_filled(pos2(c.rail, r.center().y), 3.5, fade(v.pal.tertiary, a));
}

fn now_line(ui: &mut Ui, v: &View, row: usize, c: &Cols) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let (a, dy) = v.row_anim(row, Some("__agora"));
    let p = ui.painter();
    let y = rect.center().y - ROW_GAP / 4.0 + dy;
    let red = v.pal.red;

    // a linha "cresce" da bolinha para a direita
    let grow = ease(progress(v.now, v.opened, 0.45 + row.min(14) as f64 * 0.045, 0.7));
    let x_end = c.rail + (c.right - c.rail) * grow;
    p.line_segment([pos2(c.rail, y), pos2(x_end, y)], Stroke::new(1.25, fade(red, a)));

    // dois "pings" suaves ao abrir
    let t = v.now - v.opened - 0.8;
    if (0.0..3.2).contains(&t) {
        let k = ((t % 1.6) / 1.6) as f32;
        p.circle_stroke(pos2(c.rail, y), 5.0 + 11.0 * ease(k), Stroke::new(1.5, fade(red, a * (1.0 - k) * 0.55)));
    }
    p.circle_filled(pos2(c.rail, y), 5.0, fade(red, a));

    let label = layout(ui, &fmt::hm(Local::now()), font_named(12.5, SEMIBOLD), fade(red, a), TIME_W + 8.0, 1);
    p.galley(pos2(c.rail - RAIL_GAP - label.size().x, y - label.size().y / 2.0), label, Color32::PLACEHOLDER);
}

fn event_row(ui: &mut Ui, v: &View, row: usize, c: &Cols, e: &Event, last: bool) {
    let now = Local::now();
    let (start, end) = (e.local_start(), e.local_end());
    let ongoing = start <= now && now < end;
    let past = end <= now;
    let col = rgb(e.color);
    let pal = v.pal;

    let card_w = c.right - c.card;
    let chip_w = if ongoing { 52.0 } else { 0.0 };
    let text_x = 20.0;
    let title = layout(ui, &e.title, font_named(15.5, SEMIBOLD), Color32::PLACEHOLDER, card_w - text_x - 14.0 - chip_w, 2);
    let mut sub_parts: Vec<String> = Vec::new();
    if let Some(l) = &e.location {
        sub_parts.push(l.lines().next().unwrap_or_default().to_string());
    }
    sub_parts.push(e.calendar.clone());
    let sub = layout(ui, &sub_parts.join("  ·  "), font(13.0), Color32::PLACEHOLDER, card_w - text_x - 14.0, 1);

    let pad_y = 11.0;
    let progress_h = if ongoing { 8.0 } else { 0.0 };
    let card_h = (pad_y + title.size().y + 3.0 + sub.size().y + pad_y + progress_h).max(58.0);

    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), card_h + ROW_GAP), Sense::hover());
    let (mut a, dy) = v.row_anim(row, Some(&e.id));
    if past {
        a *= 0.48;
    }
    let hover = ui.ctx().animate_bool(Id::unique(("hover", &e.id)), resp.hovered());
    let lift = hover * 1.5;
    let p = ui.painter();
    let top = rect.top() + dy;
    let card = Rect::from_min_size(pos2(c.card, top - lift), vec2(card_w, card_h));

    // trilho vertical
    if !last {
        p.line_segment([pos2(c.rail, top + 6.0), pos2(c.rail, top + card_h + ROW_GAP + 6.0)], Stroke::new(1.0, fade(pal.separator, v.alpha)));
    }

    // horário
    let t1 = layout(ui, &fmt::hm(start), font_named(13.5, MEDIUM), fade(pal.primary, a), TIME_W + 8.0, 1);
    let t2 = layout(ui, &fmt::hm(end), font(12.0), fade(pal.secondary, a), TIME_W + 8.0, 1);
    let tx = c.rail - RAIL_GAP;
    p.galley(pos2(tx - t1.size().x, top + 10.0), t1.clone(), Color32::PLACEHOLDER);
    p.galley(pos2(tx - t2.size().x, top + 10.0 + t1.size().y + 1.0), t2, Color32::PLACEHOLDER);

    // bolinha no trilho
    let dot = pos2(c.rail, top + 19.0);
    if ongoing {
        p.circle_filled(dot, 7.0, fade(tint(pal.bg, col, 0.25), a));
        p.circle_filled(dot, 4.5, fade(col, a));
    } else {
        p.circle_filled(dot, 4.0, fade(if past { pal.tertiary } else { col }, a));
    }

    // cartão
    if !pal.dark {
        let sh = Shadow { offset: [0, 2 + (hover * 3.0) as i8], blur: 14 + (hover * 8.0) as u8, spread: 0, color: fade(pal.shadow, a) };
        p.add(sh.as_shape(card, CornerRadius::same(14)));
    }
    let base = if hover > 0.0 { tint(pal.card, pal.card_hover, hover) } else { pal.card };
    let fill = if ongoing { tint(base, col, if pal.dark { 0.18 } else { 0.08 }) } else { base };
    p.rect_filled(card, CornerRadius::same(14), fade(fill, a));
    if pal.dark {
        p.rect_stroke(card, CornerRadius::same(14), Stroke::new(1.0, fade(Color32::from_white_alpha(10), a)), StrokeKind::Inside);
    }

    // barra de cor à esquerda
    let bar = Rect::from_min_max(pos2(card.left() + 9.0, card.top() + 10.0), pos2(card.left() + 12.5, card.bottom() - 10.0 - progress_h));
    p.rect_filled(bar, CornerRadius::same(2), fade(col, a));

    // textos
    let mut y = card.top() + pad_y;
    let title_h = title.size().y;
    p.galley(pos2(card.left() + text_x, y), title, fade(pal.primary, a));
    y += title_h + 3.0;
    p.galley(pos2(card.left() + text_x, y), sub, fade(pal.secondary, a));

    if ongoing {
        // selo "agora"
        let chip = Rect::from_min_size(pos2(card.right() - 12.0 - 44.0, card.top() + 11.0), vec2(44.0, 20.0));
        p.rect_filled(chip, CornerRadius::same(10), fade(col, a));
        p.text(chip.center(), Align2::CENTER_CENTER, "agora", font_named(11.5, SEMIBOLD), fade(Color32::WHITE, a));

        // progresso do evento
        let total = (end - start).num_seconds().max(1) as f32;
        let done = ((now - start).num_seconds() as f32 / total).clamp(0.0, 1.0);
        let grow = ease(progress(v.now, v.opened, 0.6, 0.9));
        let track = Rect::from_min_max(pos2(card.left() + text_x, card.bottom() - 12.0), pos2(card.right() - 14.0, card.bottom() - 9.0));
        p.rect_filled(track, CornerRadius::same(2), fade(tint(pal.card, col, 0.22), a));
        let mut fill_r = track;
        fill_r.set_width(track.width() * done * grow);
        p.rect_filled(fill_r, CornerRadius::same(2), fade(col, a));
    }

    if resp.hovered() {
        let mut tip = format!("{}\n{} – {}", e.title, fmt::hm(start), fmt::hm(end));
        if let Some(l) = &e.location {
            tip.push_str(&format!("\n{l}"));
        }
        tip.push_str(&format!("\n{} · {}", e.calendar, e.source.label()));
        resp.on_hover_text(tip);
    }
}

/// Esmaece a borda superior/inferior da área rolável (conteúdo "some" no fundo).
pub fn edge_fade(painter: &egui::Painter, rect: Rect, bg: Color32, h: f32) {
    let mut mesh = egui::Mesh::default();
    let clear = Color32::from_rgba_premultiplied(0, 0, 0, 0);
    let quad = |mesh: &mut egui::Mesh, top: f32, bottom: f32, ctop: Color32, cbot: Color32| {
        let i = mesh.vertices.len() as u32;
        let pts: [(Pos2, Color32); 4] = [
            (pos2(rect.left(), top), ctop),
            (pos2(rect.right(), top), ctop),
            (pos2(rect.right(), bottom), cbot),
            (pos2(rect.left(), bottom), cbot),
        ];
        for (pos, color) in pts {
            mesh.colored_vertex(pos, color);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    quad(&mut mesh, rect.top(), rect.top() + h, bg, clear);
    quad(&mut mesh, rect.bottom() - h, rect.bottom(), clear, bg);
    painter.add(mesh);
}
