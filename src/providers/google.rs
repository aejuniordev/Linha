//! Google Agenda (Gmail) via Calendar API v3, login OAuth com PKCE e redirect em 127.0.0.1.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use rand::distr::{Alphanumeric, SampleString};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use super::{check, client};
use crate::config::GoogleCfg;
use crate::model::{parse_hex_color, Event, Source, When, DEFAULT_COLOR};
use crate::store::{TokenResponse, Tokens};

const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://www.googleapis.com/calendar/v3";

fn require(cfg: &GoogleCfg) -> Result<()> {
    if cfg.client_id.trim().is_empty() || cfg.client_secret.trim().is_empty() {
        bail!("Google: preencha `client_id` e `client_secret` em [google] no config.toml (veja o README)");
    }
    Ok(())
}

/// Abre o navegador, espera o retorno em 127.0.0.1 e guarda os tokens.
pub fn login(cfg: &GoogleCfg) -> Result<()> {
    require(cfg)?;
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| anyhow!("servidor local: {e}"))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| anyhow!("sem porta local"))?;
    let redirect = format!("http://127.0.0.1:{port}");

    let mut rng = rand::rng();
    let verifier = Alphanumeric.sample_string(&mut rng, 64);
    let state = Alphanumeric.sample_string(&mut rng, 24);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let auth_url = Url::parse_with_params(
        AUTH,
        &[
            ("client_id", cfg.client_id.trim()),
            ("redirect_uri", redirect.as_str()),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )?;

    println!();
    println!("  Abrindo o navegador para entrar na sua conta Google…");
    println!("  Se não abrir, cole este link:\n  {auth_url}");
    println!();
    let _ = webbrowser::open(auth_url.as_str());

    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let code = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            bail!("Google: tempo esgotado esperando o login");
        }
        let Some(req) = server.recv_timeout(left)? else { continue };
        let url = Url::parse(&format!("http://127.0.0.1{}", req.url()))?;
        let get = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());
        if get("state").as_deref() != Some(state.as_str()) {
            let _ = req.respond(tiny_http::Response::from_string("").with_status_code(404));
            continue;
        }
        let ok = get("code");
        let html = if ok.is_some() {
            "<h2 style='font-family:-apple-system,system-ui;font-weight:500'>Pronto ✓</h2><p style='font-family:system-ui;color:#888'>Pode fechar esta aba.</p>"
        } else {
            "<h2 style='font-family:system-ui'>Login cancelado</h2>"
        };
        let header = tiny_http::Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
        let _ = req.respond(tiny_http::Response::from_string(html).with_header(header));
        match ok {
            Some(c) => break c,
            None => bail!("Google: {}", get("error").unwrap_or_else(|| "login cancelado".into())),
        }
    };

    let resp = client()
        .post(TOKEN)
        .form(&[
            ("code", code.as_str()),
            ("client_id", cfg.client_id.trim()),
            ("client_secret", cfg.client_secret.trim()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send().context("Google: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Google (trocar código)")?.json()?;
    Tokens::from_response(&tr, None).save(Source::Google)?;
    println!("  Google conectado.");
    Ok(())
}

fn access_token(cfg: &GoogleCfg) -> Result<String> {
    require(cfg)?;
    let tokens = Tokens::load(Source::Google).ok_or_else(|| anyhow!("Google: rode `linha login google`"))?;
    if tokens.is_fresh() {
        return Ok(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone().ok_or_else(|| anyhow!("Google: sessão expirou, rode `linha login google`"))?;
    let resp = client()
        .post(TOKEN)
        .form(&[
            ("client_id", cfg.client_id.trim()),
            ("client_secret", cfg.client_secret.trim()),
            ("refresh_token", refresh.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send().context("Google: sem conexão com o servidor")?;
    let tr: TokenResponse = check(resp, "Google (renovar sessão)")?.json()?;
    let t = Tokens::from_response(&tr, Some(refresh));
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

pub fn fetch(cfg: &GoogleCfg, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Event>> {
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
    for cal in cals.into_iter().filter(|c| c.selected != Some(false) && c.hidden != Some(true)) {
        let color = cal.background_color.as_deref().and_then(parse_hex_color).unwrap_or(DEFAULT_COLOR);
        let cal_name = cal.summary_override.or(cal.summary).unwrap_or_else(|| "Google".into());
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
                });
            }
            page = list.next_page_token;
            if page.is_none() {
                break;
            }
        }
    }
    Ok(out)
}
