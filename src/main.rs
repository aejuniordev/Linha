//! Linha — a linha do tempo do seu dia (Outlook, Google e iCloud) num relance.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod blocks;
mod config;
mod daemon;
mod demo;
mod model;
mod providers;
mod store;
mod sync;
mod ui;
mod update;

use anyhow::{bail, Result};

use config::Config;
use model::{Source, When};

/// No Windows o Linha fica na área de notificação; nos outros sistemas, janela + daemon.
const RESIDENT: bool = cfg!(windows);

const HELP: &str = "\
Linha — a linha do tempo do seu dia

USO
  linha                     abre a linha do tempo
  linha ajustes             abre a tela de Ajustes (contas e preferências)
  linha demo                abre com uma agenda de exemplo
  linha daemon              roda em segundo plano: sincroniza e abre ao despertar
  linha sync                sincroniza agora e lista os eventos no terminal
  linha login <serviço>     conecta mais uma conta outlook | google pelo navegador
  linha logout <conta>      desconecta uma conta (e-mail, ou o serviço se só houver uma)
  linha contas              lista as contas conectadas
  linha autostart on|off    inicia o daemon junto com o sistema
  linha pasta               mostra onde ficam as configurações

As contas e preferências ficam na tela de Ajustes (ícone de engrenagem).
";

fn main() {
    #[cfg(windows)]
    attach_console();

    if let Err(e) = run() {
        eprintln!("erro: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // flags internas, usadas quando o Linha se reinicia depois de atualizar
    let restarted = take_flag(&mut args, "--reiniciado");
    let hidden = take_flag(&mut args, "--oculto");
    if restarted {
        wait_previous(args.first().map(String::as_str) == Some("daemon"));
    }
    update::cleanup();
    let arg = |i: usize| args.get(i).map(String::as_str);

    match arg(0) {
        None => {
            // já existe uma janela aberta (ou o Linha está na bandeja)? só pede para aparecer
            if daemon::ping_window() {
                return Ok(());
            }
            ui::run(Config::load()?, ui::Mode { resident: RESIDENT.then_some(!hidden), ..Default::default() })
        }
        Some("ajustes" | "config" | "settings") => {
            if daemon::ping_window_with(b"s") {
                return Ok(());
            }
            let mode = ui::Mode { settings: true, resident: RESIDENT.then_some(true), ..Default::default() };
            ui::run(Config::load().unwrap_or_default(), mode)
        }
        Some("demo") => ui::run(Config::load().unwrap_or_default(), ui::Mode { demo: true, ..Default::default() }),
        Some("daemon") if RESIDENT => {
            // no Windows o "daemon" é o próprio Linha na área de notificação
            if daemon::ping_window_with(b"-") {
                return Ok(()); // já está rodando; não precisa aparecer
            }
            let cfg = Config::load().unwrap_or_default();
            let show = cfg.geral.abrir_ao_iniciar;
            ui::run(cfg, ui::Mode { resident: Some(show), ..Default::default() })
        }
        Some("daemon") => daemon::run(!restarted),
        Some("pasta") => {
            println!("configuração: {}", store::config_path().display());
            println!("dados:        {}", store::data_dir().display());
            Ok(())
        }
        Some("login") => {
            let mut cfg = Config::load()?;
            let flow = providers::oauth::Flow::default();
            let src = parse_source(arg(1))?;
            let (tokens, who) = match src {
                Source::Outlook => providers::outlook::login(&cfg.outlook, &flow)?,
                Source::Google => providers::google::login(&cfg.google, &flow)?,
                Source::ICloud => bail!("o iCloud usa Apple ID + senha de app: conecte pela tela de Ajustes (`linha ajustes`)"),
            };
            let id = cfg.upsert_account(src, &who, None);
            tokens.save(&id)?;
            cfg.save()?;
            println!("{} conectado: {}", src.label(), cfg.account_label(&id));
            Ok(())
        }
        Some("logout") => {
            let mut cfg = Config::load()?;
            let id = find_account(&cfg, arg(1))?;
            let label = cfg.account_label(&id);
            cfg.remove_account(&id);
            cfg.save()?;
            println!("{label}: desconectado.");
            Ok(())
        }
        Some("contas") => {
            let cfg = Config::load()?;
            if cfg.contas.is_empty() {
                println!("nenhuma conta conectada");
            }
            for a in &cfg.contas {
                let principal = cfg.principal.as_ref().is_some_and(|p| p.account == a.id);
                println!(
                    "{:<8} {}{}{}",
                    a.servico.label(),
                    cfg.account_label(&a.id),
                    if a.ativo { "" } else { "  (oculta)" },
                    if principal { "  ← principal" } else { "" }
                );
            }
            Ok(())
        }
        Some("sync") => print_sync(),
        Some("autostart") => {
            let on = match arg(1) {
                Some("on") => true,
                Some("off") => false,
                _ => bail!("use: linha autostart on|off"),
            };
            daemon::autostart(on)?;
            let mut cfg = Config::load()?;
            cfg.geral.iniciar_com_sistema = on;
            cfg.save()
        }
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => bail!("comando desconhecido: {other}\n\n{HELP}"),
    }
}

/// Acha a conta por e-mail, id ou serviço (este só se houver uma conta dele).
fn find_account(cfg: &Config, s: Option<&str>) -> Result<String> {
    let s = s.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("diga qual conta (veja `linha contas`)"))?;
    if let Some(a) = cfg.contas.iter().find(|a| a.id == s || a.nome.eq_ignore_ascii_case(s)) {
        return Ok(a.id.clone());
    }
    let src = parse_source(Some(s))?;
    let same: Vec<_> = cfg.contas.iter().filter(|a| a.servico == src).collect();
    match same.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => bail!("nenhuma conta {} conectada", src.label()),
        _ => bail!("há mais de uma conta {}: diga o e-mail (veja `linha contas`)", src.label()),
    }
}

