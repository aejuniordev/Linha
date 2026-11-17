//! Tipos centrais: eventos, origem e o cache local em disco.

use std::collections::BTreeMap;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc, Weekday};
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

    /// Aceita a chave (`google`) ou o nome serializado antigo (`Google`).
    pub fn from_key(s: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|src| src.key().eq_ignore_ascii_case(s))
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
    /// Página do evento no serviço (para editar no navegador).
    #[serde(default)]
    pub link: Option<String>,
    /// Conta de onde veio (id em `Config::contas`).
    #[serde(default)]
    pub account: String,
    /// Calendário de onde veio (id no serviço).
    #[serde(default)]
    pub calendar_id: String,
    /// Identificador do evento no serviço (id do Google/Graph, UID no iCloud).
    #[serde(default)]
    pub uid: String,
    /// Marcado como "disponível" (não ocupa horário).
    #[serde(default)]
    pub free: bool,
    /// Bloqueio de horário criado pelo Linha (não aparece na linha do tempo).
    #[serde(default)]
    pub block: bool,
}

impl Event {
    /// Endereço para editar o evento no navegador: a página do próprio evento
    /// quando o serviço informa, senão a agenda web da conta.
    pub fn web_url(&self) -> &str {
        match &self.link {
            Some(l) if l.starts_with("https://") => l,
            _ => match self.source {
                Source::Outlook => "https://outlook.office.com/calendar/",
                Source::Google => "https://calendar.google.com/calendar/",
                Source::ICloud => "https://www.icloud.com/calendar/",
            },
        }
    }

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
    /// E-mail/dono da conta, como o serviço informou.
    #[serde(default)]
    pub who: Option<String>,
}

/// Um calendário de uma conta conectada.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalendarInfo {
    pub source: Source,
    /// Conta dona deste calendário (id em `Config::contas`).
    #[serde(default)]
    pub account: String,
    /// Calendário padrão da conta (Google "primary", Outlook "Calendário").
    #[serde(default)]
    pub primary: bool,
    /// Identificador no serviço (id do Graph/Google, URL do CalDAV).
    pub id: String,
    pub name: String,
    pub color: [u8; 3],
    /// Dá para criar eventos nele.
    pub writable: bool,
    /// Reunião online que o calendário sabe gerar (`hangoutsMeet`, `teamsForBusiness`…).
    #[serde(default)]
    pub meeting: Option<String>,
}

impl CalendarInfo {
    /// Nome amigável da reunião online deste calendário.
    pub fn meeting_label(&self) -> Option<&'static str> {
        Some(match self.meeting.as_deref()? {
            "hangoutsMeet" => "Google Meet",
            "teamsForBusiness" => "Microsoft Teams",
            "skypeForBusiness" => "Skype for Business",
            "skypeForConsumer" => "Skype",
            _ => "reunião online",
        })
    }
}

/// Evento a ser criado.
#[derive(Clone, Debug)]
pub struct NewEvent {
    pub title: String,
    pub when: When,
    pub location: Option<String>,
    pub description: Option<String>,
    pub repeat: Option<Repeat>,
    pub reminder: Reminder,
    pub show_as: ShowAs,
    pub private: bool,
    /// E-mails dos convidados (Google e Outlook mandam o convite).
    pub attendees: Vec<String>,
    /// Gera o link de reunião online com este provedor (`hangoutsMeet`, `teamsForBusiness`…).
    pub online_meeting: Option<String>,
    /// Bloqueio de horário do Linha: leva uma marca para ser reconhecido depois.
    pub block: bool,
}

