//! Preferências do Linha. Editadas pela tela de Ajustes e gravadas em
//! `config.toml`, na pasta de configuração do usuário (veja `store::config_dir`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Source;
use crate::store;

// IDs dos apps OAuth embutidos no executável na hora do build (veja docs/OAUTH.md).
// Ninguém precisa registrar app próprio: basta clicar em "Conectar" nos Ajustes.
const OUTLOOK_CLIENT_ID: Option<&str> = option_env!("LINHA_OUTLOOK_CLIENT_ID");
const GOOGLE_CLIENT_ID: Option<&str> = option_env!("LINHA_GOOGLE_CLIENT_ID");
const GOOGLE_CLIENT_SECRET: Option<&str> = option_env!("LINHA_GOOGLE_CLIENT_SECRET");

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub geral: Geral,
    pub outlook: OutlookCfg,
    pub google: GoogleCfg,
    pub icloud: ICloudCfg,
    /// Calendário onde os eventos novos são criados.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<Principal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Principal {
    pub source: Source,
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Geral {
    /// Quantos dias mostrar a partir de hoje (1 = só hoje).
    pub dias: u32,
    /// Intervalo de sincronização, em minutos.
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
    /// Registra o daemon para iniciar junto com o sistema.
    pub iniciar_com_sistema: bool,
    /// Procura novas versões no GitHub e se atualiza sozinho.
    pub atualizar_auto: bool,
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
            iniciar_com_sistema: false,
            atualizar_auto: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutlookCfg {
    /// Conta conectada e exibida na linha do tempo.
    pub ativo: bool,
    /// Substitui o client ID embutido (só para desenvolvimento).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    /// "common" aceita contas pessoais (outlook.com/hotmail) e corporativas.
    pub tenant: String,
}

impl Default for OutlookCfg {
    fn default() -> Self {
        OutlookCfg { ativo: false, client_id: String::new(), tenant: "common".into() }
    }
}

impl OutlookCfg {
    pub fn client_id(&self) -> Option<String> {
        pick(&self.client_id, OUTLOOK_CLIENT_ID)
    }
}


#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GoogleCfg {
    /// Conta conectada e exibida na linha do tempo.
    pub ativo: bool,
    /// Substituem o app OAuth embutido (só para desenvolvimento).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
}

impl GoogleCfg {
    /// (client_id, client_secret). Em apps de desktop o "secret" do Google não é
    /// segredo de verdade — a segurança vem do PKCE e do redirect em loopback.
    pub fn credentials(&self) -> Option<(String, String)> {
        if !self.client_id.trim().is_empty() {
            return Some((self.client_id.trim().to_string(), self.client_secret.trim().to_string()));
        }
        Some((pick("", GOOGLE_CLIENT_ID)?, pick("", GOOGLE_CLIENT_SECRET).unwrap_or_default()))
    }
}


#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ICloudCfg {
    /// Conta conectada e exibida na linha do tempo.
    pub ativo: bool,
    pub apple_id: String,
    /// Senha de app gerada em account.apple.com (não é a senha do Apple ID).
    pub senha_app: String,
}

/// Valor da config se preenchido; senão o embutido no build (se houver).
fn pick(own: &str, builtin: Option<&'static str>) -> Option<String> {
    let own = own.trim();
    if !own.is_empty() {
        return Some(own.to_string());
    }
    builtin.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = store::config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("lendo {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("config inválida em {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let text = format!(
            "# Linha — gerado pela tela de Ajustes. Prefira mudar por lá.\n\n{}",
            toml::to_string_pretty(self)?
        );
        store::write_private(&store::config_path(), text.as_bytes())
    }

    pub fn any_enabled(&self) -> bool {
        self.outlook.ativo || self.google.ativo || self.icloud.ativo
    }
}
