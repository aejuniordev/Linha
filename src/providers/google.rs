//! Google Agenda (Gmail) via Calendar API v3. Login pelo navegador
//! (authorization code + PKCE, retorno em http://127.0.0.1).

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use url::Url;

use super::oauth::{self, Flow};
use super::{check, client};
use crate::config::GoogleCfg;
use crate::model::{parse_hex_color, CalendarInfo, Event, NewEvent, Reminder, ShowAs, Source, When, DEFAULT_COLOR};
use crate::store::{TokenResponse, Tokens};

/// Ler agendas e criar eventos.
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly https://www.googleapis.com/auth/calendar.events";
const WRITE_SCOPE: &str = "auth/calendar.events";
const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://www.googleapis.com/calendar/v3";

fn credentials(cfg: &GoogleCfg) -> Result<(String, String)> {
    cfg.credentials().ok_or_else(|| anyhow!("Google: esta versão do Linha foi compilada sem o app do Google (veja docs/OAUTH.md)"))
}

/// Abre o navegador para a pessoa entrar na conta Google e guarda os tokens.
pub fn login(cfg: &GoogleCfg, flow: &Flow) -> Result<()> {
    let (id, secret) = credentials(cfg)?;
    let grant = oauth::authorize(
        flow,
        AUTH,
        "127.0.0.1",
        &[
            ("client_id", id.as_str()),
            ("scope", SCOPE),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
        ],
    )
    .context("Google")?;
    let resp = client()
        .post(TOKEN)
        .form(&[
            ("code", grant.code.as_str()),
            ("client_id", id.as_str()),
            ("client_secret", secret.as_str()),
            ("redirect_uri", grant.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", grant.verifier.as_str()),
        ])
        .send()
        .context("Google: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Google (trocar código)")?.json()?;
    let mut t = Tokens::from_response(&tr, None);
    t.scope.get_or_insert_with(|| SCOPE.into());
    t.save(Source::Google)
}

fn access_token(cfg: &GoogleCfg) -> Result<String> {
    let (id, secret) = credentials(cfg)?;
    let tokens = Tokens::load(Source::Google).ok_or_else(|| anyhow!("Google: conecte a conta em Ajustes"))?;
    if tokens.is_fresh() {
        return Ok(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone().ok_or_else(|| anyhow!("Google: sessão expirou, conecte de novo em Ajustes"))?;
    let resp = client()
        .post(TOKEN)
        .form(&[
            ("client_id", id.as_str()),
            ("client_secret", secret.as_str()),
            ("refresh_token", refresh.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send().context("Google: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Google (renovar sessão)")?.json()?;
    let t = Tokens::from_response(&tr, Some(&tokens));
    t.save(Source::Google)?;
    Ok(t.access_token)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarList {
    items: Vec<CalEntry>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalEntry {
    id: String,
    summary: Option<String>,
    summary_override: Option<String>,
    background_color: Option<String>,
    selected: Option<bool>,
    hidden: Option<bool>,
    access_role: Option<String>,
    conference_properties: Option<ConferenceProps>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConferenceProps {
    #[serde(default)]
    allowed_conference_solution_types: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventList {
    #[serde(default)]
    items: Vec<GEvent>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct GEvent {
    id: String,
    status: Option<String>,
    summary: Option<String>,
    location: Option<String>,
    #[serde(rename = "htmlLink")]
    html_link: Option<String>,
    start: Option<GTime>,
    end: Option<GTime>,
    #[serde(default)]
    attendees: Vec<Attendee>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GTime {
    date: Option<String>,
    date_time: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attendee {
    #[serde(rename = "self", default)]
    is_self: bool,
    response_status: Option<String>,
}

fn parse_when(start: &GTime, end: &GTime) -> Result<When> {
    if let (Some(s), Some(e)) = (&start.date_time, &end.date_time) {
        return Ok(When::Timed {
            start: DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc),
            end: DateTime::parse_from_rfc3339(e)?.with_timezone(&Utc),
        });
    }
    let s = start.date.as_deref().ok_or_else(|| anyhow!("evento sem início"))?;
    let e = end.date.as_deref().unwrap_or(s);
    let s = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
    let e = NaiveDate::parse_from_str(e, "%Y-%m-%d")?.max(s.succ_opt().unwrap_or(s));
    Ok(When::AllDay { start: s, end: e })
}

pub fn fetch(cfg: &GoogleCfg, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<(Vec<Event>, Vec<CalendarInfo>)> {
    let token = access_token(cfg)?;
    let http = client();

    let mut cals = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let mut req = http.get(format!("{API}/users/me/calendarList")).bearer_auth(&token).query(&[("minAccessRole", "reader")]);
        if let Some(p) = &page {
            req = req.query(&[("pageToken", p)]);
        }
        let list: CalendarList = check(req.send().context("Google: sem conexão com o servidor")?, "Google (calendários)")?.json()?;
        cals.extend(list.items);
        page = list.next_page_token;
        if page.is_none() {
            break;
        }
    }

    let mut out = Vec::new();
    let mut infos = Vec::new();
    for cal in cals.into_iter().filter(|c| c.selected != Some(false) && c.hidden != Some(true)) {
        let color = cal.background_color.as_deref().and_then(parse_hex_color).unwrap_or(DEFAULT_COLOR);
        let cal_name = cal.summary_override.or(cal.summary).unwrap_or_else(|| "Google".into());
        infos.push(CalendarInfo {
            source: Source::Google,
            id: cal.id.clone(),
            name: cal_name.clone(),
            color,
            writable: matches!(cal.access_role.as_deref(), Some("owner" | "writer")),
            meeting: cal
                .conference_properties
                .as_ref()
                .filter(|p| p.allowed_conference_solution_types.iter().any(|t| t == "hangoutsMeet"))
                .map(|_| "hangoutsMeet".to_string()),
        });
        let mut url = Url::parse(API)?;
        url.path_segments_mut().map_err(|_| anyhow!("url"))?.extend(["calendars", cal.id.as_str(), "events"]);

        let mut page: Option<String> = None;
        loop {
            let mut req = http.get(url.clone()).bearer_auth(&token).query(&[
                ("timeMin", from.to_rfc3339()),
                ("timeMax", to.to_rfc3339()),
                ("singleEvents", "true".into()),
                ("orderBy", "startTime".into()),
                ("maxResults", "250".into()),
            ]);
            if let Some(p) = &page {
                req = req.query(&[("pageToken", p)]);
            }
            let list: EventList = check(req.send().context("Google: sem conexão com o servidor")?, "Google (eventos)")?.json().context("Google: resposta de eventos")?;
            for e in list.items {
                let declined = e.attendees.iter().any(|a| a.is_self && a.response_status.as_deref() == Some("declined"));
                if e.status.as_deref() == Some("cancelled") || declined {
                    continue;
                }
                let (Some(s), Some(en)) = (&e.start, &e.end) else { continue };
                let Ok(when) = parse_when(s, en) else { continue };
                out.push(Event {
                    id: format!("google:{}", e.id),
                    title: e.summary.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "(sem título)".into()),
                    when,
                    location: e.location.filter(|s| !s.trim().is_empty()),
                    calendar: cal_name.clone(),
                    color,
                    source: Source::Google,
                    link: e.html_link,
                });
            }
            page = list.next_page_token;
            if page.is_none() {
                break;
            }
        }
    }
    Ok((out, infos))
}

/// O login atual permite criar eventos?
pub fn can_write() -> bool {
    Tokens::load(Source::Google).is_some_and(|t| t.has_scope(WRITE_SCOPE))
}

/// Cria um evento no calendário `calendar_id`.
pub fn create(cfg: &GoogleCfg, calendar_id: &str, ev: &NewEvent) -> Result<()> {
    if !can_write() {
        anyhow::bail!("Google: conecte a conta de novo em Ajustes para permitir criar eventos");
    }
    let token = access_token(cfg)?;
    let body = event_body(ev);
    let mut url = Url::parse(API)?;
    url.path_segments_mut().map_err(|_| anyhow!("url"))?.extend(["calendars", calendar_id, "events"]);
    let mut query: Vec<(&str, &str)> = Vec::new();
    if ev.online_meeting.is_some() {
        query.push(("conferenceDataVersion", "1"));
    }
    if !ev.attendees.is_empty() {
        query.push(("sendUpdates", "all"));
    }
    let resp = client()
        .post(url)
        .query(&query)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .context("Google: sem conexão com o servidor")?;
    check(resp, "Google (criar evento)")?;
    Ok(())
}

/// Corpo JSON de um evento novo (Calendar API v3, `events.insert`).
fn event_body(ev: &NewEvent) -> serde_json::Value {
    use serde_json::json;

    // o fuso é obrigatório em eventos que se repetem (a repetição é expandida nele)
    let tz = crate::model::local_tz_name().unwrap_or_else(|| "UTC".into());
    let (start, end) = match &ev.when {
        When::Timed { start, end } => (
            json!({ "dateTime": start.with_timezone(&chrono::Local).to_rfc3339(), "timeZone": tz }),
            json!({ "dateTime": end.with_timezone(&chrono::Local).to_rfc3339(), "timeZone": tz }),
        ),
        When::AllDay { start, end } => (
            json!({ "date": start.to_string(), "timeZone": tz }),
            json!({ "date": end.to_string(), "timeZone": tz }),
        ),
    };
    let mut body = json!({ "summary": ev.title, "start": start, "end": end });
    if let Some(l) = &ev.location {
        body["location"] = json!(l);
    }
    if let Some(d) = &ev.description {
        body["description"] = json!(d);
    }
    if let Some(r) = &ev.repeat {
        body["recurrence"] = json!([format!("RRULE:{}", r.rrule(matches!(ev.when, When::AllDay { .. }), 0))]);
    }
    match ev.reminder {
        Reminder::Default => {}
        Reminder::Off => body["reminders"] = json!({ "useDefault": false, "overrides": [] }),
        Reminder::Minutes(m) => {
            body["reminders"] = json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": m }] })
        }
    }
    if ev.show_as == ShowAs::Free {
        body["transparency"] = json!("transparent");
    }
    if ev.private {
        body["visibility"] = json!("private");
    }
    if !ev.attendees.is_empty() {
        body["attendees"] = ev.attendees.iter().map(|e| json!({ "email": e })).collect();
    }
    if let Some(provider) = &ev.online_meeting {
        use rand::distr::{Alphanumeric, SampleString};
        body["conferenceData"] = json!({
            "createRequest": {
                "requestId": Alphanumeric.sample_string(&mut rand::rng(), 24),
                "conferenceSolutionKey": { "type": provider },
            }
        });
    }
    body
}
