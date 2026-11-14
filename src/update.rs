//! Atualização automática pelas releases do GitHub.
//!
//! Procura a última release, compara com a versão atual e, se houver uma mais
//! nova, baixa o executável da plataforma (`linha-<target>[.exe]`), confere o
//! SHA-256 publicado ao lado e troca o executável em uso. A troca vale a partir
//! do próximo início; quem chama decide quando reiniciar (veja [`pending`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Repositório das releases (`dono/nome`). Pode ser trocado no build com `LINHA_REPO`.
const REPO: &str = match option_env!("LINHA_REPO") {
    Some(r) => r,
    None => "aejuniordev/Linha",
};
/// Plataforma deste executável (vem do build.rs).
const TARGET: &str = env!("LINHA_TARGET");
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Intervalo entre verificações automáticas.
pub const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Versão já instalada no disco, esperando o Linha reiniciar.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

pub enum Outcome {
    UpToDate,
    Installed(String),
}

/// Builds de desenvolvimento (`cargo run`) não se atualizam sozinhos.
pub fn supported() -> bool {
    !cfg!(debug_assertions)
}

/// Versão baixada que ainda não está rodando (o Linha precisa reiniciar).
pub fn pending() -> Option<String> {
    PENDING.lock().unwrap().clone()
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("Linha/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// Procura uma versão nova e, se houver, instala. Não reinicia nada.
pub fn check_and_install() -> Result<Outcome> {
    if let Some(v) = pending() {
        return Ok(Outcome::Installed(v));
    }
    let http = http()?;
    let rel: Release = http
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("sem conexão com o GitHub")?
        .error_for_status()
        .context("não consegui consultar as releases")?
        .json()?;

    let latest = rel.tag_name.trim_start_matches('v').to_string();
    if !newer(&latest, CURRENT) {
        return Ok(Outcome::UpToDate);
    }

    let name = format!("linha-{TARGET}{}", std::env::consts::EXE_SUFFIX);
    let url_of = |n: &str| rel.assets.iter().find(|a| a.name == n).map(|a| a.browser_download_url.clone());
    let bin_url = url_of(&name).ok_or_else(|| anyhow!("a versão {latest} não tem executável para {TARGET}"))?;
    let sum_url = url_of(&format!("{name}.sha256")).ok_or_else(|| anyhow!("a versão {latest} não tem checksum"))?;

    let expected = http.get(sum_url).send()?.error_for_status()?.text()?;
    let expected = expected.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let bytes = http.get(bin_url).send().context("baixando a atualização")?.error_for_status()?.bytes()?;
    let actual: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if expected.len() != 64 || actual != expected {
        bail!("o download não confere com o checksum publicado; atualização descartada");
    }

    install(&bytes)?;
    *PENDING.lock().unwrap() = Some(latest.clone());
    Ok(Outcome::Installed(latest))
}

fn exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// Troca o executável em uso pelo novo. No Windows não dá para sobrescrever um
/// .exe rodando, mas dá para renomeá-lo: o antigo vira `.old` e é apagado no
/// próximo início.
fn install(bytes: &[u8]) -> Result<()> {
    let exe = exe_path()?;
    let new = sibling(&exe, ".new");
    let old = sibling(&exe, ".old");
    std::fs::write(&new, bytes).with_context(|| format!("sem permissão para gravar em {}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).context("não consegui mover o executável atual")?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe); // desfaz
        return Err(e).context("não consegui instalar o novo executável");
    }
    Ok(())
}

/// Apaga o executável antigo deixado pela última atualização.
pub fn cleanup() {
    if let Ok(exe) = exe_path() {
        let _ = std::fs::remove_file(sibling(&exe, ".old"));
        let _ = std::fs::remove_file(sibling(&exe, ".new"));
    }
}

/// Inicia o executável (já atualizado) com estes argumentos. O novo processo
/// espera este sair antes de ocupar as portas (veja `--reiniciado` no main).
pub fn relaunch(args: &[&str]) -> Result<()> {
    let mut cmd = std::process::Command::new(exe_path()?);
    cmd.args(args).arg("--reiniciado");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    cmd.spawn().context("não consegui reiniciar o Linha")?;
    Ok(())
}

/// `a` é mais nova que `b`? (comparação numérica de "1.2.3"; sufixos como "-beta" contam como 0)
fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
    };
    parse(a) > parse(b)
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compara_versoes() {
        assert!(newer("0.5.0", "0.4.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.4.0", "0.4.0"));
        assert!(!newer("0.3.9", "0.4.0"));
    }
}