fn parse_source(s: Option<&str>) -> Result<Source> {
    match s.map(str::to_ascii_lowercase).as_deref() {
        Some("outlook" | "microsoft" | "office" | "hotmail") => Ok(Source::Outlook),
        Some("google" | "gmail") => Ok(Source::Google),
        Some("icloud" | "apple") => Ok(Source::ICloud),
        _ => bail!("escolha a conta: outlook, google ou icloud"),
    }
}

fn print_sync() -> Result<()> {
    let cfg = Config::load()?;
    if !cfg.any_enabled() {
        bail!("nenhuma conta conectada — abra os Ajustes (`linha ajustes`) e conecte pelo menos uma");
    }
    let cache = sync::sync_all(&cfg);
    for (id, st) in &cache.status {
        let name = cfg.account_label(id);
        match (&st.ok, &st.message) {
            (true, _) => println!("✓ {name}  {} evento(s)", st.count),
            (false, Some(m)) => println!("✗ {name}  {m}"),
            (false, None) => println!("✗ {name}"),
        }
    }
    println!();
    let today = chrono::Local::now().date_naive();
    for d in 0..cfg.geral.dias.clamp(1, 14) {
        let day = today + chrono::Duration::days(d as i64);
        println!("{}", day.format("%d/%m"));
        for e in cache.events_on(day) {
            let when = match e.when {
                When::AllDay { .. } => "dia todo     ".to_string(),
                When::Timed { .. } => format!("{}–{}  ", e.local_start().format("%H:%M"), e.local_end().format("%H:%M")),
            };
            println!("  {when}{}  ({} · {})", e.title, e.calendar, cfg.account_label(&e.account));
        }
    }
    Ok(())
}

/// No Windows o executável não abre console; se veio de um terminal, usa o dele.
#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX);
    }
}

/// Remove `flag` dos argumentos; devolve se estava lá.
fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// Depois de uma atualização, espera o processo antigo liberar as portas (até 15 s).
fn wait_previous(daemon: bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let busy = || if daemon && !RESIDENT { daemon::daemon_running() } else { daemon::ping_window_with(b"-") };
    while busy() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