impl NewEvent {
    /// Data local de início (base da repetição).
    pub fn local_start_date(&self) -> NaiveDate {
        match &self.when {
            When::Timed { start, .. } => start.with_timezone(&Local).date_naive(),
            When::AllDay { start, .. } => *start,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatEnd {
    Never,
    /// Última data (inclusive), no horário local.
    Until(NaiveDate),
    Count(u32),
}

/// Regra de repetição (subconjunto comum a Google, Outlook e iCalendar).
#[derive(Clone, Debug, PartialEq)]
pub struct Repeat {
    pub freq: Freq,
    /// A cada N dias/semanas/meses/anos.
    pub interval: u32,
    /// Dias da semana (só na repetição semanal; vazio = o dia do início).
    pub weekdays: Vec<Weekday>,
    pub end: RepeatEnd,
}

impl Repeat {
    /// Regra RRULE (RFC 5545), sem o prefixo `RRULE:`.
    /// `day_shift` desloca os dias da semana quando o DTSTART vai em UTC e cai
    /// em outro dia que o horário local (ex.: 22h em Brasília = 1h UTC do dia seguinte).
    pub fn rrule(&self, all_day: bool, day_shift: i64) -> String {
        let freq = match self.freq {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        };
        let mut r = format!("FREQ={freq}");
        if self.interval > 1 {
            r.push_str(&format!(";INTERVAL={}", self.interval));
        }
        if self.freq == Freq::Weekly && !self.weekdays.is_empty() {
            let days: Vec<&str> = self.weekdays.iter().map(|d| byday(shift_weekday(*d, day_shift))).collect();
            r.push_str(&format!(";BYDAY={}", days.join(",")));
        }
        match self.end {
            RepeatEnd::Never => {}
            RepeatEnd::Count(n) => r.push_str(&format!(";COUNT={}", n.max(1))),
            RepeatEnd::Until(d) if all_day => r.push_str(&format!(";UNTIL={}", d.format("%Y%m%d"))),
            RepeatEnd::Until(d) => {
                // fim do dia local, em UTC
                let end = local_midnight(d.succ_opt().unwrap_or(d)).with_timezone(&Utc) - chrono::Duration::seconds(1);
                r.push_str(&format!(";UNTIL={}", end.format("%Y%m%dT%H%M%SZ")));
            }
        }
        r
    }
}

fn shift_weekday(d: Weekday, by: i64) -> Weekday {
    let n = (d.num_days_from_monday() as i64 + by).rem_euclid(7);
    Weekday::try_from(n as u8).unwrap_or(d)
}

fn byday(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reminder {
    /// O lembrete padrão do calendário.
    Default,
    Off,
    /// Minutos antes do início.
    Minutes(u32),
}

/// Como o horário aparece para quem vê sua disponibilidade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowAs {
    Busy,
    Free,
    /// Só no Outlook.
    Tentative,
    /// Só no Outlook ("Ausente").
    Away,
    /// Só no Outlook.
    WorkingElsewhere,
}

impl ShowAs {
    pub fn label(self) -> &'static str {
        match self {
            ShowAs::Busy => "Ocupado",
            ShowAs::Free => "Disponível",
            ShowAs::Tentative => "Provisório",
            ShowAs::Away => "Ausente",
            ShowAs::WorkingElsewhere => "Trabalhando em outro lugar",
        }
    }

    /// Opções que o serviço aceita.
    pub fn options(src: Source) -> &'static [ShowAs] {
        match src {
            Source::Outlook => &[ShowAs::Busy, ShowAs::Free, ShowAs::Tentative, ShowAs::Away, ShowAs::WorkingElsewhere],
            Source::Google | Source::ICloud => &[ShowAs::Busy, ShowAs::Free],
        }
    }
}

/// Fuso horário local no formato IANA (ex.: `America/Sao_Paulo`).
pub fn local_tz_name() -> Option<String> {
    iana_time_zone::get_timezone().ok().filter(|s| s.contains('/'))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub updated: Option<DateTime<Utc>>,
    pub events: Vec<Event>,
    /// Situação da última sincronização, por conta (id em `Config::contas`).
    pub status: BTreeMap<String, SyncStatus>,
    /// Calendários vistos na última sincronização de cada conta.
    pub calendars: Vec<CalendarInfo>,
}

impl Cache {
    pub fn load() -> Cache {
        let path = crate::store::cache_path();
        let mut cache: Cache = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // cache de antes das várias contas: cada serviço tinha uma só, com id = nome do serviço
        for e in cache.events.iter_mut().filter(|e| e.account.is_empty()) {
            e.account = e.source.key().to_string();
        }
        for c in cache.calendars.iter_mut().filter(|c| c.account.is_empty()) {
            c.account = c.source.key().to_string();
        }
        cache.status = std::mem::take(&mut cache.status)
            .into_iter()
            .map(|(k, v)| (Source::from_key(&k).map(|s| s.key().to_string()).unwrap_or(k), v))
            .collect();
        cache
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        crate::store::write_private(&crate::store::cache_path(), json.as_bytes())
    }

    /// Eventos do dia, ordenados: dia inteiro primeiro, depois por início.
    pub fn events_on(&self, day: NaiveDate) -> Vec<Event> {
        // bloqueios do Linha repetem eventos que já aparecem pela conta de origem
        let mut v: Vec<Event> = self.events.iter().filter(|e| !e.block && e.touches_day(day)).cloned().collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(freq: Freq, interval: u32, weekdays: Vec<Weekday>, end: RepeatEnd) -> Repeat {
        Repeat { freq, interval, weekdays, end }
    }

    #[test]
    fn rrule_basico() {
        assert_eq!(rep(Freq::Daily, 1, vec![], RepeatEnd::Never).rrule(false, 0), "FREQ=DAILY");
        assert_eq!(rep(Freq::Monthly, 2, vec![], RepeatEnd::Count(5)).rrule(false, 0), "FREQ=MONTHLY;INTERVAL=2;COUNT=5");
        let d = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert_eq!(rep(Freq::Yearly, 1, vec![], RepeatEnd::Until(d)).rrule(true, 0), "FREQ=YEARLY;UNTIL=20261231");
        assert!(rep(Freq::Daily, 1, vec![], RepeatEnd::Until(d)).rrule(false, 0).ends_with('Z'));
    }

    #[test]
    fn rrule_semanal_com_deslocamento() {
        let r = rep(Freq::Weekly, 1, vec![Weekday::Mon, Weekday::Sun], RepeatEnd::Never);
        assert_eq!(r.rrule(false, 0), "FREQ=WEEKLY;BYDAY=MO,SU");
        // 22h de domingo em Brasília = 1h de segunda em UTC
        assert_eq!(r.rrule(false, 1), "FREQ=WEEKLY;BYDAY=TU,MO");
    }
}
