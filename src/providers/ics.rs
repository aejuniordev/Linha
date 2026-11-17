//! Leitor de iCalendar (RFC 5545) enxuto, com expansão das recorrências mais comuns.
//!
//! Suporta: VEVENT, DTSTART/DTEND/DURATION com TZID, dia inteiro, RRULE
//! (DAILY/WEEKLY/MONTHLY/YEARLY com INTERVAL, COUNT, UNTIL, BYDAY, BYMONTHDAY, BYMONTH),
//! EXDATE e exceções via RECURRENCE-ID.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use chrono::{
    DateTime, Datelike, Duration, Local, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;

use crate::model::{local_midnight, Event, Source, When};

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

/// Junta linhas "dobradas" (continuações começam com espaço ou tab).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

fn parse_line(line: &str) -> Option<Prop> {
    // separa "NOME;PARAM=..." de "valor" no primeiro ':' fora de aspas
    let mut in_quotes = false;
    let mut split = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let i = split?;
    let (head, value) = (&line[..i], &line[i + 1..]);
    let mut parts = Vec::new();
    let mut cur = String::new();
    in_quotes = false;
    for c in head.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.first()?.to_ascii_uppercase();
    let params = parts[1..]
        .iter()
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_ascii_uppercase(), v.to_string())))
        .collect();
    Some(Prop { name, params, value: value.to_string() })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Um instante do iCalendar, preservando o fuso para expandir recorrências corretamente.
#[derive(Debug, Clone, PartialEq)]
pub enum Dt {
    Date(NaiveDate),
    Utc(NaiveDateTime),
    Zoned(NaiveDateTime, Tz),
    Floating(NaiveDateTime),
}

fn windows_zone(name: &str) -> Option<Tz> {
    let iana = match name {
        "E. South America Standard Time" => "America/Sao_Paulo",
        "SA Eastern Standard Time" => "America/Fortaleza",
        "Central Brazilian Standard Time" => "America/Cuiaba",
        "Pacific Standard Time" => "America/Los_Angeles",
        "Mountain Standard Time" => "America/Denver",
        "Central Standard Time" => "America/Chicago",
        "Eastern Standard Time" => "America/New_York",
        "GMT Standard Time" => "Europe/London",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "GTB Standard Time" => "Europe/Athens",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "UTC" | "Coordinated Universal Time" => "UTC",
        _ => return None,
    };
    Tz::from_str(iana).ok()
}

fn resolve_tz(tzid: &str) -> Option<Tz> {
    let t = tzid.trim().trim_matches('"');
    if let Ok(tz) = Tz::from_str(t) {
        return Some(tz);
    }
    if let Some(tz) = windows_zone(t) {
        return Some(tz);
    }
    // ex.: "/mozilla.org/20050126_1/America/Sao_Paulo"
    let segs: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    for n in [3, 2] {
        if segs.len() >= n {
            if let Ok(tz) = Tz::from_str(&segs[segs.len() - n..].join("/")) {
                return Some(tz);
            }
        }
    }
    None
}

impl Dt {
    fn parse_one(value: &str, tzid: Option<&str>, is_date: bool) -> Option<Dt> {
        let v = value.trim();
        if is_date || (v.len() == 8 && v.chars().all(|c| c.is_ascii_digit())) {
            return NaiveDate::parse_from_str(v.get(..8)?, "%Y%m%d").ok().map(Dt::Date);
        }
        let (body, utc) = match v.strip_suffix('Z') {
            Some(b) => (b, true),
            None => (v, false),
        };
        let naive = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").ok()?;
        if utc {
            return Some(Dt::Utc(naive));
        }
        match tzid.and_then(resolve_tz) {
            Some(tz) => Some(Dt::Zoned(naive, tz)),
            None => Some(Dt::Floating(naive)),
        }
    }

    fn parse_prop(p: &Prop) -> Vec<Dt> {
        let is_date = p.param("VALUE").map(|v| v.eq_ignore_ascii_case("DATE")).unwrap_or(false);
        let tzid = p.param("TZID");
        p.value.split(',').filter_map(|v| Dt::parse_one(v, tzid, is_date)).collect()
    }

