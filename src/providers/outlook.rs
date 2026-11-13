//! Outlook / Microsoft 365 via Microsoft Graph, com login por "device code".

use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;

use super::{check, client};
use crate::config::OutlookCfg;
use crate::model::{parse_hex_color, Event, Source, When, DEFAULT_COLOR};
use crate::store::{OAuthError, TokenResponse, Tokens};

const SCOPE: &str = "offline_access Calendars.Read";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

fn endpoint(cfg: &OutlookCfg, what: &str) -> String {
    let tenant = if cfg.tenant.trim().is_empty() { "common" } else { cfg.tenant.trim() };
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/{what}")
}

fn require(cfg: &OutlookCfg) -> Result<()> {
    if cfg.client_id.trim().is_empty() {
        bail!("Outlook: falta `client_id` em [outlook] no config.toml (veja o README)");
    }
    Ok(())
}

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}

/// Login interativo no terminal: mostra um código para digitar em microsoft.com/devicelogin.
pub fn login(cfg: &OutlookCfg) -> Result<()> {
    require(cfg)?;
    let http = client();
    let dc: DeviceCode = check(
        http.post(endpoint(cfg, "devicecode"))
            .form(&[("client_id", cfg.client_id.trim()), ("scope", SCOPE)])
            .send().context("Outlook: sem conexão com o servidor")?,
        "Outlook (device code)",
    )?
    .json()?;

    println!();
    println!("  1. Abra  {}", dc.verification_uri);
    println!("  2. Digite o código  {}", dc.user_code);
    println!();
    let _ = webbrowser::open(&dc.verification_uri);

    let mut interval = dc.interval.unwrap_or(5);
    let deadline = std::time::Instant::now() + Duration::from_secs(dc.expires_in);
    while std::time::Instant::now() < deadline {
        sleep(Duration::from_secs(interval));
        let resp = http
            .post(endpoint(cfg, "token"))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", cfg.client_id.trim()),
                ("device_code", dc.device_code.as_str()),
            ])
            .send().context("Outlook: sem conexão com o servidor")?;
        if resp.status().is_success() {
            let tr: TokenResponse = resp.json()?;
            Tokens::from_response(&tr, None).save(Source::Outlook)?;
            println!("  Outlook conectado.");
            return Ok(());
        }
        let err: OAuthError = resp.json().context("resposta inesperada do login da Microsoft")?;
        match err.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => interval += 5,
            _ => bail!("Outlook: {} {}", err.error, err.error_description.unwrap_or_default()),
        }
    }
    bail!("Outlook: o código expirou, tente de novo")
}

fn access_token(cfg: &OutlookCfg) -> Result<String> {
    require(cfg)?;
    let tokens = Tokens::load(Source::Outlook).ok_or_else(|| anyhow!("Outlook: rode `linha login outlook`"))?;
    if tokens.is_fresh() {
        return Ok(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone().ok_or_else(|| anyhow!("Outlook: sessão expirou, rode `linha login outlook`"))?;
    let resp = client()
        .post(endpoint(cfg, "token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", cfg.client_id.trim()),
            ("refresh_token", refresh.as_str()),
            ("scope", SCOPE),
        ])
        .send().context("Outlook: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Outlook (renovar sessão)")?.json()?;
    let t = Tokens::from_response(&tr, Some(refresh));
    t.save(Source::Outlook)?;
    Ok(t.access_token)
}

#[derive(Deserialize)]
struct Page<T> {
    value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Calendar {
    id: String,
    name: Option<String>,
    hex_color: Option<String>,
    color: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GEvent {
    id: String,
    subject: Option<String>,
    start: GTime,
    end: GTime,
    #[serde(default)]
    is_all_day: bool,
    #[serde(default)]
    is_cancelled: bool,
    location: Option<GLocation>,
    response_status: Option<GResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GTime {
    date_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GLocation {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct GResponse {
    response: Option<String>,
}

/// Cores nomeadas do Outlook quando `hexColor` vem vazio.
fn named_color(name: &str) -> [u8; 3] {
    match name {
        "lightBlue" => [0x4F, 0x9B, 0xE6],
        "lightGreen" => [0x5C, 0xB8, 0x5C],
        "lightOrange" => [0xF0, 0x9A, 0x37],
        "lightGray" => [0x8E, 0x8E, 0x93],
        "lightYellow" => [0xE8, 0xC2, 0x2E],
        "lightTeal" => [0x30, 0xB0, 0xC7],
        "lightPink" => [0xE8, 0x6A, 0xA6],
        "lightBrown" => [0xA2, 0x84, 0x5E],
        "lightRed" => [0xFF, 0x45, 0x3A],
        _ => DEFAULT_COLOR,
    }
}

fn parse_graph_time(s: &str) -> Result<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .with_context(|| format!("data inválida do Graph: {s}"))
}

fn date_part(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").with_context(|| format!("data inválida: {s}"))
}

pub fn fetch(cfg: &OutlookCfg, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Event>> {
    let token = access_token(cfg)?;
    let http = client();
    let get = |url: &str| -> Result<reqwest::blocking::Response> {
        check(
            http.get(url)
                .bearer_auth(&token)
                .header("Prefer", "outlook.timezone=\"UTC\"")
                .send().context("Outlook: sem conexão com o servidor")?,
            "Outlook",
        )
    };

    let cals: Page<Calendar> = get(&format!("{GRAPH}/me/calendars?$select=id,name,hexColor,color&$top=100"))?.json()?;
    let fmt = "%Y-%m-%dT%H:%M:%SZ";
    let mut out = Vec::new();

    for cal in cals.value {
        let color = cal
            .hex_color
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or_else(|| named_color(cal.color.as_deref().unwrap_or("")));
        let cal_name = cal.name.unwrap_or_else(|| "Outlook".into());
        let mut url = Some(format!(
            "{GRAPH}/me/calendars/{}/calendarView?startDateTime={}&endDateTime={}&$top=200\
             &$select=id,subject,start,end,isAllDay,isCancelled,location,responseStatus",
            cal.id,
            from.format(fmt),
            to.format(fmt)
        ));
        while let Some(u) = url.take() {
            let page: Page<GEvent> = get(&u)?.json()?;
            for e in page.value {
                let declined = e.response_status.as_ref().and_then(|r| r.response.as_deref()) == Some("declined");
                if e.is_cancelled || declined {
                    continue;
                }
                let when = if e.is_all_day {
                    When::AllDay { start: date_part(&e.start.date_time)?, end: date_part(&e.end.date_time)? }
                } else {
                    When::Timed {
                        start: parse_graph_time(&e.start.date_time)?.and_utc(),
                        end: parse_graph_time(&e.end.date_time)?.and_utc(),
                    }
                };
                out.push(Event {
                    id: format!("outlook:{}", e.id),
                    title: e.subject.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "(sem título)".into()),
                    when,
                    location: e.location.and_then(|l| l.display_name).filter(|s| !s.trim().is_empty()),
                    calendar: cal_name.clone(),
                    color,
                    source: Source::Outlook,
                });
            }
            url = page.next;
        }
    }
    Ok(out)
}
