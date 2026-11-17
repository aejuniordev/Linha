//! Tela "Novo evento": cria um evento no calendário principal (ou em outro escolhido).
//!
//! O formulário mostra o que os três serviços têm em comum (título, local,
//! descrição, horário, repetição, lembrete, disponibilidade, privacidade) e,
//! conforme o calendário escolhido, o que só alguns têm: convidados e reunião
//! online (Google e Outlook) e os demais estados de disponibilidade (Outlook).

use std::sync::{Arc, Mutex};

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc, Weekday};
use eframe::egui::{self, Align, Button, Color32, Key, Layout, RichText, Ui};

use super::settings::{card, hint, pal_blue, row, section};
use super::theme::{font, font_named, Pal, MEDIUM, SEMIBOLD};
use crate::config::{Config, Principal};
use crate::model::{CalendarInfo, Freq, NewEvent, Reminder, Repeat, RepeatEnd, ShowAs, Source, When};
use crate::providers;

/// Passo da lista de horários, em minutos.
const STEP: u32 = 15;

/// Dias da semana na ordem do calendário brasileiro (domingo primeiro).
const WEEK: [(Weekday, &str); 7] = [
    (Weekday::Sun, "Dom"),
    (Weekday::Mon, "Seg"),
    (Weekday::Tue, "Ter"),
    (Weekday::Wed, "Qua"),
    (Weekday::Thu, "Qui"),
    (Weekday::Fri, "Sex"),
    (Weekday::Sat, "Sáb"),
];

type Slot = Arc<Mutex<Option<Result<(), String>>>>;

#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    /// Evento criado: vale sincronizar.
    pub saved: bool,
    /// A pessoa escolheu um novo calendário principal.
    pub principal: Option<Principal>,
}

#[derive(Clone, Copy, PartialEq)]
enum EndKind {
    Never,
    Until,
    Count,
}

pub struct Compose {
    title: String,
    location: String,
    description: String,
    all_day: bool,
    start_date: jiff::civil::Date,
    end_date: jiff::civil::Date,
    /// Minutos desde a meia-noite.
    start: u32,
    end: u32,

    /// `None` = não repete.
    freq: Option<Freq>,
    interval: u32,
    weekdays: Vec<Weekday>,
    end_kind: EndKind,
    until: jiff::civil::Date,
    count: u32,

    reminder: Reminder,
    show_as: ShowAs,
    private: bool,
    attendees: String,
    meeting: bool,

    /// Calendário escolhido: (conta, id do calendário).
    target: Option<(String, String)>,
    make_principal: bool,
    pending: Option<Slot>,
    error: Option<String>,
    focus: bool,
}

impl Compose {
    pub fn new(cfg: &Config, day: NaiveDate) -> Compose {
        let now = Local::now();
        let next_hour = if day == now.date_naive() { (now.hour() + 1).min(23) * 60 } else { 9 * 60 };
        let (end_day, end) = if next_hour + 60 >= 24 * 60 { (day + Duration::days(1), 0) } else { (day, next_hour + 60) };
        Compose {
            title: String::new(),
            location: String::new(),
            description: String::new(),
            all_day: false,
            start_date: to_jiff(day),
            end_date: to_jiff(end_day),
            start: next_hour,
            end,
            freq: None,
            interval: 1,
            weekdays: vec![day.weekday()],
            end_kind: EndKind::Never,
            until: to_jiff(day + Duration::days(90)),
            count: 10,
            reminder: Reminder::Default,
            show_as: ShowAs::Busy,
            private: false,
            attendees: String::new(),
            meeting: false,
            target: cfg.principal.as_ref().map(|p| (p.account.clone(), p.id.clone())),
            make_principal: cfg.principal.is_none(),
            pending: None,
            error: None,
            focus: true,
        }
    }

    fn start_dt(&self) -> NaiveDateTime {
        at_minutes(from_jiff(self.start_date), self.start)
    }

