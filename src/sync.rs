//! Sincroniza todas as contas ativas em paralelo, atualiza o cache e, se ligado,
//! mantém os bloqueios de horário no calendário principal.

use std::collections::HashSet;

use chrono::{Duration, Local, Utc};

use crate::config::Config;
use crate::model::{local_midnight, Cache, SyncStatus};
use crate::{blocks, providers};

/// Janela buscada: de hoje 00:00 até o fim do último dia exibido (com o bloqueio
/// ligado, pelo menos [`blocks::HORIZON_DAYS`] dias, para os bloqueios irem adiante).
pub fn window(cfg: &Config) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let today = Local::now().date_naive();
    let mut days = cfg.geral.dias.clamp(1, 14) as i64;
    if cfg.bloqueio.ativo {
        days = days.max(blocks::HORIZON_DAYS);
    }
    let from = local_midnight(today).with_timezone(&Utc);
    let to = local_midnight(today + Duration::days(days)).with_timezone(&Utc);
    (from, to)
}

pub fn sync_all(cfg: &Config) -> Cache {
    let mut cache = Cache::load();
    let (from, to) = window(cfg);

    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = cfg
            .active_accounts()
            .map(|acct| (acct.id.clone(), s.spawn(move || providers::fetch(cfg, acct, from, to))))
            .collect();
        handles
            .into_iter()
            .map(|(id, h)| (id, h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("falha interna")))))
            .collect()
    });

    // contas desativadas ou removidas somem da tela
    cache.events.retain(|e| cfg.active(&e.account));
    cache.status.retain(|id, _| cfg.active(id));
    cache.calendars.retain(|c| cfg.active(&c.account));

    let mut failed = HashSet::new();
    for (id, res) in results {
        match res {
            Ok(f) => {
                cache.events.retain(|e| e.account != id);
                cache.calendars.retain(|c| c.account != id);
                cache.calendars.extend(f.calendars);
                let count = f.events.iter().filter(|e| !e.block).count();
                cache.events.extend(f.events);
                cache.status.insert(id, SyncStatus { ok: true, message: None, at: Some(Utc::now()), count, who: f.who });
            }
            Err(err) => {
                // mantém os eventos antigos dessa conta e registra o erro
                let prev = cache.status.get(&id);
                let (at, who) = (prev.and_then(|s| s.at), prev.and_then(|s| s.who.clone()));
                cache.status.insert(id.clone(), SyncStatus { ok: false, message: Some(format!("{err:#}")), at, count: 0, who });
                failed.insert(id);
            }
        }
    }
    // descarta o que ficou para trás
    cache.events.retain(|e| e.local_end().with_timezone(&Utc) >= from || e.local_start().with_timezone(&Utc) >= from);

    if let Err(e) = blocks::reconcile(cfg, &mut cache, &failed) {
        eprintln!("aviso: bloqueio de horários: {e:#}");
    }

    cache.updated = Some(Utc::now());
    if let Err(e) = cache.save() {
        eprintln!("aviso: não consegui gravar o cache: {e:#}");
    }
    cache
}
