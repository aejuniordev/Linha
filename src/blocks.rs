//! Bloqueio de horários: para cada horário ocupado nos calendários escolhidos das
//! outras contas, mantém um evento "Ocupado" no calendário principal — cria quando
//! aparece, apaga quando o original some ou muda de horário.
//!
//! Os bloqueios são reconhecidos de dois jeitos (qualquer um basta): uma marca no
//! próprio evento (propriedade estendida no Google/Outlook, `X-LINHA-BLOQUEIO` no
//! iCloud) e a lista local dos que o Linha criou (`bloqueios.json`). Assim um
//! bloqueio nunca é tratado como compromisso de verdade — nem recriado em dobro.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{CalRef, Config, Principal};
use crate::model::{Cache, Event, NewEvent, Reminder, ShowAs, When, DEFAULT_COLOR};
use crate::{providers, store};

/// Quantos dias à frente os bloqueios são mantidos.
pub const HORIZON_DAYS: i64 = 14;
/// Limite de criações/remoções por rodada (o resto fica para a próxima).
const MAX_OPS: usize = 60;
pub const TITLE: &str = "Ocupado";
const DESCRIPTION: &str = "Horário ocupado em outra agenda. Criado e mantido automaticamente pelo Linha.";

type Span = (DateTime<Utc>, DateTime<Utc>);

/// Um bloqueio criado pelo Linha (lista local, para reconhecer mesmo sem a marca).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct Created {
    account: String,
    calendar_id: String,
    uid: String,
    end: DateTime<Utc>,
}

fn state_path() -> std::path::PathBuf {
    store::data_dir().join("bloqueios.json")
}