    fn end_dt(&self) -> NaiveDateTime {
        at_minutes(from_jiff(self.end_date), self.end)
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
            calendars.iter().filter(|c| c.writable && cfg.active(&c.account)).collect();
        let valid = |t: Option<(&str, &str)>| t.is_some_and(|(a, id)| writable.iter().any(|c| c.account == a && c.id == id));
        if !valid(self.target.as_ref().map(|(a, id)| (a.as_str(), id.as_str()))) {
            // sem escolha válida (conta desconectada, lista ainda vazia…): principal, senão o primeiro
            let principal = cfg.principal.as_ref().map(|p| (p.account.as_str(), p.id.as_str()));
            self.target = if valid(principal) {
                principal.map(|(a, id)| (a.to_string(), id.to_string()))
            } else {
                writable.first().map(|c| (c.account.clone(), c.id.clone()))
            };
        }
        let target: Option<&CalendarInfo> =
            self.target.as_ref().and_then(|(a, id)| writable.iter().copied().find(|c| &c.account == a && &c.id == id));
        let src = target.map(|c| c.source);
        self.fit_to(src, target);

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
                    ui.add(egui::TextEdit::singleline(&mut self.location).hint_text("Local").desired_width(f32::INFINITY));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.description)
                            .hint_text("Descrição")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                });

                section(ui, pal, "Quando");
                card(ui, pal, |ui| self.when_ui(ui, pal));

                section(ui, pal, "Repetir");
                card(ui, pal, |ui| self.repeat_ui(ui, pal));

                section(ui, pal, "Calendário");
                card(ui, pal, |ui| {
                    if writable.is_empty() {
                        hint(ui, pal, "Nenhum calendário disponível para escrita. Conecte uma conta em Ajustes e sincronize. Se o Outlook ou o Google já estavam conectados, conecte de novo para dar permissão de criar eventos.");
                        return;
                    }
                    let current = target.map(|c| label(cfg, c)).unwrap_or_else(|| "Escolha…".into());
                    egui::ComboBox::from_id_salt("calendario").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                        for c in &writable {
                            let sel = self.target.as_ref().is_some_and(|(a, id)| a == &c.account && id == &c.id);
                            if ui.selectable_label(sel, label(cfg, c)).clicked() {
                                self.target = Some((c.account.clone(), c.id.clone()));
                            }
                        }
                    });
                    let is_principal = cfg.principal.as_ref().is_some_and(|p| {
                        self.target.as_ref().is_some_and(|(a, id)| &p.account == a && &p.id == id)
                    });
                    if !is_principal {
                        ui.checkbox(&mut self.make_principal, "Usar como calendário principal");
                    }
                    if let Some(acct) = target.and_then(|c| cfg.account(&c.account)) {
                        if !providers::can_write(acct) {
                            hint(ui, pal, &format!("Conecte {} de novo em Ajustes para permitir criar eventos.", cfg.account_label(&acct.id)));
                        }
                    }
                });

                // convidados e reunião online: só Google e Outlook
                if matches!(src, Some(Source::Google | Source::Outlook)) {
                    section(ui, pal, "Convidados");
                    card(ui, pal, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.attendees)
                                .hint_text("E-mails, separados por vírgula")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                        if let Some(name) = target.and_then(|c| c.meeting_label()) {
                            ui.checkbox(&mut self.meeting, format!("Adicionar {name}"));
                        }
                        match parse_emails(&self.attendees) {
                            Err(bad) => {
                                ui.label(RichText::new(format!("E-mail inválido: {bad}")).font(font(11.5)).color(pal.red));
                            }
                            Ok(v) if !v.is_empty() => hint(ui, pal, "Os convidados recebem o convite por e-mail."),
                            Ok(_) => {}
                        }
                    });
                }

                section(ui, pal, "Opções");
                card(ui, pal, |ui| self.options_ui(ui, pal, src));
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
            ui.add_space(8.0);
        });

        if submit && !busy && !self.title.trim().is_empty() {
            self.save(cfg, &ctx, target);
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        out
    }

    /// Ajusta as escolhas ao que o calendário escolhido aceita.
    fn fit_to(&mut self, src: Option<Source>, target: Option<&CalendarInfo>) {
        let Some(src) = src else { return };
        if !reminder_options(src).contains(&self.reminder) {
            self.reminder = reminder_options(src)[0];
        }
        if !ShowAs::options(src).contains(&self.show_as) {
            self.show_as = ShowAs::Busy;
        }
        if target.and_then(|c| c.meeting.as_ref()).is_none() {
            self.meeting = false;
        }
    }

    fn when_ui(&mut self, ui: &mut Ui, pal: &Pal) {
        ui.checkbox(&mut self.all_day, "Dia inteiro");
        let before = (self.start_date, self.start);
        let old_start = self.start_dt();
        let old_end = self.end_dt();

        row(ui, pal, "Início", |ui| {
            if !self.all_day {
                time_combo(ui, "inicio-hora", &mut self.start);
            }
            date_button(ui, "inicio-dia", &mut self.start_date);
        });
        if (self.start_date, self.start) != before {
            // mantém a duração ao mudar o início
            let dur = (old_end - old_start).max(Duration::minutes(STEP as i64));
            let end = self.start_dt() + dur;
            self.end_date = to_jiff(end.date());
            self.end = end.hour() * 60 + end.minute();
            // repetição semanal no dia do início acompanha a mudança
            let (old_wd, new_wd) = (from_jiff(before.0).weekday(), from_jiff(self.start_date).weekday());
            if self.weekdays == [old_wd] {
                self.weekdays = vec![new_wd];
            }
        }

        row(ui, pal, "Fim", |ui| {
            if !self.all_day {
                time_combo(ui, "fim-hora", &mut self.end);
            }
            date_button(ui, "fim-dia", &mut self.end_date);
        });

        let bad = if self.all_day { self.end_date < self.start_date } else { self.end_dt() <= self.start_dt() };
        if bad {
            ui.label(RichText::new("O fim precisa ser depois do início.").font(font(11.5)).color(pal.red));
        }
    }

    fn repeat_ui(&mut self, ui: &mut Ui, pal: &Pal) {
        let freq_label = |f: Option<Freq>| match f {
            None => "Não repete",
            Some(Freq::Daily) => "Todo dia",
            Some(Freq::Weekly) => "Toda semana",
            Some(Freq::Monthly) => "Todo mês",
            Some(Freq::Yearly) => "Todo ano",
        };
        egui::ComboBox::from_id_salt("repetir").selected_text(freq_label(self.freq)).width(ui.available_width()).show_ui(ui, |ui| {
            for f in [None, Some(Freq::Daily), Some(Freq::Weekly), Some(Freq::Monthly), Some(Freq::Yearly)] {
                ui.selectable_value(&mut self.freq, f, freq_label(f));
            }
        });
        let Some(freq) = self.freq else { return };

        let unit = match (freq, self.interval > 1) {
            (Freq::Daily, false) => "dia",
            (Freq::Daily, true) => "dias",
            (Freq::Weekly, false) => "semana",
            (Freq::Weekly, true) => "semanas",
            (Freq::Monthly, false) => "mês",
            (Freq::Monthly, true) => "meses",
            (Freq::Yearly, false) => "ano",
            (Freq::Yearly, true) => "anos",
        };
        row(ui, pal, "A cada", |ui| {
            ui.label(unit);
            ui.add(egui::DragValue::new(&mut self.interval).range(1..=99));
        });

        if freq == Freq::Weekly {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (wd, name) in WEEK {
                    let on = self.weekdays.contains(&wd);
                    if ui.selectable_label(on, name).clicked() {
                        if on {
                            self.weekdays.retain(|d| *d != wd);
                        } else {
                            self.weekdays.push(wd);
                        }
                    }
                }
            });
            if self.weekdays.is_empty() {
                hint(ui, pal, "Sem dia marcado: repete no dia do início.");
            }
        } else {
            let first = from_jiff(self.start_date);
            let note = match freq {
                Freq::Monthly => format!("No dia {} de cada mês.", first.day()),
                Freq::Yearly => format!("Todo {} de {}.", first.day(), MONTHS[first.month0() as usize]),
                _ => String::new(),
            };
            if !note.is_empty() {
                hint(ui, pal, &note);
            }
        }

        let end_label = |k: EndKind| match k {
            EndKind::Never => "Nunca",
            EndKind::Until => "Em uma data",
            EndKind::Count => "Após algumas vezes",
        };
        row(ui, pal, "Termina", |ui| {
            egui::ComboBox::from_id_salt("termina").selected_text(end_label(self.end_kind)).show_ui(ui, |ui| {
                for k in [EndKind::Never, EndKind::Until, EndKind::Count] {
                    ui.selectable_value(&mut self.end_kind, k, end_label(k));
                }
            });
        });
        match self.end_kind {
            EndKind::Never => {}
            EndKind::Until => row(ui, pal, "Até", |ui| date_button(ui, "ate-dia", &mut self.until)),
            EndKind::Count => row(ui, pal, "Ocorrências", |ui| {
                ui.add(egui::DragValue::new(&mut self.count).range(1..=999).suffix(" vezes"));
            }),
        }
    }

    fn options_ui(&mut self, ui: &mut Ui, pal: &Pal, src: Option<Source>) {
        let src = src.unwrap_or(Source::Google);
        row(ui, pal, "Lembrete", |ui| {
            egui::ComboBox::from_id_salt("lembrete").selected_text(reminder_label(self.reminder)).height(300.0).show_ui(ui, |ui| {
                for r in reminder_options(src) {
                    ui.selectable_value(&mut self.reminder, *r, reminder_label(*r));
                }
            });
        });
        row(ui, pal, "Mostrar como", |ui| {
            egui::ComboBox::from_id_salt("mostrar-como").selected_text(self.show_as.label()).show_ui(ui, |ui| {
                for s in ShowAs::options(src) {
                    ui.selectable_value(&mut self.show_as, *s, s.label());
                }
            });
        });
        ui.checkbox(&mut self.private, "Particular (outras pessoas veem só \"ocupado\")");
    }

    fn principal(&self, calendars: &[CalendarInfo]) -> Option<Principal> {
        let (a, id) = self.target.as_ref()?;
        let c = calendars.iter().find(|c| &c.account == a && &c.id == id)?;
        Some(Principal { account: c.account.clone(), source: c.source, id: c.id.clone(), name: c.name.clone() })
    }

    /// Monta o evento a partir do formulário (ou diz o que está errado).
    fn build(&self, target: Option<&CalendarInfo>) -> Result<NewEvent, String> {
        let first = from_jiff(self.start_date);
        let when = if self.all_day {
            let last = from_jiff(self.end_date);
            if last < first {
                return Err("O fim precisa ser no mesmo dia do início ou depois.".into());
            }
            When::AllDay { start: first, end: last + Duration::days(1) }
        } else {
            let utc = |dt: NaiveDateTime| Local.from_local_datetime(&dt).earliest().map(|d| d.with_timezone(&Utc));
            match (utc(self.start_dt()), utc(self.end_dt())) {
                (Some(start), Some(end)) if end > start => When::Timed { start, end },
                _ => return Err("Horário inválido: o fim precisa ser depois do início.".into()),
            }
        };
        let repeat = match self.freq {
            None => None,
            Some(freq) => {
                let end = match self.end_kind {
                    EndKind::Never => RepeatEnd::Never,
                    EndKind::Count => RepeatEnd::Count(self.count.max(1)),
                    EndKind::Until => {
                        let until = from_jiff(self.until);
                        if until < first {
                            return Err("A repetição precisa terminar depois do início.".into());
                        }
                        RepeatEnd::Until(until)
                    }
                };
                let mut weekdays = if freq == Freq::Weekly { self.weekdays.clone() } else { Vec::new() };
                if freq == Freq::Weekly && weekdays.is_empty() {
                    weekdays.push(first.weekday());
                }
                weekdays.sort_by_key(|d| d.num_days_from_sunday());
                Some(Repeat { freq, interval: self.interval.max(1), weekdays, end })
            }
        };
        let with_guests = matches!(target.map(|c| c.source), Some(Source::Google | Source::Outlook));
        let attendees = if with_guests {
            parse_emails(&self.attendees).map_err(|bad| format!("E-mail de convidado inválido: {bad}"))?
        } else {
            Vec::new()
        };
        let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        Ok(NewEvent {
            title: self.title.trim().to_string(),
            when,
            location: text(&self.location),
            description: text(&self.description),
            repeat,
            reminder: self.reminder,
            show_as: self.show_as,
            private: self.private,
            attendees,
            online_meeting: if self.meeting { target.and_then(|c| c.meeting.clone()) } else { None },
            block: false,
        })
    }

    fn save(&mut self, cfg: &Config, ctx: &egui::Context, target: Option<&CalendarInfo>) {
        let Some((account, id)) = self.target.clone() else { return };
        let Some(acct) = cfg.account(&account).cloned() else { return };
        let ev = match self.build(target) {
            Ok(ev) => ev,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.error = None;
        let slot: Slot = Arc::default();
        let (s, cfg, ctx) = (slot.clone(), cfg.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = providers::create(&cfg, &acct, &id, &ev).map(|_| ()).map_err(|e| format!("{e:#}"));
            *s.lock().unwrap() = Some(res);
            ctx.request_repaint();
        });
        self.pending = Some(slot);
    }
}

