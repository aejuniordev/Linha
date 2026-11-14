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
use crate::{sync, update};

/// Porta usada como trava de instância única do daemon.
const DAEMON_PORT: u16 = 47_613;
/// Porta em que a janela aberta escuta pedidos de "aparecer de novo".
pub const WINDOW_PORT: u16 = 47_614;

const TICK: Duration = Duration::from_secs(5);

/// `greet`: abre a linha do tempo ao iniciar (se configurado). Falso ao voltar de uma atualização.
pub fn run(greet: bool) -> Result<()> {
    let _lock = TcpListener::bind(("127.0.0.1", DAEMON_PORT)).context("o daemon do Linha já está rodando")?;
    println!("Linha: daemon ativo. Ctrl+C para sair.");

    let mut cfg = Config::load()?;
    if greet && cfg.geral.abrir_ao_iniciar {
        open_window();
    }
    sync::sync_all(&cfg);

    let mut last_wall = SystemTime::now();
    let mut last_sync = Instant::now();
    // primeira verificação de atualização um minuto depois de iniciar
    let mut next_update = Instant::now() + Duration::from_secs(60);
    loop {
        std::thread::sleep(TICK);
        if Instant::now() >= next_update {
            next_update = Instant::now() + update::EVERY;
            if cfg.geral.atualizar_auto && update::supported() {
                if let Ok(update::Outcome::Installed(v)) = update::check_and_install() {
                    println!("Linha {v} instalado; reiniciando o daemon.");
                    // o novo processo espera este sair (e liberar a trava) antes de começar
                    if update::relaunch(&["daemon"]).is_ok() {
                        std::process::exit(0);
                    }
                }
            }
        }
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

/// Inicia o daemon em segundo plano agora (se já estiver rodando, o novo sai sozinho).
pub fn spawn() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = Command::new(exe);
    cmd.arg("daemon");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    let _ = cmd.spawn();
}

/// Já existe um daemon rodando (macOS/Linux)?
pub fn daemon_running() -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], DAEMON_PORT).into(), Duration::from_millis(300)).is_ok()
}

/// Avisa a janela já aberta para aparecer de novo. `true` se havia uma.
pub fn ping_window() -> bool {
    ping_window_with(b"r")
}

/// Como [`ping_window`], mandando um pedido (`b"s"` = abrir nos Ajustes).
pub fn ping_window_with(msg: &[u8]) -> bool {
    use std::io::Write;
    #[cfg(windows)]
    {
        // deixa a janela do outro processo vir para a frente
        #[link(name = "user32")]
        extern "system" {
            fn AllowSetForegroundWindow(pid: u32) -> i32;
        }
        unsafe { AllowSetForegroundWindow(u32::MAX) };
    }
    match TcpStream::connect_timeout(&([127, 0, 0, 1], WINDOW_PORT).into(), Duration::from_millis(300)) {
        Ok(mut s) => {
            let _ = s.write_all(msg);
            true
        }
        Err(_) => false,
    }
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

    /// `reg.exe` sem abrir janela de console (a interface chama isto).
    fn reg() -> Command {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new("reg");
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    pub fn autostart(enable: bool, exe: &str) -> Result<()> {
        let out = if enable {
            reg()
                .args(["add", KEY, "/v", "Linha", "/t", "REG_SZ", "/f", "/d"])
                .arg(format!("\"{exe}\" daemon"))
                .output()?
        } else {
            reg().args(["delete", KEY, "/v", "Linha", "/f"]).output()?
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