    pub fn to_utc(&self) -> DateTime<Utc> {
        match self {
            Dt::Date(d) => local_midnight(*d).with_timezone(&Utc),
            Dt::Utc(n) => n.and_utc(),
            Dt::Zoned(n, tz) => tz
                .from_local_datetime(n)
                .earliest()
                .or_else(|| tz.from_local_datetime(&(*n + Duration::hours(1))).earliest())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|| n.and_utc()),
            Dt::Floating(n) => Local
                .from_local_datetime(n)
                .earliest()
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|| n.and_utc()),
        }
    }

    fn naive(&self) -> NaiveDateTime {
        match self {
            Dt::Date(d) => d.and_time(NaiveTime::MIN),
            Dt::Utc(n) | Dt::Zoned(n, _) | Dt::Floating(n) => *n,
        }
    }

    /// Mesmo "tipo" de horário (fuso), com outro horário de parede.
    fn with_naive(&self, n: NaiveDateTime) -> Dt {
        match self {
            Dt::Date(_) => Dt::Date(n.date()),
            Dt::Utc(_) => Dt::Utc(n),
            Dt::Zoned(_, tz) => Dt::Zoned(n, *tz),
            Dt::Floating(_) => Dt::Floating(n),
        }
    }
}

fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (c, in_time) {
                    ('W', _) => Duration::weeks(n),
                    ('D', _) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { -total } else { total })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone)]
struct RRule {
    freq: Freq,
    interval: u32,
    count: Option<u32>,
    until: Option<Dt>,
    byday: Vec<(Option<i32>, Weekday)>,
    bymonthday: Vec<i32>,
    bymonth: Vec<u32>,
}

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

impl RRule {
    fn parse(s: &str) -> Option<RRule> {
        let mut r = RRule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            byday: vec![],
            bymonthday: vec![],
            bymonth: vec![],
        };
        let mut freq = None;
        for part in s.split(';') {
            let (k, v) = part.split_once('=')?;
            match k.to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None, // HOURLY etc. não suportado
                    })
                }
                "INTERVAL" => r.interval = v.parse().unwrap_or(1).max(1),
                "COUNT" => r.count = v.parse().ok(),
                "UNTIL" => r.until = Dt::parse_one(v, None, false),
                "BYDAY" => {
                    for d in v.split(',') {
                        let d = d.trim();
                        if d.len() < 2 {
                            continue;
                        }
                        let (num, wd) = d.split_at(d.len() - 2);
                        if let Some(wd) = weekday(wd) {
                            r.byday.push((num.parse().ok(), wd));
                        }
                    }
                }
                "BYMONTHDAY" => r.bymonthday = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                "BYMONTH" => r.bymonth = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                _ => {}
            }
        }
        r.freq = freq?;
        Some(r)
    }
}

fn days_in_month(y: i32, m: u32) -> u32 {
    let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else { return 0 };
    match first.checked_add_months(Months::new(1)) {
        Some(next) => (next - first).num_days() as u32,
        None => 0,
    }
}

/// Primeiro dia do período `p` — usado para parar a expansão depois da janela.
fn period_start(rule: &RRule, d0: NaiveDate, p: u32) -> Option<NaiveDate> {
    let step = p.saturating_mul(rule.interval);
    match rule.freq {
        Freq::Daily => d0.checked_add_signed(Duration::days(step as i64)),
        Freq::Weekly => (d0 - Duration::days(d0.weekday().num_days_from_monday() as i64))
            .checked_add_signed(Duration::weeks(step as i64)),
        Freq::Monthly => NaiveDate::from_ymd_opt(d0.year(), d0.month(), 1)?.checked_add_months(Months::new(step)),
        Freq::Yearly => NaiveDate::from_ymd_opt(d0.year().checked_add(step as i32)?, 1, 1),
    }
}

