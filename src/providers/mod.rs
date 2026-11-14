//! Conectores de calendário.

pub mod google;
pub mod icloud;
pub mod ics;
pub mod oauth;
pub mod outlook;

use std::time::Duration;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use reqwest::blocking::{Client, Response};

use crate::config::Config;
use crate::model::{CalendarInfo, Event, NewEvent, Source};

pub fn client() -> Client {
    Client::builder()
        .user_agent(concat!("Linha/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("cliente HTTP")
}

/// Busca eventos (e a lista de calendários) de uma origem no intervalo [from, to).
pub fn fetch(cfg: &Config, src: Source, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<(Vec<Event>, Vec<CalendarInfo>)> {
    match src {
        Source::Outlook => outlook::fetch(&cfg.outlook, from, to),
        Source::Google => google::fetch(&cfg.google, from, to),
        Source::ICloud => icloud::fetch(&cfg.icloud, from, to),
    }
}

/// Cria um evento num calendário.
pub fn create(cfg: &Config, src: Source, calendar_id: &str, ev: &NewEvent) -> Result<()> {
    match src {
        Source::Outlook => outlook::create(&cfg.outlook, calendar_id, ev),
        Source::Google => google::create(&cfg.google, calendar_id, ev),
        Source::ICloud => icloud::create(&cfg.icloud, calendar_id, ev),
    }
}

/// O login desta conta permite criar eventos? (Outlook e Google pedem permissão
/// de escrita; logins antigos, só de leitura, precisam conectar de novo.)
pub fn can_write(src: Source) -> bool {
    match src {
        Source::Outlook => outlook::can_write(),
        Source::Google => google::can_write(),
        Source::ICloud => true,
    }
}

pub fn enabled(cfg: &Config, src: Source) -> bool {
    match src {
        Source::Outlook => cfg.outlook.ativo,
        Source::Google => cfg.google.ativo,
        Source::ICloud => cfg.icloud.ativo,
    }
}

/// Falha com uma mensagem legível se a resposta não for 2xx.
pub fn check(resp: Response, what: &str) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    match status.as_u16() {
        401 => bail!("{what}: não autorizado (401). Conecte a conta de novo em Ajustes. {body}"),
        403 => bail!("{what}: acesso negado (403). {body}"),
        _ => bail!("{what}: HTTP {status}. {body}"),
    }
}
