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

use crate::config::{Account, Config};
use crate::model::{CalendarInfo, Event, NewEvent, Source};

pub fn client() -> Client {
    Client::builder()
        .user_agent(concat!("Linha/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("cliente HTTP")
}

/// O que uma conta devolve ao sincronizar.
pub struct Fetched {
    pub events: Vec<Event>,
    pub calendars: Vec<CalendarInfo>,
    /// E-mail/dono da conta, quando o serviço informa.
    pub who: Option<String>,
}

/// Busca eventos (e a lista de calendários) de uma conta no intervalo [from, to).
pub fn fetch(cfg: &Config, acct: &Account, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Fetched> {
    match acct.servico {
        Source::Outlook => outlook::fetch(&cfg.outlook, &acct.id, from, to),
        Source::Google => google::fetch(&cfg.google, &acct.id, from, to),
        Source::ICloud => icloud::fetch(&acct.icloud(), &acct.id, from, to),
    }
}

/// Cria um evento num calendário da conta e devolve o id dele no serviço.
pub fn create(cfg: &Config, acct: &Account, calendar_id: &str, ev: &NewEvent) -> Result<String> {
    match acct.servico {
        Source::Outlook => outlook::create(&cfg.outlook, &acct.id, calendar_id, ev),
        Source::Google => google::create(&cfg.google, &acct.id, calendar_id, ev),
        Source::ICloud => icloud::create(&acct.icloud(), calendar_id, ev),
    }
}

/// Apaga um evento (`uid` = id no serviço; no iCloud, o UID).
pub fn delete(cfg: &Config, acct: &Account, calendar_id: &str, uid: &str) -> Result<()> {
    match acct.servico {
        Source::Outlook => outlook::delete(&cfg.outlook, &acct.id, uid),
        Source::Google => google::delete(&cfg.google, &acct.id, calendar_id, uid),
        Source::ICloud => icloud::delete(&acct.icloud(), calendar_id, uid),
    }
}

/// O login desta conta permite criar eventos? (Outlook e Google pedem permissão
/// de escrita; logins antigos, só de leitura, precisam conectar de novo.)
pub fn can_write(acct: &Account) -> bool {
    match acct.servico {
        Source::Outlook => outlook::can_write(&acct.id),
        Source::Google => google::can_write(&acct.id),
        Source::ICloud => true,
    }
}

/// A conta está conectada (tem tokens ou senha de app)?
pub fn connected(acct: &Account) -> bool {
    match acct.servico {
        Source::Outlook | Source::Google => crate::store::Tokens::load(&acct.id).is_some(),
        Source::ICloud => !acct.senha_app.trim().is_empty(),
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
