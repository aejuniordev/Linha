//! Tela "Novo evento": cria um evento no calendário principal (ou em outro escolhido).

use std::sync::{Arc, Mutex};

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike, Utc};
use eframe::egui::{self, Align, Button, Color32, Key, Layout, RichText, Ui};

use super::settings::{card, hint, pal_blue, row, section};
use super::theme::{font, font_named, Pal, MEDIUM, SEMIBOLD};
use crate::config::{Config, Principal};
use crate::model::{CalendarInfo, NewEvent, Source, When};
use crate::providers;

/// Passo da lista de horários, em minutos.
const STEP: u32 = 15;

type Slot = Arc<Mutex<Option<Result<(), String>>>>;

#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    /// Evento criado: vale sincronizar.
    pub saved: bool,
    /// A pessoa escolheu um novo calendário principal.
    pub principal: Option<Principal>,
}

pub struct Compose {
    title: String,
    location: String,
    date: jiff::civil::Date,
    all_day: bool,
    /// Minutos desde a meia-noite (o fim pode ser 1440 = meia-noite seguinte).
    start: u32,
    end: u32,
    /// Calendário escolhido: (origem, id).
    target: Option<(Source, String)>,
    make_principal: bool,
    pending: Option<Slot>,
    error: Option<String>,
    focus: bool,
}

