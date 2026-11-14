//! Tipos centrais: eventos, origem e o cache local em disco.

use std::collections::BTreeMap;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Source {
    Outlook,
    Google,
    ICloud,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Outlook, Source::Google, Source::ICloud];

    pub fn label(self) -> &'static str {
        match self {
            Source::Outlook => "Outlook",
            Source::Google => "Google",
            Source::ICloud => "iCloud",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Source::Outlook => "outlook",
            Source::Google => "google",
            Source::ICloud => "icloud",
        }
    }
}

/// Quando o evento acontece.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum When {
    /// Evento com horário, em UTC.
    Timed { start: DateTime<Utc>, end: DateTime<Utc> },
    /// Evento de dia inteiro. `end` é exclusivo (padrão iCalendar).
    AllDay { start: NaiveDate, end: NaiveDate },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub title: String,
    pub when: When,
    pub location: Option<String>,
    pub calendar: String,
    /// Cor do calendário (RGB).
    pub color: [u8; 3],
    pub source: Source,
}

impl Event {
    /// Início em horário local (dia inteiro = meia-noite local).
    pub fn local_start(&self) -> DateTime<Local> {
        match &self.when {
            When::Timed { start, .. } => start.with_timezone(&Local),
            When::AllDay { start, .. } => local_midnight(*start),
        }
    }

    pub fn local_end(&self) -> DateTime<Local> {
        match &self.when {
            When::Timed { end, .. } => end.with_timezone(&Local),
            When::AllDay { end, .. } => local_midnight(*end),
        }
    }

    pub fn is_all_day(&self) -> bool {
        matches!(self.when, When::AllDay { .. })
    }

    /// O evento ocupa alguma parte do dia `day` (horário local)?
    pub fn touches_day(&self, day: NaiveDate) -> bool {
        match &self.when {
            When::AllDay { start, end } => *start <= day && day < *end,
            When::Timed { .. } => {
                let ds = local_midnight(day);
                let de = local_midnight(day.succ_opt().unwrap_or(day));
                let s = self.local_start();
                let e = self.local_end().max(s);
                // eventos de duração zero contam no dia em que começam
                (s < de && e > ds) || (s == e && s >= ds && s < de)
            }
        }
    }
}

pub fn local_midnight(day: NaiveDate) -> DateTime<Local> {
    let naive = day.and_hms_opt(0, 0, 0).expect("meia-noite válida");
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap_or_else(|| Local.from_utc_datetime(&naive))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncStatus {
    pub ok: bool,
    pub message: Option<String>,
    pub at: Option<DateTime<Utc>>,
    pub count: usize,
}

/// Um calendário de uma conta conectada.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalendarInfo {
    pub source: Source,
    /// Identificador no serviço (id do Graph/Google, URL do CalDAV).
    pub id: String,
    pub name: String,
    pub color: [u8; 3],
    /// Dá para criar eventos nele.
    pub writable: bool,
}

/// Evento a ser criado.
#[derive(Clone, Debug)]
pub struct NewEvent {
    pub title: String,
    pub when: When,
    pub location: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub updated: Option<DateTime<Utc>>,
    pub events: Vec<Event>,
    pub status: BTreeMap<Source, SyncStatus>,
    /// Calendários vistos na última sincronização de cada conta.
    pub calendars: Vec<CalendarInfo>,
}

impl Cache {
    pub fn load() -> Cache {
        let path = crate::store::cache_path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        crate::store::write_private(&crate::store::cache_path(), json.as_bytes())
    }

    /// Eventos do dia, ordenados: dia inteiro primeiro, depois por início.
    pub fn events_on(&self, day: NaiveDate) -> Vec<Event> {
        let mut v: Vec<Event> = self.events.iter().filter(|e| e.touches_day(day)).cloned().collect();
        v.sort_by(|a, b| {
            b.is_all_day()
                .cmp(&a.is_all_day())
                .then(a.local_start().cmp(&b.local_start()))
                .then(a.title.cmp(&b.title))
        });
        v
    }
}

/// Converte "#RRGGBB" (ou "RRGGBB", "#RRGGBBAA") em RGB.
pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() < 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

/// Azul do sistema iOS — cor padrão quando o calendário não tem uma.
pub const DEFAULT_COLOR: [u8; 3] = [0, 122, 255];
