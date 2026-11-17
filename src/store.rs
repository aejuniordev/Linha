//! Caminhos no disco, escrita atômica com permissão restrita e tokens OAuth.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Pastas do usuário:
/// - Windows: `%APPDATA%\Linha\config` e `%APPDATA%\Linha\data`
/// - macOS:   `~/Library/Application Support/Linha`
/// - Linux:   `~/.config/linha` e `~/.local/share/linha`
fn dirs() -> ProjectDirs {
    let name = if cfg!(any(windows, target_os = "macos")) { "Linha" } else { "linha" };
    ProjectDirs::from_path(PathBuf::from(name)).expect("não foi possível achar a pasta do usuário")
}

/// Abre a pasta no gerenciador de arquivos do sistema.
pub fn reveal(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

pub fn config_dir() -> PathBuf {
    dirs().config_dir().to_path_buf()
}

pub fn data_dir() -> PathBuf {
    dirs().data_dir().to_path_buf()
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn cache_path() -> PathBuf {
    data_dir().join("cache.json")
}

/// Tokens de uma conta (`id` em `Config::contas`; as contas antigas usam o nome do serviço).
fn token_path(account: &str) -> PathBuf {
    // o id é gerado pelo Linha, mas não custa garantir um nome de arquivo seguro
    let safe: String = account.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    data_dir().join("tokens").join(format!("{safe}.json"))
}

/// Trava entre processos (janela e daemon podem sincronizar ao mesmo tempo).
/// Devolve `None` se outro processo segura a trava há menos de `stale`.
pub fn try_lock(name: &str, stale: std::time::Duration) -> Option<LockGuard> {
    let path = data_dir().join(format!("{name}.lock"));
    let _ = std::fs::create_dir_all(data_dir());
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or_default() < stale);
    if fresh {
        return None;
    }
    let _ = std::fs::remove_file(&path);
    std::fs::OpenOptions::new().write(true).create_new(true).open(&path).ok()?;
    Some(LockGuard(path))
}

pub struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Escreve em arquivo temporário e renomeia (atômico). Em Unix, permissão 0600.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("criando {}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("gravando {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("renomeando para {}", path.display()))?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    /// Permissões concedidas. `None` em logins antigos (antes da escrita existir).
    #[serde(default)]
    pub scope: Option<String>,
}

impl Tokens {
    /// Monta os tokens a partir da resposta; o que faltar (refresh, escopo) vem dos anteriores.
    pub fn from_response(r: &TokenResponse, previous: Option<&Tokens>) -> Tokens {
        Tokens {
            access_token: r.access_token.clone(),
            refresh_token: r.refresh_token.clone().or_else(|| previous.and_then(|p| p.refresh_token.clone())),
            expires_at: Utc::now() + Duration::seconds(r.expires_in.unwrap_or(3600) as i64),
            scope: r.scope.clone().or_else(|| previous.and_then(|p| p.scope.clone())),
        }
    }

    /// O login concedeu esta permissão?
    pub fn has_scope(&self, needle: &str) -> bool {
        self.scope.as_deref().is_some_and(|s| s.contains(needle))
    }

    pub fn is_fresh(&self) -> bool {
        self.expires_at - Duration::seconds(90) > Utc::now()
    }

    pub fn load(account: &str) -> Option<Tokens> {
        let s = std::fs::read_to_string(token_path(account)).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn save(&self, account: &str) -> Result<()> {
        write_private(&token_path(account), serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn delete(account: &str) {
        let _ = std::fs::remove_file(token_path(account));
    }
}

/// Resposta padrão de um endpoint de token OAuth 2.0.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    /// Permissões concedidas (separadas por espaço).
    pub scope: Option<String>,
}
