//! Processo de fundo: sincroniza de tempos em tempos e, quando o computador
//! desperta, abre a linha do tempo.
//!
//! Detecção de "despertar" sem APIs específicas de cada sistema: a thread dorme
//! 5 s por vez; se o relógio de parede andou muito mais que isso, a máquina
//! estava suspensa.

use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

#[allow(unused_imports)]
use anyhow::{bail, Context, Result};

use crate::config::Config;
use crate::sync;

/// Porta usada como trava de instância única do daemon.
const DAEMON_PORT: u16 = 47_613;
/// Porta em que a janela aberta escuta pedidos de "aparecer de novo".
pub const WINDOW_PORT: u16 = 47_614;

const TICK: Duration = Duration::from_secs(5);

pub fn run() -> Result<()> {
    let _lock = TcpListener::bind(("127.0.0.1", DAEMON_PORT)).context("o daemon do Linha já está rodando")?;
    println!("Linha: daemon ativo. Ctrl+C para sair.");

    let mut cfg = Config::load()?;
    if cfg.geral.abrir_ao_iniciar {
        open_window();
    }
    sync::sync_all(&cfg);

    let mut last_wall = SystemTime::now();
    let mut last_sync = Instant::now();
    loop {
        std::thread::sleep(TICK);
        let now = SystemTime::now();
        let gap = now.duration_since(last_wall).unwrap_or_default();
        last_wall = now;

        if gap > TICK + Duration::from_secs(cfg.geral.limiar_sono_seg.max(10)) {
            // acordou: espera a rede voltar, recarrega a config, abre a janela e sincroniza
            std::thread::sleep(Duration::from_secs(2));
            cfg = Config::load().unwrap_or(cfg);
            open_window();
            sync::sync_all(&cfg);
            last_sync = Instant::now();
            continue;
        }
        if last_sync.elapsed() >= Duration::from_secs(cfg.geral.intervalo_sync_min.max(1) * 60) {
            cfg = Config::load().unwrap_or(cfg);
            sync::sync_all(&cfg);
            last_sync = Instant::now();
        }
    }
}

/// Se já existe uma janela aberta, pede para ela reaparecer; senão abre uma nova.
pub fn open_window() {
    if ping_window() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    if let Err(e) = cmd.spawn() {
        eprintln!("não consegui abrir a janela: {e}");
    }
}

/// Avisa a janela já aberta para tocar a animação de novo. `true` se havia uma.
pub fn ping_window() -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], WINDOW_PORT).into(), Duration::from_millis(300)).is_ok()
}

// ---------------------------------------------------------------------------
// Iniciar com o sistema
// ---------------------------------------------------------------------------

pub fn autostart(enable: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy().to_string();
    imp::autostart(enable, &exe)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub fn autostart(enable: bool, exe: &str) -> Result<()> {
        let dir = directories::BaseDirs::new().context("sem pasta do usuário")?.config_dir().join("autostart");
        let file = dir.join("linha.desktop");
        if !enable {
            let _ = std::fs::remove_file(&file);
            println!("Início automático desativado.");
            return Ok(());
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            &file,
            format!(
                "[Desktop Entry]\nType=Application\nName=Linha\nComment=Linha do tempo da agenda\n\
                 Exec=\"{exe}\" daemon\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"
            ),
        )?;
        println!("Início automático ativado: {}", file.display());
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn autostart(enable: bool, exe: &str) -> Result<()> {
        let home = directories::BaseDirs::new().context("sem pasta do usuário")?.home_dir().to_path_buf();
        let file = home.join("Library/LaunchAgents/app.linha.daemon.plist");
        let _ = Command::new("launchctl").args(["unload", "-w"]).arg(&file).output();
        if !enable {
            let _ = std::fs::remove_file(&file);
            println!("Início automático desativado.");
            return Ok(());
        }
        std::fs::create_dir_all(file.parent().unwrap())?;
        let exe = exe.replace('&', "&amp;").replace('<', "&lt;");
        std::fs::write(
            &file,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>app.linha.daemon</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>daemon</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
</dict></plist>
"#
            ),
        )?;
        let out = Command::new("launchctl").args(["load", "-w"]).arg(&file).output()?;
        if !out.status.success() {
            bail!("launchctl: {}", String::from_utf8_lossy(&out.stderr));
        }
        println!("Início automático ativado: {}", file.display());
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn autostart(enable: bool, exe: &str) -> Result<()> {
        let out = if enable {
            Command::new("reg")
                .args(["add", KEY, "/v", "Linha", "/t", "REG_SZ", "/f", "/d"])
                .arg(format!("\"{exe}\" daemon"))
                .output()?
        } else {
            Command::new("reg").args(["delete", KEY, "/v", "Linha", "/f"]).output()?
        };
        if enable && !out.status.success() {
            bail!("reg: {}", String::from_utf8_lossy(&out.stderr));
        }
        println!("Início automático {}.", if enable { "ativado" } else { "desativado" });
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::*;
    pub fn autostart(_enable: bool, _exe: &str) -> Result<()> {
        bail!("início automático não suportado neste sistema")
    }
}
