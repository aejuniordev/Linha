//! Datas e horas em português.

use chrono::{DateTime, Datelike, Local, NaiveDate};

const WEEKDAYS: [&str; 7] = ["segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado", "domingo"];
const WEEKDAYS_SHORT: [&str; 7] = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"];
const MONTHS: [&str; 12] = [
    "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro",
];
const MONTHS_SHORT: [&str; 12] = ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// "Sexta-feira, 13 de novembro"
pub fn long_date(d: NaiveDate) -> String {
    format!(
        "{}, {} de {}",
        capitalize(WEEKDAYS[d.weekday().num_days_from_monday() as usize]),
        d.day(),
        MONTHS[d.month0() as usize]
    )
}

/// "sex, 13 nov"
pub fn short_date(d: NaiveDate) -> String {
    format!("{}, {} {}", WEEKDAYS_SHORT[d.weekday().num_days_from_monday() as usize], d.day(), MONTHS_SHORT[d.month0() as usize])
}

/// "Hoje", "Amanhã" ou "Domingo"
pub fn day_title(d: NaiveDate, today: NaiveDate) -> String {
    match (d - today).num_days() {
        0 => "Hoje".into(),
        1 => "Amanhã".into(),
        _ => capitalize(WEEKDAYS[d.weekday().num_days_from_monday() as usize]),
    }
}

pub fn hm(t: DateTime<Local>) -> String {
    t.format("%H:%M").to_string()
}

/// "25 min", "1 h", "1 h 10 min"
pub fn span(minutes: i64) -> String {
    let m = minutes.max(0);
    match (m / 60, m % 60) {
        (0, 0) => "instantes".into(),
        (0, mm) => format!("{mm} min"),
        (h, 0) => format!("{h} h"),
        (h, mm) => format!("{h} h {mm} min"),
    }
}

/// "agora há pouco", "há 3 min", "há 2 h"
pub fn ago(minutes: i64) -> String {
    match minutes {
        m if m < 1 => "agora há pouco".into(),
        m if m < 60 => format!("há {m} min"),
        m if m < 60 * 24 => format!("há {} h", m / 60),
        m => format!("há {} dias", m / (60 * 24)),
    }
}
