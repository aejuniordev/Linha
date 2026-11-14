//! Outlook / Microsoft 365 via Microsoft Graph. Login pelo navegador
//! (authorization code + PKCE, retorno em http://localhost).

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;

use super::oauth::{self, Flow};
use super::{check, client};
use crate::config::OutlookCfg;
use crate::model::{parse_hex_color, CalendarInfo, Event, NewEvent, Source, When, DEFAULT_COLOR};
use crate::store::{TokenResponse, Tokens};

/// Ler e criar eventos.
const SCOPE: &str = "offline_access Calendars.ReadWrite";
/// Escopo dos logins feitos antes da escrita existir (o refresh não pode pedir mais que isso).
const LEGACY_SCOPE: &str = "offline_access Calendars.Read";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

fn endpoint(cfg: &OutlookCfg, what: &str) -> String {
    let tenant = if cfg.tenant.trim().is_empty() { "common" } else { cfg.tenant.trim() };
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/{what}")
}

fn client_id(cfg: &OutlookCfg) -> Result<String> {
    cfg.client_id().ok_or_else(|| anyhow!("Outlook: esta versão do Linha foi compilada sem o app da Microsoft (veja docs/OAUTH.md)"))
}

/// Abre o navegador para a pessoa entrar na conta Microsoft e guarda os tokens.
pub fn login(cfg: &OutlookCfg, flow: &Flow) -> Result<()> {
    let id = client_id(cfg)?;
    // A Microsoft aceita qualquer porta em http://localhost para apps de desktop.
    let grant = oauth::authorize(
        flow,
        &endpoint(cfg, "authorize"),
        "localhost",
        &[("client_id", id.as_str()), ("scope", SCOPE), ("prompt", "select_account")],
    )
    .context("Outlook")?;
    let resp = client()
        .post(endpoint(cfg, "token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", id.as_str()),
            ("code", grant.code.as_str()),
            ("redirect_uri", grant.redirect_uri.as_str()),
            ("code_verifier", grant.verifier.as_str()),
            ("scope", SCOPE),
        ])
        .send()
        .context("Outlook: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Outlook (trocar código)")?.json()?;
    let mut t = Tokens::from_response(&tr, None);
    t.scope.get_or_insert_with(|| SCOPE.into());
    t.save(Source::Outlook)
}

fn access_token(cfg: &OutlookCfg) -> Result<String> {
    let id = client_id(cfg)?;
    let tokens = Tokens::load(Source::Outlook).ok_or_else(|| anyhow!("Outlook: conecte a conta em Ajustes"))?;
    if tokens.is_fresh() {
        return Ok(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone().ok_or_else(|| anyhow!("Outlook: sessão expirou, conecte de novo em Ajustes"))?;
    let resp = client()
        .post(endpoint(cfg, "token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", id.as_str()),
            ("refresh_token", refresh.as_str()),
            ("scope", refresh_scope(&tokens)),
        ])
        .send().context("Outlook: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Outlook (renovar sessão)")?.json()?;
    let t = Tokens::from_response(&tr, Some(&tokens));
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
    can_edit: Option<bool>,
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

pub fn fetch(cfg: &OutlookCfg, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<(Vec<Event>, Vec<CalendarInfo>)> {
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

    let cals: Page<Calendar> = get(&format!("{GRAPH}/me/calendars?$select=id,name,hexColor,color,canEdit&$top=100"))?.json()?;
    let fmt = "%Y-%m-%dT%H:%M:%SZ";
    let mut out = Vec::new();
    let mut infos = Vec::new();

    for cal in cals.value {
        let color = cal
            .hex_color
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or_else(|| named_color(cal.color.as_deref().unwrap_or("")));
        let cal_name = cal.name.unwrap_or_else(|| "Outlook".into());
        infos.push(CalendarInfo {
            source: Source::Outlook,
            id: cal.id.clone(),
            name: cal_name.clone(),
            color,
            writable: cal.can_edit.unwrap_or(true),
        });
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
    Ok((out, infos))
}

fn refresh_scope(tokens: &Tokens) -> &'static str {
    if tokens.has_scope("Calendars.ReadWrite") {
        SCOPE
    } else {
        LEGACY_SCOPE
    }
}

/// O login atual permite criar eventos?
pub fn can_write() -> bool {
    Tokens::load(Source::Outlook).is_some_and(|t| t.has_scope("Calendars.ReadWrite"))
}

/// Cria um evento no calendário `calendar_id`.
pub fn create(cfg: &OutlookCfg, calendar_id: &str, ev: &NewEvent) -> Result<()> {
    if !can_write() {
        anyhow::bail!("Outlook: conecte a conta de novo em Ajustes para permitir criar eventos");
    }
    let token = access_token(cfg)?;
    let fmt = "%Y-%m-%dT%H:%M:%S";
    let (start, end, all_day) = match &ev.when {
        When::Timed { start, end } => (start.format(fmt).to_string(), end.format(fmt).to_string(), false),
        When::AllDay { start, end } => (format!("{start}T00:00:00"), format!("{end}T00:00:00"), true),
    };
    let mut body = serde_json::json!({
        "subject": ev.title,
        "isAllDay": all_day,
        "start": { "dateTime": start, "timeZone": "UTC" },
        "end": { "dateTime": end, "timeZone": "UTC" },
    });
    if let Some(l) = &ev.location {
        body["location"] = serde_json::json!({ "displayName": l });
    }
    let mut url = url::Url::parse(GRAPH)?;
    url.path_segments_mut().map_err(|_| anyhow!("url"))?.extend(["me", "calendars", calendar_id, "events"]);
    let resp = client().post(url).bearer_auth(&token).json(&body).send().context("Outlook: sem conexão com o servidor")?;
    check(resp, "Outlook (criar evento)")?;
    Ok(())
}
