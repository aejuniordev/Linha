//! Linha — a linha do tempo do seu dia (Outlook, Google e iCloud) num relance.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod daemon;
mod demo;
mod model;
mod providers;
mod store;
mod sync;
mod ui;

use anyhow::{bail, Result};

use config::Config;
use model::{Source, When};

const HELP: &str = "\
Linha — a linha do tempo do seu dia

USO
  linha                     abre a linha do tempo
  linha demo                abre com uma agenda de exemplo
  linha daemon              roda em segundo plano: sincroniza e abre ao despertar
  linha config              cria/mostra o arquivo de configuração
  linha login <conta>       conecta outlook | google | icloud
  linha logout <conta>      esquece o login de uma conta
  linha sync                sincroniza agora e lista os eventos no terminal
  linha autostart on|off    inicia o daemon junto com o sistema
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);

    match arg(0) {
        None => {
            // já existe uma janela aberta? só pede para ela reaparecer
            if daemon::ping_window() {
                return Ok(());
            }
            ui::run(Config::load()?, false)
        }
        Some("demo") => ui::run(Config::load().unwrap_or_default(), true),
        Some("daemon") => daemon::run(),
        Some("config") => {
            let path = Config::ensure_file()?;
            println!("{}", path.display());
            Ok(())
        }
        Some("login") => {
            let cfg = Config::load()?;
            match parse_source(arg(1))? {
                Source::Outlook => providers::outlook::login(&cfg.outlook),
                Source::Google => providers::google::login(&cfg.google),
                Source::ICloud => providers::icloud::login(&cfg.icloud),
            }
        }
        Some("logout") => {
            let src = parse_source(arg(1))?;
            store::Tokens::delete(src);
            println!("{}: login esquecido.", src.label());
            Ok(())
        }
        Some("sync") => print_sync(),
        Some("autostart") => match arg(1) {
            Some("on") => daemon::autostart(true),
            Some("off") => daemon::autostart(false),
            _ => bail!("use: linha autostart on|off"),
        },
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => bail!("comando desconhecido: {other}\n\n{HELP}"),
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
        bail!("nenhuma conta ativa — rode `linha config` e ative pelo menos uma");
    }
    let cache = sync::sync_all(&cfg);
    for (src, st) in &cache.status {
        match (&st.ok, &st.message) {
            (true, _) => println!("✓ {:<8} {} evento(s)", src.label(), st.count),
            (false, Some(m)) => println!("✗ {:<8} {m}", src.label()),
            (false, None) => println!("✗ {}", src.label()),
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
            println!("  {when}{}  ({} · {})", e.title, e.calendar, e.source.label());
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
