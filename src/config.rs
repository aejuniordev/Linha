//! Configuração em TOML (`linha config` mostra o caminho).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub geral: Geral,
    pub outlook: OutlookCfg,
    pub google: GoogleCfg,
    pub icloud: ICloudCfg,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Geral {
    /// Quantos dias mostrar a partir de hoje (1 = só hoje).
    pub dias: u32,
    /// Intervalo de sincronização do daemon, em minutos.
    pub intervalo_sync_min: u64,
    /// Fecha a janela sozinha depois de N segundos sem interação (0 = nunca).
    pub fechar_apos_seg: u64,
    /// "sistema", "claro" ou "escuro".
    pub tema: String,
    /// Mantém a janela por cima das outras.
    pub sempre_no_topo: bool,
    /// O daemon abre a linha do tempo assim que inicia (login).
    pub abrir_ao_iniciar: bool,
    /// Segundos de "sumiço" do relógio que contam como o computador ter dormido.
    pub limiar_sono_seg: u64,
    /// Janela sem bordas com cantos arredondados (precisa de compositor no Linux).
    pub cantos_arredondados: bool,
}

impl Default for Geral {
    fn default() -> Self {
        Geral {
            dias: 2,
            intervalo_sync_min: 10,
            fechar_apos_seg: 0,
            tema: "sistema".into(),
            sempre_no_topo: true,
            abrir_ao_iniciar: true,
            limiar_sono_seg: 30,
            cantos_arredondados: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OutlookCfg {
    pub ativo: bool,
    /// Application (client) ID de um app registrado no Microsoft Entra (Azure).
    pub client_id: String,
    /// "common" aceita contas pessoais (outlook.com/hotmail) e corporativas.
    pub tenant: String,
}

impl Default for OutlookCfg {
    fn default() -> Self {
        OutlookCfg { ativo: false, client_id: String::new(), tenant: "common".into() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GoogleCfg {
    pub ativo: bool,
    /// OAuth client do tipo "App para computador" no Google Cloud Console.
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ICloudCfg {
    pub ativo: bool,
    pub apple_id: String,
    /// Senha de app gerada em account.apple.com (não é a senha do Apple ID).
    pub senha_app: String,
}

pub const EXAMPLE: &str = include_str!("../config.example.toml");

impl Config {
    pub fn load() -> Result<Config> {
        let path = store::config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("lendo {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("config inválida em {}", path.display()))
    }

    /// Cria o arquivo de exemplo se ainda não existir. Devolve o caminho.
    pub fn ensure_file() -> Result<std::path::PathBuf> {
        let path = store::config_path();
        if !path.exists() {
            store::write_private(&path, EXAMPLE.as_bytes())?;
        }
        Ok(path)
    }

    pub fn any_enabled(&self) -> bool {
        self.outlook.ativo || self.google.ativo || self.icloud.ativo
    }
}