/// Dias do mês que batem com BYDAY (com ordinal: 1MO, -1FR; sem: todas as segundas…).
fn month_byday(y: i32, m: u32, byday: &[(Option<i32>, Weekday)]) -> Vec<NaiveDate> {
    let n = days_in_month(y, m);
    let mut out = Vec::new();
    for &(ord, wd) in byday {
        let all: Vec<NaiveDate> = (1..=n)
            .filter_map(|d| NaiveDate::from_ymd_opt(y, m, d))
            .filter(|d| d.weekday() == wd)
            .collect();
        match ord {
            None | Some(0) => out.extend(all),
            Some(k) if k > 0 => out.extend(all.get(k as usize - 1)),
            Some(k) => {
                let idx = all.len() as i32 + k;
                if idx >= 0 {
                    out.extend(all.get(idx as usize));
                }
            }
        }
    }
    out
}

fn month_days(y: i32, m: u32, mdays: &[i32]) -> Vec<NaiveDate> {
    let n = days_in_month(y, m) as i32;
    mdays
        .iter()
        .filter_map(|&d| {
            let d = if d < 0 { n + 1 + d } else { d };
            if d >= 1 && d <= n {
                NaiveDate::from_ymd_opt(y, m, d as u32)
            } else {
                None
            }
        })
        .collect()
}

/// Datas candidatas do período `p` (0, 1, 2…) da regra, em ordem.
fn period_dates(rule: &RRule, d0: NaiveDate, p: u32) -> Vec<NaiveDate> {
    let step = p.saturating_mul(rule.interval);
    let mut dates = match rule.freq {
        Freq::Daily => {
            let d = d0 + Duration::days(step as i64);
            let ok_day = rule.byday.is_empty() || rule.byday.iter().any(|(_, w)| *w == d.weekday());
            let ok_month = rule.bymonth.is_empty() || rule.bymonth.contains(&d.month());
            if ok_day && ok_month {
                vec![d]
            } else {
                vec![]
            }
        }
        Freq::Weekly => {
            let week = d0 - Duration::days(d0.weekday().num_days_from_monday() as i64) + Duration::weeks(step as i64);
            let days: Vec<Weekday> = if rule.byday.is_empty() {
                vec![d0.weekday()]
            } else {
                rule.byday.iter().map(|(_, w)| *w).collect()
            };
            days.into_iter().map(|w| week + Duration::days(w.num_days_from_monday() as i64)).collect()
        }
        Freq::Monthly => {
            let Some(first) = period_start(rule, d0, p) else { return vec![] };
            let (y, m) = (first.year(), first.month());
            if !rule.bymonth.is_empty() && !rule.bymonth.contains(&m) {
                vec![]
            } else if !rule.byday.is_empty() {
                let v = month_byday(y, m, &rule.byday);
                if rule.bymonthday.is_empty() {
                    v
                } else {
                    let md = month_days(y, m, &rule.bymonthday);
                    v.into_iter().filter(|d| md.contains(d)).collect()
                }
            } else if !rule.bymonthday.is_empty() {
                month_days(y, m, &rule.bymonthday)
            } else {
                NaiveDate::from_ymd_opt(y, m, d0.day()).into_iter().collect()
            }
        }
        Freq::Yearly => {
            let y = d0.year() + step as i32;
            let months = if rule.bymonth.is_empty() { vec![d0.month()] } else { rule.bymonth.clone() };
            let mut v = Vec::new();
            for m in months {
                if !rule.byday.is_empty() {
                    v.extend(month_byday(y, m, &rule.byday));
                } else if !rule.bymonthday.is_empty() {
                    v.extend(month_days(y, m, &rule.bymonthday));
                } else {
                    v.extend(NaiveDate::from_ymd_opt(y, m, d0.day()));
                }
            }
            v
        }
    };
    dates.sort();
    dates.dedup();
    dates
}

#[derive(Debug, Clone)]
pub struct VEvent {
    pub uid: String,
    pub summary: String,
    pub location: Option<String>,
    pub start: Dt,
    pub end: Option<Dt>,
    pub duration: Option<Duration>,
    rrule: Option<RRule>,
    exdates: Vec<Dt>,
    pub recurrence_id: Option<Dt>,
    pub cancelled: bool,
    /// TRANSP:TRANSPARENT = disponível.
    pub transparent: bool,
    /// Bloqueio criado pelo Linha (`X-LINHA-BLOQUEIO`).
    pub block: bool,
}