impl Compose {
    pub fn new(cfg: &Config, day: NaiveDate) -> Compose {
        let now = Local::now();
        let next_hour = if day == now.date_naive() { (now.hour() + 1).min(23) * 60 } else { 9 * 60 };
        Compose {
            title: String::new(),
            location: String::new(),
            date: to_jiff(day),
            all_day: false,
            start: next_hour,
            end: next_hour + 60,
            target: cfg.principal.as_ref().map(|p| (p.source, p.id.clone())),
            make_principal: cfg.principal.is_none(),
            pending: None,
            error: None,
            focus: true,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, pal: &Pal, cfg: &Config, calendars: &[CalendarInfo]) -> Outcome {
        let mut out = Outcome::default();
        let ctx = ui.ctx().clone();

        // resultado do envio
        if let Some(res) = self.pending.as_ref().and_then(|p| p.lock().unwrap().take()) {
            self.pending = None;
            match res {
                Ok(()) => {
                    out.saved = true;
                    out.close = true;
                    if self.make_principal {
                        out.principal = self.principal(calendars);
                    }
                    return out;
                }
                Err(e) => self.error = Some(e),
            }
        }
        let busy = self.pending.is_some();

        let writable: Vec<&CalendarInfo> =
            calendars.iter().filter(|c| c.writable && providers::enabled(cfg, c.source)).collect();
        let valid = |t: Option<(Source, &str)>| t.is_some_and(|(s, id)| writable.iter().any(|c| c.source == s && c.id == id));
        if !valid(self.target.as_ref().map(|(s, id)| (*s, id.as_str()))) {
            // sem escolha válida (conta desconectada, lista ainda vazia…): principal, senão o primeiro
            let principal = cfg.principal.as_ref().map(|p| (p.source, p.id.as_str()));
            self.target = if valid(principal) {
                principal.map(|(s, id)| (s, id.to_string()))
            } else {
                writable.first().map(|c| (c.source, c.id.clone()))
            };
        }

        // ---- barra do topo ----
        ui.horizontal(|ui| {
            ui.label(RichText::new("Novo evento").font(font_named(22.0, SEMIBOLD)).color(pal.primary));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!busy, Button::new("Cancelar")).clicked() {
                    out.close = true;
                }
            });
        });
        ui.add_space(10.0);

        let mut submit = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_enabled_ui(!busy, |ui| {
                card(ui, pal, |ui| {
                    let t = ui.add(
                        egui::TextEdit::singleline(&mut self.title)
                            .hint_text("Título")
                            .font(font_named(16.0, MEDIUM))
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.focus) {
                        t.request_focus();
                    }
                    submit |= t.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add(egui::TextEdit::singleline(&mut self.location).hint_text("Local (opcional)").desired_width(f32::INFINITY));
                });

                section(ui, pal, "Quando");
                card(ui, pal, |ui| {
                    row(ui, pal, "Dia", |ui| {
                        ui.add(
                            egui_extras::DatePickerButton::new(&mut self.date)
                                .id_salt("dia")
                                .format("%d/%m/%Y")
                                .calendar_week(false)
                                .combo_boxes(false),
                        );
                    });
                    ui.checkbox(&mut self.all_day, "Dia inteiro");
                    if !self.all_day {
                        let before = self.start;
                        row(ui, pal, "Começa", |ui| time_combo(ui, "inicio", &mut self.start, 0, 24 * 60 - STEP));
                        if self.start != before {
                            // mantém a duração ao mudar o início
                            let dur = self.end.saturating_sub(before).max(STEP);
                            self.end = (self.start + dur).min(24 * 60);
                        }
                        let first_end = self.start + STEP;
                        row(ui, pal, "Termina", |ui| time_combo(ui, "fim", &mut self.end, first_end, 24 * 60));
                        self.end = self.end.max(first_end);
                    }
                });

                section(ui, pal, "Calendário");
                card(ui, pal, |ui| {
                    if writable.is_empty() {
                        hint(ui, pal, "Nenhum calendário disponível para escrita. Conecte uma conta em Ajustes e sincronize. Se o Outlook ou o Google já estavam conectados, conecte de novo para dar permissão de criar eventos.");
                        return;
                    }
                    let current = self
                        .target
                        .as_ref()
                        .and_then(|(s, id)| writable.iter().find(|c| c.source == *s && &c.id == id))
                        .map(|c| label(c))
                        .unwrap_or_else(|| "Escolha…".into());
                    egui::ComboBox::from_id_salt("calendario").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                        for c in &writable {
                            let sel = self.target.as_ref().is_some_and(|(s, id)| *s == c.source && id == &c.id);
                            if ui.selectable_label(sel, label(c)).clicked() {
                                self.target = Some((c.source, c.id.clone()));
                            }
                        }
                    });
                    let is_principal = cfg.principal.as_ref().is_some_and(|p| {
                        self.target.as_ref().is_some_and(|(s, id)| p.source == *s && &p.id == id)
                    });
                    if !is_principal {
                        ui.checkbox(&mut self.make_principal, "Usar como calendário principal");
                    }
                    if let Some((s, _)) = &self.target {
                        if !providers::can_write(*s) {
                            hint(ui, pal, &format!("Conecte o {} de novo em Ajustes para permitir criar eventos.", s.label()));
                        }
                    }
                });
            });

            if let Some(e) = &self.error {
                ui.label(RichText::new(e).font(font(12.5)).color(pal.red));
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = !self.title.trim().is_empty() && self.target.is_some() && !busy;
                let b = Button::new(RichText::new("Salvar").font(font_named(14.5, MEDIUM)).color(Color32::WHITE))
                    .fill(pal_blue(pal))
                    .min_size(egui::vec2(96.0, 30.0));
                if ui.add_enabled(ready, b).clicked() {
                    submit = true;
                }
                if busy {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(RichText::new("Salvando…").font(font(12.5)).color(pal.secondary));
                }
            });
        });

        if submit && !busy && !self.title.trim().is_empty() {
            self.save(cfg, &ctx);
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        out
    }

    fn principal(&self, calendars: &[CalendarInfo]) -> Option<Principal> {
        let (s, id) = self.target.as_ref()?;
        let c = calendars.iter().find(|c| c.source == *s && &c.id == id)?;
        Some(Principal { source: c.source, id: c.id.clone(), name: c.name.clone() })
    }

    fn save(&mut self, cfg: &Config, ctx: &egui::Context) {
        let Some((src, id)) = self.target.clone() else { return };
        let day = from_jiff(self.date);
        let when = if self.all_day {
            When::AllDay { start: day, end: day + Duration::days(1) }
        } else {
            let at = |mins: u32| {
                let midnight = day.and_hms_opt(0, 0, 0).expect("meia-noite");
                let local = midnight + Duration::minutes(mins as i64);
                Local.from_local_datetime(&local).earliest().map(|d| d.with_timezone(&Utc))
            };
            match (at(self.start), at(self.end)) {
                (Some(start), Some(end)) if end > start => When::Timed { start, end },
                _ => {
                    self.error = Some("Horário inválido: o fim precisa ser depois do início.".into());
                    return;
                }
            }
        };
        let ev = NewEvent {
            title: self.title.trim().to_string(),
            when,
            location: Some(self.location.trim().to_string()).filter(|s| !s.is_empty()),
        };
        self.error = None;
        let slot: Slot = Arc::default();
        let (s, cfg, ctx) = (slot.clone(), cfg.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = providers::create(&cfg, src, &id, &ev).map_err(|e| format!("{e:#}"));
            *s.lock().unwrap() = Some(res);
            ctx.request_repaint();
        });
        self.pending = Some(slot);
    }
}

fn label(c: &CalendarInfo) -> String {
    format!("{} · {}", c.name, c.source.label())
}

fn hm(mins: u32) -> String {
    format!("{:02}:{:02}", (mins / 60) % 24, mins % 60)
}

fn time_combo(ui: &mut Ui, id: &str, value: &mut u32, first: u32, last: u32) {
    egui::ComboBox::from_id_salt(id).selected_text(hm(*value)).width(84.0).height(240.0).show_ui(ui, |ui| {
        let mut m = first;
        while m <= last {
            ui.selectable_value(value, m, hm(m));
            m += STEP;
        }
    });
}

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).expect("data válida")
}

fn from_jiff(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32).expect("data válida")
}
