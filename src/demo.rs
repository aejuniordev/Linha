//! Agenda de exemplo (`linha demo`) para ver a interface sem conectar contas.

use chrono::{Duration, DurationRound, Local, TimeZone, Utc};

use crate::model::{Cache, Event, Source, SyncStatus, When};

pub fn cache() -> Cache {
    let now = Local::now().duration_trunc(Duration::minutes(15)).unwrap_or_else(|_| Local::now());
    let today = now.date_naive();
    let tomorrow = today + Duration::days(1);
    let at = |mins: i64| now + Duration::minutes(mins);
    let tomorrow_at = |h: u32, m: u32| {
        Local.from_local_datetime(&tomorrow.and_hms_opt(h, m, 0).unwrap()).earliest().unwrap().with_timezone(&Utc)
    };

    let mut n = 0;
    let mut ev = |title: &str, when: When, loc: Option<&str>, cal: &str, color: [u8; 3], source: Source| {
        n += 1;
        Event {
            id: format!("demo:{n}"),
            title: title.into(),
            when,
            location: loc.map(Into::into),
            calendar: cal.into(),
            color,
            source,
        }
    };
    let timed = |a: i64, b: i64| When::Timed { start: at(a).with_timezone(&Utc), end: at(b).with_timezone(&Utc) };
    let blue = [10, 132, 255];
    let orange = [255, 149, 0];
    let green = [52, 199, 89];
    let purple = [175, 82, 222];
    let pink = [255, 45, 85];
    let teal = [48, 176, 199];
    let red = [255, 59, 48];

    let events = vec![
        ev("Aniversário da Ana", When::AllDay { start: today, end: tomorrow }, None, "Família", purple, Source::ICloud),
        ev("Corrida no parque", timed(-330, -285), Some("Parque Ibirapuera"), "Pessoal", green, Source::ICloud),
        ev("Daily do time", timed(-210, -195), Some("Microsoft Teams"), "Trabalho", blue, Source::Outlook),
        ev("Revisão de design do app", timed(-150, -90), Some("Sala Copacabana"), "Produto", orange, Source::Google),
        ev("Aula de violão", timed(-30, 45), Some("Studio Lá Maior"), "Pessoal", teal, Source::Google),
        ev("Ligar para a mãe", timed(75, 90), None, "Família", pink, Source::ICloud),
        ev("Leitura · O Pequeno Príncipe", timed(120, 150), None, "Pessoal", purple, Source::ICloud),
        ev("Entrega do relatório trimestral", When::AllDay { start: tomorrow, end: tomorrow + Duration::days(1) }, None, "Trabalho", red, Source::Outlook),
        ev("Voo GRU → SDU", When::Timed { start: tomorrow_at(7, 30), end: tomorrow_at(8, 35) }, Some("Terminal 2 · Portão 14"), "Viagens", blue, Source::Outlook),
        ev("Workshop de produto", When::Timed { start: tomorrow_at(10, 0), end: tomorrow_at(12, 0) }, Some("Escritório Rio"), "Produto", orange, Source::Google),
        ev("Jantar com a Bia", When::Timed { start: tomorrow_at(20, 0), end: tomorrow_at(22, 0) }, Some("Aprazível, Santa Teresa"), "Pessoal", pink, Source::ICloud),
    ];

    let mut cache = Cache { updated: Some(Utc::now() - Duration::minutes(2)), events, ..Default::default() };
    for src in Source::ALL {
        let count = cache.events.iter().filter(|e| e.source == src).count();
        cache.status.insert(src, SyncStatus { ok: true, message: None, at: cache.updated, count });
    }
    cache
}