const MONTHS: [&str; 12] =
    ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];

/// Lembretes oferecidos. O iCloud não tem "padrão do calendário" pela API.
fn reminder_options(src: Source) -> &'static [Reminder] {
    const ALL: [Reminder; 12] = [
        Reminder::Default,
        Reminder::Off,
        Reminder::Minutes(0),
        Reminder::Minutes(5),
        Reminder::Minutes(10),
        Reminder::Minutes(15),
        Reminder::Minutes(30),
        Reminder::Minutes(60),
        Reminder::Minutes(120),
        Reminder::Minutes(1440),
        Reminder::Minutes(2880),
        Reminder::Minutes(10080),
    ];
    match src {
        Source::ICloud => &ALL[1..],
        Source::Google | Source::Outlook => &ALL,
    }
}

fn reminder_label(r: Reminder) -> String {
    match r {
        Reminder::Default => "Padrão do calendário".into(),
        Reminder::Off => "Sem lembrete".into(),
        Reminder::Minutes(0) => "Na hora".into(),
        Reminder::Minutes(m) if m % 10080 == 0 => plural(m / 10080, "semana", "semanas"),
        Reminder::Minutes(m) if m % 1440 == 0 => plural(m / 1440, "dia", "dias"),
        Reminder::Minutes(m) if m % 60 == 0 => plural(m / 60, "hora", "horas"),
        Reminder::Minutes(m) => plural(m, "minuto", "minutos"),
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {} antes", if n == 1 { one } else { many })
}

/// Separa e valida os e-mails. `Err` traz o primeiro inválido.
fn parse_emails(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for e in s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).map(str::trim).filter(|e| !e.is_empty()) {
        let ok = e.split_once('@').is_some_and(|(user, host)| {
            !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.') && !host.contains('@')
        });
        if !ok {
            return Err(e.to_string());
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(e)) {
            out.push(e.to_string());
        }
    }
    Ok(out)
}