fn load_state() -> Vec<Created> {
    std::fs::read_to_string(state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_state(v: &[Created]) -> Result<()> {
    store::write_private(&state_path(), serde_json::to_string_pretty(v)?.as_bytes())
}

/// Horários ocupados nos calendários de origem, unidos quando se sobrepõem.
/// Ficam de fora: eventos de dia inteiro, marcados como disponíveis, já terminados,
/// os da própria conta principal e os bloqueios.
pub fn busy_spans(cfg: &Config, events: &[Event], dest_account: &str, now: DateTime<Utc>) -> Vec<Span> {
    let origins: HashSet<&CalRef> = cfg.bloqueio.origens.iter().collect();
    let spans = events
        .iter()
        .filter(|e| !e.block && !e.free && e.account != dest_account && cfg.active(&e.account))
        .filter(|e| origins.contains(&CalRef { conta: e.account.clone(), id: e.calendar_id.clone() }))
        .filter_map(|e| match e.when {
            When::Timed { start, end } if end > now && end > start => Some((start, end)),
            _ => None,
        })
        .collect();
    merge(spans)
}

/// Une intervalos que se sobrepõem (encostados ficam separados).
pub fn merge(mut v: Vec<Span>) -> Vec<Span> {
    v.sort();
    let mut out: Vec<Span> = Vec::with_capacity(v.len());
    for (s, e) in v {
        match out.last_mut() {
            Some(last) if s < last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// O que falta criar e o que sobra (índices em `existing`), casando por horário.
/// Bloqueios repetidos no mesmo horário também sobram.
pub fn diff(desired: &[Span], existing: &[Span]) -> (Vec<Span>, Vec<usize>) {
    let mut used = vec![false; existing.len()];
    let mut create = Vec::new();
    for d in desired {
        match existing.iter().enumerate().position(|(i, x)| !used[i] && x == d) {
            Some(i) => used[i] = true,
            None => create.push(*d),
        }
    }
    let extra = used.iter().enumerate().filter(|(_, u)| !**u).map(|(i, _)| i).collect();
    (create, extra)
}

/// Destino válido: o calendário principal, numa conta ativa que pode escrever.
fn destination(cfg: &Config) -> Option<&Principal> {
    if !cfg.bloqueio.ativo {
        return None;
    }
    let p = cfg.principal.as_ref()?;
    let acct = cfg.account(&p.account).filter(|a| a.ativo)?;
    (providers::connected(acct) && providers::can_write(acct)).then_some(p)
}

fn ends(e: &Event) -> DateTime<Utc> {
    e.local_end().with_timezone(&Utc)
}

/// Acerta os bloqueios a partir do cache recém-sincronizado. Contas com falha na
/// sincronização (`failed`) suspendem a rodada: sem saber a agenda delas, apagar
/// ou criar bloqueios poderia estar errado.
pub fn reconcile(cfg: &Config, cache: &mut Cache, failed: &HashSet<String>) -> Result<()> {
    let now = Utc::now();
    let mut state = load_state();
    // bloqueios que perderam a marca (ou serviços que não a devolvem) ainda são bloqueios
    let known: HashSet<(&str, &str)> = state.iter().map(|c| (c.account.as_str(), c.uid.as_str())).collect();
    for e in cache.events.iter_mut().filter(|e| !e.block) {
        if known.contains(&(e.account.as_str(), e.uid.as_str())) {
            e.block = true;
        }
    }

    let dest = destination(cfg);
    let has_blocks = cache.events.iter().any(|e| e.block && ends(e) > now);
    if dest.is_none() && !has_blocks {
        return Ok(());
    }

    let mut involved: HashSet<&str> = cfg.bloqueio.origens.iter().map(|c| c.conta.as_str()).collect();
    involved.extend(dest.map(|d| d.account.as_str()));
    involved.extend(cache.events.iter().filter(|e| e.block).map(|e| e.account.as_str()));
    if involved.iter().any(|a| failed.contains(*a)) {
        return Ok(());
    }

    // janela e daemon podem sincronizar juntos: só um mexe nos bloqueios por vez
    let Some(_lock) = store::try_lock("bloqueio", std::time::Duration::from_secs(120)) else { return Ok(()) };

    let in_dest = |e: &Event| dest.is_some_and(|d| e.account == d.account && e.calendar_id == d.id);
    let current: Vec<&Event> = cache.events.iter().filter(|e| e.block && ends(e) > now && in_dest(e)).collect();
    let stray: Vec<Event> = cache.events.iter().filter(|e| e.block && ends(e) > now && !in_dest(e)).cloned().collect();

    let desired = dest.map(|d| busy_spans(cfg, &cache.events, &d.account, now)).unwrap_or_default();
    let spans: Vec<Span> = current.iter().filter_map(|e| span_of(e)).collect();
    let (to_create, extra) = diff(&desired, &spans);
    let mut to_delete: Vec<Event> = stray;
    to_delete.extend(extra.into_iter().map(|i| current[i].clone()));

    let mut ops = 0;
    let mut first_err: Option<anyhow::Error> = None;

    for e in to_delete {
        if ops >= MAX_OPS {
            break;
        }
        ops += 1;
        let Some(acct) = cfg.account(&e.account) else { continue };
        match providers::delete(cfg, acct, &e.calendar_id, &e.uid) {
            Ok(()) => {
                cache.events.retain(|x| x.id != e.id);
                state.retain(|c| !(c.account == e.account && c.uid == e.uid));
            }
            Err(err) => {
                first_err.get_or_insert(err);
            }
        }
    }

    if let Some(d) = dest {
        let acct = cfg.account(&d.account).ok_or_else(|| anyhow!("conta principal sumiu"))?;
        let color = cache
            .calendars
            .iter()
            .find(|c| c.account == d.account && c.id == d.id)
            .map(|c| c.color)
            .unwrap_or(DEFAULT_COLOR);
        for (start, end) in to_create {
            if ops >= MAX_OPS {
                break;
            }
            ops += 1;
            let ev = NewEvent {
                title: TITLE.into(),
                when: When::Timed { start, end },
                location: None,
                description: Some(DESCRIPTION.into()),
                repeat: None,
                reminder: Reminder::Off,
                show_as: ShowAs::Busy,
                private: true,
                attendees: Vec::new(),
                online_meeting: None,
                block: true,
            };
            match providers::create(cfg, acct, &d.id, &ev) {
                Ok(uid) => {
                    state.push(Created { account: d.account.clone(), calendar_id: d.id.clone(), uid: uid.clone(), end });
                    cache.events.push(Event {
                        id: format!("{}:{uid}", d.account),
                        title: TITLE.into(),
                        when: ev.when,
                        location: None,
                        calendar: d.name.clone(),
                        color,
                        source: d.source,
                        link: None,
                        account: d.account.clone(),
                        calendar_id: d.id.clone(),
                        uid,
                        free: false,
                        block: true,
                    });
                }
                Err(err) => {
                    first_err.get_or_insert(err);
                    break; // provavelmente vale para todos (permissão, rede…)
                }
            }
        }
    }

    // esquece o que já passou há mais de um dia
    state.retain(|c| c.end > now - Duration::days(1));
    save_state(&state)?;
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn span_of(e: &Event) -> Option<Span> {
    match e.when {
        When::Timed { start, end } => Some((start, end)),
        When::AllDay { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
    }

    #[test]
    fn une_sobrepostos() {
        let v = vec![(t(10, 0), t(11, 0)), (t(9, 0), t(10, 30)), (t(11, 0), t(12, 0)), (t(14, 0), t(15, 0))];
        assert_eq!(merge(v), vec![(t(9, 0), t(11, 0)), (t(11, 0), t(12, 0)), (t(14, 0), t(15, 0))]);
    }

    #[test]
    fn diferenca_por_horario() {
        let desired = vec![(t(9, 0), t(10, 0)), (t(13, 0), t(14, 0))];
        let existing = vec![(t(9, 0), t(10, 0)), (t(9, 0), t(10, 0)), (t(16, 0), t(17, 0))];
        let (create, extra) = diff(&desired, &existing);
        assert_eq!(create, vec![(t(13, 0), t(14, 0))]);
        // o repetido e o que não existe mais saem
        assert_eq!(extra, vec![1, 2]);
    }
}