/// Calendário de onde vêm os VEVENTs.
pub struct Target<'a> {
    pub account: &'a str,
    pub calendar_id: &'a str,
    pub name: &'a str,
    pub color: [u8; 3],
    pub source: Source,
}

impl VEvent {
    fn span(&self) -> Duration {
        if let Some(d) = self.duration {
            return d.max(Duration::zero());
        }
        match (&self.start, &self.end) {
            (Dt::Date(s), Some(Dt::Date(e))) => (*e - *s).max(Duration::days(1)),
            (Dt::Date(_), _) => Duration::days(1),
            (s, Some(e)) => (e.to_utc() - s.to_utc()).max(Duration::zero()),
            (_, None) => Duration::zero(),
        }
    }
}

/// Lê todos os VEVENT de um texto iCalendar (ignora VALARM e afins).
pub fn parse_vevents(text: &str) -> Vec<VEvent> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<Prop>> = None;
    let mut depth_inside = 0; // componentes aninhados dentro do VEVENT (VALARM)
    for line in unfold(text) {
        let Some(p) = parse_line(&line) else { continue };
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") if cur.is_none() => {
                cur = Some(Vec::new());
                depth_inside = 0;
            }
            ("BEGIN", _) if cur.is_some() => depth_inside += 1,
            ("END", "VEVENT") if depth_inside == 0 => {
                if let Some(props) = cur.take() {
                    if let Some(ev) = build(props) {
                        out.push(ev);
                    }
                }
            }
            ("END", _) if cur.is_some() => depth_inside -= 1,
            _ => {
                if depth_inside == 0 {
                    if let Some(c) = cur.as_mut() {
                        c.push(p);
                    }
                }
            }
        }
    }
    out
}

fn build(props: Vec<Prop>) -> Option<VEvent> {
    let get = |n: &str| props.iter().find(|p| p.name == n);
    let start = Dt::parse_prop(get("DTSTART")?).into_iter().next()?;
    Some(VEvent {
        uid: get("UID").map(|p| p.value.clone()).unwrap_or_default(),
        summary: get("SUMMARY").map(|p| unescape(&p.value)).unwrap_or_default(),
        location: get("LOCATION").map(|p| unescape(&p.value)).filter(|s| !s.trim().is_empty()),
        end: get("DTEND").and_then(|p| Dt::parse_prop(p).into_iter().next()),
        duration: get("DURATION").and_then(|p| parse_duration(&p.value)),
        rrule: get("RRULE").and_then(|p| RRule::parse(&p.value)),
        exdates: props.iter().filter(|p| p.name == "EXDATE").flat_map(Dt::parse_prop).collect(),
        recurrence_id: get("RECURRENCE-ID").and_then(|p| Dt::parse_prop(p).into_iter().next()),
        cancelled: get("STATUS").map(|p| p.value.eq_ignore_ascii_case("CANCELLED")).unwrap_or(false),
        transparent: get("TRANSP").is_some_and(|p| p.value.trim().eq_ignore_ascii_case("TRANSPARENT")),
        block: get(super::icloud::BLOCK_PROP).is_some(),
        start,
    })
}

fn overlaps(start: DateTime<Utc>, span: Duration, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
    let end = start + span;
    if span.is_zero() {
        start >= from && start < to
    } else {
        start < to && end > from
    }
}

fn to_event(ev: &VEvent, start: &Dt, span: Duration, t: &Target) -> Event {
    let when = match start {
        Dt::Date(d) => When::AllDay { start: *d, end: *d + span.max(Duration::days(1)) },
        other => {
            let s = other.to_utc();
            When::Timed { start: s, end: s + span }
        }
    };
    let s = start.to_utc().timestamp();
    Event {
        id: format!("{}:{}:{}", t.account, ev.uid, s),
        title: if ev.summary.trim().is_empty() { "(sem título)".into() } else { ev.summary.clone() },
        when,
        location: ev.location.clone(),
        calendar: t.name.to_string(),
        color: t.color,
        source: t.source,
        link: None,
        account: t.account.to_string(),
        calendar_id: t.calendar_id.to_string(),
        uid: ev.uid.clone(),
        free: ev.transparent,
        block: ev.block,
    }
}