fn label(cfg: &Config, c: &CalendarInfo) -> String {
    format!("{} · {}", c.name, cfg.account_label(&c.account))
}

fn at_minutes(day: NaiveDate, mins: u32) -> NaiveDateTime {
    day.and_hms_opt(0, 0, 0).expect("meia-noite") + Duration::minutes(mins as i64)
}

fn hm(mins: u32) -> String {
    format!("{:02}:{:02}", (mins / 60) % 24, mins % 60)
}

fn time_combo(ui: &mut Ui, id: &str, value: &mut u32) {
    egui::ComboBox::from_id_salt(id).selected_text(hm(*value)).width(72.0).height(240.0).show_ui(ui, |ui| {
        let mut m = 0;
        while m < 24 * 60 {
            ui.selectable_value(value, m, hm(m));
            m += STEP;
        }
    });
}

fn date_button(ui: &mut Ui, id: &str, date: &mut jiff::civil::Date) {
    ui.add(
        egui_extras::DatePickerButton::new(date)
            .id_salt(id)
            .format("%d/%m/%Y")
            .calendar_week(false)
            .combo_boxes(false),
    );
}

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).expect("data válida")
}

fn from_jiff(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32).expect("data válida")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails() {
        assert_eq!(parse_emails("a@x.com, b@y.org;\nA@x.com").unwrap(), vec!["a@x.com", "b@y.org"]);
        assert_eq!(parse_emails("a@x.com, nope").unwrap_err(), "nope");
        assert!(parse_emails("  ").unwrap().is_empty());
    }

    #[test]
    fn reminder_labels() {
        assert_eq!(reminder_label(Reminder::Minutes(15)), "15 minutos antes");
        assert_eq!(reminder_label(Reminder::Minutes(60)), "1 hora antes");
        assert_eq!(reminder_label(Reminder::Minutes(2880)), "2 dias antes");
        assert_eq!(reminder_label(Reminder::Minutes(10080)), "1 semana antes");
    }
}
