//! Sincroniza todas as contas ativas em paralelo e atualiza o cache.

use chrono::{Duration, Local, Utc};

use crate::config::Config;
use crate::model::{local_midnight, Cache, Source, SyncStatus};
use crate::providers;

/// Janela buscada: de hoje 00:00 até o fim do último dia exibido.
pub fn window(cfg: &Config) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let today = Local::now().date_naive();
    let days = cfg.geral.dias.clamp(1, 14) as i64;
    let from = local_midnight(today).with_timezone(&Utc);
    let to = local_midnight(today + Duration::days(days)).with_timezone(&Utc);
    (from, to)
}

pub fn sync_all(cfg: &Config) -> Cache {
    let mut cache = Cache::load();
    let (from, to) = window(cfg);

    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = Source::ALL
            .into_iter()
            .filter(|src| providers::enabled(cfg, *src))
            .map(|src| (src, s.spawn(move || providers::fetch(cfg, src, from, to))))
            .collect();
        handles
            .into_iter()
            .map(|(src, h)| (src, h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("falha interna")))))
            .collect()
    });

    // contas desativadas somem da tela
    cache.events.retain(|e| providers::enabled(cfg, e.source));
    cache.status.retain(|src, _| providers::enabled(cfg, *src));

    for (src, res) in results {
        match res {
            Ok(events) => {
                cache.events.retain(|e| e.source != src);
                let count = events.len();
                cache.events.extend(events);
                cache.status.insert(src, SyncStatus { ok: true, message: None, at: Some(Utc::now()), count });
            }
            Err(err) => {
                // mantém os eventos antigos dessa conta e registra o erro
                let prev = cache.status.get(&src).and_then(|s| s.at);
                cache.status.insert(
                    src,
                    SyncStatus { ok: false, message: Some(format!("{err:#}")), at: prev, count: 0 },
                );
            }
        }
    }
    // descarta o que ficou para trás
    cache.events.retain(|e| e.local_end().with_timezone(&Utc) >= from || e.local_start().with_timezone(&Utc) >= from);
    cache.updated = Some(Utc::now());
    if let Err(e) = cache.save() {
        eprintln!("aviso: não consegui gravar o cache: {e:#}");
    }
    cache
}