/// Transforma VEVENTs (possivelmente recorrentes) em ocorrências dentro de [from, to).
pub fn expand(vevents: &[VEvent], from: DateTime<Utc>, to: DateTime<Utc>, target: &Target) -> Vec<Event> {
    // exceções (RECURRENCE-ID) substituem a ocorrência original
    let mut overridden: HashMap<&str, HashSet<i64>> = HashMap::new();
    for ev in vevents {
        if let Some(rid) = &ev.recurrence_id {
            overridden.entry(ev.uid.as_str()).or_default().insert(rid.to_utc().timestamp());
        }
    }

    let mut out = Vec::new();
    for ev in vevents {
        if ev.cancelled {
            continue;
        }
        let span = ev.span();
        let rule = match (&ev.rrule, &ev.recurrence_id) {
            (Some(r), None) => r,
            _ => {
                if overlaps(ev.start.to_utc(), span, from, to) {
                    out.push(to_event(ev, &ev.start, span, target));
                }
                continue;
            }
        };

        let skip: HashSet<i64> = ev
            .exdates
            .iter()
            .map(|d| match (d, &ev.start) {
                // EXDATE só com data num evento com horário: casa pelo dia
                (Dt::Date(day), s) if !matches!(s, Dt::Date(_)) => {
                    s.with_naive(day.and_time(s.naive().time())).to_utc().timestamp()
                }
                _ => d.to_utc().timestamp(),
            })
            .chain(overridden.get(ev.uid.as_str()).into_iter().flatten().copied())
            .collect();
        let until = rule.until.as_ref().map(|u| match u {
            Dt::Date(d) => local_midnight(*d + Duration::days(1)).with_timezone(&Utc),
            other => other.to_utc(),
        });

        let start_naive = ev.start.naive();
        let (d0, time) = (start_naive.date(), start_naive.time());
        let mut emitted = 0u32;
        let last_day = to.with_timezone(&Local).date_naive() + Duration::days(1);
        'periods: for p in 0..200_000u32 {
            match period_start(rule, d0, p) {
                Some(ps) if ps <= last_day => {}
                _ => break,
            }
            for d in period_dates(rule, d0, p) {
                if d < d0 {
                    continue;
                }
                let occ = ev.start.with_naive(d.and_time(time));
                let occ_utc = occ.to_utc();
                if until.is_some_and(|u| occ_utc > u) || occ_utc >= to {
                    break 'periods;
                }
                emitted += 1;
                if rule.count.is_some_and(|c| emitted > c) {
                    break 'periods;
                }
                if !skip.contains(&occ_utc.timestamp()) && overlaps(occ_utc, span, from, to) {
                    out.push(to_event(ev, &occ, span, target));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn target(name: &str) -> Target<'_> {
        Target { account: "icloud", calendar_id: "cal", name, color: [0, 0, 0], source: Source::ICloud }
    }

    #[test]
    fn transparencia_e_bloqueio() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x@linha\r\nSUMMARY:Ocupado\r\nDTSTART:20261110T120000Z\r\n\
                   DTEND:20261110T130000Z\r\nTRANSP:TRANSPARENT\r\nX-LINHA-BLOQUEIO:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let evs = expand(&parse_vevents(ics), utc("2026-11-10T00:00:00Z"), utc("2026-11-11T00:00:00Z"), &target("c"));
        assert_eq!(evs.len(), 1);
        assert!(evs[0].free && evs[0].block);
        assert_eq!((evs[0].uid.as_str(), evs[0].account.as_str(), evs[0].calendar_id.as_str()), ("x@linha", "icloud", "cal"));
    }

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:abc\r\nSUMMARY:Daily de\r\n  produto\r\n\
DTSTART;TZID=America/Sao_Paulo:20261102T093000\r\nDTEND;TZID=America/Sao_Paulo:20261102T100000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=20\r\nEXDATE;TZID=America/Sao_Paulo:20261111T093000\r\n\
LOCATION:Sala 3\\, 2º andar\r\nBEGIN:VALARM\r\nTRIGGER:-PT10M\r\nSUMMARY:alarme\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:abc\r\nRECURRENCE-ID;TZID=America/Sao_Paulo:20261113T093000\r\n\
DTSTART;TZID=America/Sao_Paulo:20261113T110000\r\nDTEND;TZID=America/Sao_Paulo:20261113T113000\r\n\
SUMMARY:Daily (remarcada)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn weekly_with_exdate_and_override() {
        let v = parse_vevents(WEEKLY);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].summary, "Daily de produto");
        assert_eq!(v[0].location.as_deref(), Some("Sala 3, 2º andar"));
        let evs = expand(&v, utc("2026-11-09T00:00:00Z"), utc("2026-11-16T00:00:00Z"), &target("Trabalho"));
        let mut starts: Vec<String> = evs
            .iter()
            .map(|e| match &e.when {
                When::Timed { start, .. } => format!("{} {}", start.format("%d %H:%M"), e.title),
                _ => unreachable!(),
            })
            .collect();
        starts.sort();
        // seg 9 (12:30Z), qua 11 cancelada por EXDATE, sex 13 remarcada para 11:00 (14:00Z)
        assert_eq!(starts, vec!["09 12:30 Daily de produto", "13 14:00 Daily (remarcada)"]);
    }

    #[test]
    fn monthly_last_friday_and_count() {
        let ics = "BEGIN:VEVENT\nUID:m\nSUMMARY:Retro\nDTSTART:20260130T170000Z\nDURATION:PT1H\nRRULE:FREQ=MONTHLY;BYDAY=-1FR;COUNT=12\nEND:VEVENT\n";
        let v = parse_vevents(ics);
        let evs = expand(&v, utc("2026-11-01T00:00:00Z"), utc("2027-03-01T00:00:00Z"), &target("c"));
        let days: Vec<String> = evs.iter().map(|e| e.local_start().with_timezone(&Utc).format("%Y-%m-%d").to_string()).collect();
        // jan..dez/2026 = 12 ocorrências; nov e dez dentro da janela
        assert_eq!(days, vec!["2026-11-27", "2026-12-25"]);
    }

    #[test]
    fn all_day_yearly_birthday() {
        let ics = "BEGIN:VEVENT\nUID:b\nSUMMARY:Aniversário da Ana\nDTSTART;VALUE=DATE:19900314\nDTEND;VALUE=DATE:19900315\nRRULE:FREQ=YEARLY\nEND:VEVENT\n";
        let v = parse_vevents(ics);
        let evs = expand(&v, utc("2027-03-01T00:00:00Z"), utc("2027-04-01T00:00:00Z"), &target("c"));
        assert_eq!(evs.len(), 1);
        assert_eq!(
            evs[0].when,
            When::AllDay { start: NaiveDate::from_ymd_opt(2027, 3, 14).unwrap(), end: NaiveDate::from_ymd_opt(2027, 3, 15).unwrap() }
        );
    }

    #[test]
    fn daily_until_and_interval() {
        let ics = "BEGIN:VEVENT\nUID:d\nSUMMARY:Remédio\nDTSTART:20261110T110000Z\nDTEND:20261110T111500Z\nRRULE:FREQ=DAILY;INTERVAL=2;UNTIL=20261116T110000Z\nEND:VEVENT\n";
        let evs = expand(&parse_vevents(ics), utc("2026-11-01T00:00:00Z"), utc("2026-12-01T00:00:00Z"), &target("c"));
        assert_eq!(evs.len(), 4); // 10, 12, 14, 16
    }

    #[test]
    fn durations_and_tz() {
        assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("P1W"), Some(Duration::weeks(1)));
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert!(resolve_tz("/mozilla.org/20050126_1/America/Sao_Paulo").is_some());
        assert!(resolve_tz("E. South America Standard Time").is_some());
    }
}
