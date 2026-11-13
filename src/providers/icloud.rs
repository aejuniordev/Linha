//! iCloud via CalDAV (caldav.icloud.com), autenticado com Apple ID + senha de app.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use reqwest::Method;
use url::Url;

use super::{check, client, ics};
use crate::config::ICloudCfg;
use crate::model::{parse_hex_color, Event, Source, DEFAULT_COLOR};

const ROOT: &str = "https://caldav.icloud.com/";
const NS_DAV: &str = "DAV:";
const NS_CAL: &str = "urn:ietf:params:xml:ns:caldav";
const NS_APPLE: &str = "http://apple.com/ns/ical/";

struct Dav<'a> {
    http: Client,
    cfg: &'a ICloudCfg,
}

impl Dav<'_> {
    fn request(&self, method: &str, url: &Url, depth: &str, body: String) -> Result<String> {
        let m = Method::from_bytes(method.as_bytes())?;
        let resp = self
            .http
            .request(m, url.clone())
            .basic_auth(self.cfg.apple_id.trim(), Some(self.cfg.senha_app.trim()))
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body)
            .send().context("iCloud: sem conexão com o servidor")?;
        if resp.status().as_u16() == 401 {
            bail!("iCloud: Apple ID ou senha de app incorretos (use uma senha de app, não a senha normal)");
        }
        Ok(check(resp, "iCloud")?.text()?)
    }
}

/// Pega o texto de um <href> dentro do primeiro elemento `tag` do documento.
fn find_href(xml: &str, tag: &str) -> Result<String> {
    let doc = roxmltree::Document::parse(xml).context("iCloud: XML inválido")?;
    doc.descendants()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.descendants().find(|c| c.has_tag_name((NS_DAV, "href"))))
        .and_then(|h| h.text())
        .map(|t| t.trim().to_string())
        .ok_or_else(|| anyhow!("iCloud: resposta sem <{tag}>"))
}

struct CalInfo {
    url: Url,
    name: String,
    color: [u8; 3],
}

fn discover(dav: &Dav) -> Result<Vec<CalInfo>> {
    let root = Url::parse(ROOT)?;

    let xml = dav.request(
        "PROPFIND",
        &root,
        "0",
        r#"<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#.into(),
    )?;
    let principal = root.join(&find_href(&xml, "current-user-principal")?)?;

    let xml = dav.request(
        "PROPFIND",
        &principal,
        "0",
        r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#.into(),
    )?;
    let home = principal.join(&find_href(&xml, "calendar-home-set")?)?;

    let xml = dav.request(
        "PROPFIND",
        &home,
        "1",
        r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/">
  <d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/></d:prop>
</d:propfind>"#
            .into(),
    )?;

    let doc = roxmltree::Document::parse(&xml).context("iCloud: XML inválido")?;
    let mut cals = Vec::new();
    for resp in doc.descendants().filter(|n| n.has_tag_name((NS_DAV, "response"))) {
        let Some(href) = resp.children().find(|c| c.has_tag_name((NS_DAV, "href"))).and_then(|h| h.text()) else {
            continue;
        };
        let is_calendar = resp
            .descendants()
            .filter(|n| n.has_tag_name((NS_DAV, "resourcetype")))
            .any(|rt| rt.children().any(|c| c.has_tag_name((NS_CAL, "calendar"))));
        if !is_calendar {
            continue;
        }
        let comps: Vec<&str> = resp
            .descendants()
            .filter(|n| n.has_tag_name((NS_CAL, "comp")))
            .filter_map(|n| n.attribute("name"))
            .collect();
        if !comps.is_empty() && !comps.contains(&"VEVENT") {
            continue; // lista de lembretes (VTODO)
        }
        let text_of = |ns: &str, tag: &str| {
            resp.descendants()
                .find(|n| n.has_tag_name((ns, tag)))
                .and_then(|n| n.text())
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
        };
        cals.push(CalInfo {
            url: home.join(href.trim())?,
            name: text_of(NS_DAV, "displayname").unwrap_or_else(|| "iCloud".into()),
            color: text_of(NS_APPLE, "calendar-color").as_deref().and_then(parse_hex_color).unwrap_or(DEFAULT_COLOR),
        });
    }
    Ok(cals)
}

pub fn fetch(cfg: &ICloudCfg, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Event>> {
    if cfg.apple_id.trim().is_empty() || cfg.senha_app.trim().is_empty() {
        bail!("iCloud: conecte a conta em Ajustes (Apple ID + senha de app)");
    }
    let dav = Dav { http: client(), cfg };
    let (s, e) = (from.format("%Y%m%dT%H%M%SZ"), to.format("%Y%m%dT%H%M%SZ"));
    // Pedimos ao servidor para expandir recorrências; se ele não expandir, o leitor ICS expande.
    let query = format!(
        r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><c:calendar-data><c:expand start="{s}" end="{e}"/></c:calendar-data></d:prop>
  <c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT">
    <c:time-range start="{s}" end="{e}"/>
  </c:comp-filter></c:comp-filter></c:filter>
</c:calendar-query>"#
    );

    let mut out = Vec::new();
    for cal in discover(&dav)? {
        let xml = dav.request("REPORT", &cal.url, "1", query.clone())?;
        let doc = roxmltree::Document::parse(&xml).context("iCloud: XML inválido")?;
        let mut vevents = Vec::new();
        for data in doc.descendants().filter(|n| n.has_tag_name((NS_CAL, "calendar-data"))) {
            if let Some(text) = data.text() {
                vevents.extend(ics::parse_vevents(text));
            }
        }
        out.extend(ics::expand(&vevents, from, to, &cal.name, cal.color, Source::ICloud));
    }
    Ok(out)
}

/// Testa as credenciais. Devolve os nomes dos calendários encontrados.
pub fn login(cfg: &ICloudCfg) -> Result<Vec<String>> {
    if cfg.apple_id.trim().is_empty() || cfg.senha_app.trim().is_empty() {
        bail!("iCloud: preencha o Apple ID e a senha de app");
    }
    let dav = Dav { http: client(), cfg };
    Ok(discover(&dav)?.into_iter().map(|c| c.name).collect())
}
