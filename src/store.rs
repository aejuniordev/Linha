//! Caminhos no disco, escrita atômica com permissão restrita e tokens OAuth.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::model::Source;

fn dirs() -> ProjectDirs {
    ProjectDirs::from("app", "linha", "Linha").expect("não foi possível achar a pasta do usuário")
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

fn token_path(src: Source) -> PathBuf {
    data_dir().join("tokens").join(format!("{}.json", src.key()))
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
}

impl Tokens {
    pub fn from_response(r: &TokenResponse, previous_refresh: Option<String>) -> Tokens {
        Tokens {
            access_token: r.access_token.clone(),
            refresh_token: r.refresh_token.clone().or(previous_refresh),
            expires_at: Utc::now() + Duration::seconds(r.expires_in.unwrap_or(3600) as i64),
        }
    }

    pub fn is_fresh(&self) -> bool {
        self.expires_at - Duration::seconds(90) > Utc::now()
    }

    pub fn load(src: Source) -> Option<Tokens> {
        let s = std::fs::read_to_string(token_path(src)).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn save(&self, src: Source) -> Result<()> {
        write_private(&token_path(src), serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn delete(src: Source) {
        let _ = std::fs::remove_file(token_path(src));
    }
}

/// Resposta padrão de um endpoint de token OAuth 2.0.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

/// Erro padrão OAuth 2.0 (`{"error": "...", "error_description": "..."}`).
#[derive(Debug, Deserialize)]
pub struct OAuthError {
    pub error: String,
    pub error_description: Option<String>,
}
