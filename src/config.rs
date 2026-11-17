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
    /// App OAuth da Microsoft (vale para todas as contas Outlook).
    pub outlook: OutlookCfg,
    /// App OAuth do Google (vale para todas as contas Google).
    pub google: GoogleCfg,
    /// Só para ler configs antigas (uma conta iCloud); hoje fica em `contas`.
    #[serde(skip_serializing)]
    pub icloud: ICloudCfg,
    /// Contas conectadas (pode haver mais de uma de cada serviço).
    #[serde(rename = "conta", skip_serializing_if = "Vec::is_empty")]
    pub contas: Vec<Account>,
    /// Calendário onde os eventos novos são criados.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<Principal>,
    /// Bloqueio, no calendário principal, dos horários ocupados nas outras contas.
    pub bloqueio: Bloqueio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Principal {
    /// Conta dona do calendário (vazio em configs antigas = a única conta do serviço).
    #[serde(default)]
    pub account: String,
    pub source: Source,
    pub id: String,
    pub name: String,
}

/// Uma conta conectada.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Identificador interno; também nomeia o arquivo de tokens.
    pub id: String,
    pub servico: Source,
    /// E-mail da conta (ou Apple ID), para mostrar.
    #[serde(default)]
    pub nome: String,
    /// Exibida na linha do tempo e sincronizada.
    #[serde(default = "yes")]
    pub ativo: bool,
    /// iCloud: Apple ID e senha de app (a Apple não oferece OAuth para agenda).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub apple_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub senha_app: String,
}

fn yes() -> bool {
    true
}

impl Account {
    /// Credenciais no formato que o conector do iCloud usa.
    pub fn icloud(&self) -> ICloudCfg {
        ICloudCfg { ativo: true, apple_id: self.apple_id.clone(), senha_app: self.senha_app.clone() }
    }
}

/// Referência a um calendário de uma conta.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CalRef {
    pub conta: String,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bloqueio {
    /// Cria eventos "Ocupado" no calendário principal nos horários dos calendários de `origens`.
    pub ativo: bool,
    /// Calendários (de outras contas) cujos horários são bloqueados.
    pub origens: Vec<CalRef>,
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
    /// Só em configs antigas (uma conta por serviço); hoje fica em `contas`.
    #[serde(skip_serializing)]
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
    /// Só em configs antigas (uma conta por serviço); hoje fica em `contas`.
    #[serde(skip_serializing)]
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
        let mut cfg: Config = toml::from_str(&text).with_context(|| format!("config inválida em {}", path.display()))?;
        cfg.migrate();
        Ok(cfg)
    }

    /// Config de antes das várias contas: cada serviço vira uma conta cujo id é o
    /// nome do serviço — assim os tokens já gravados (`tokens/google.json`) continuam valendo.
    fn migrate(&mut self) {
        if self.contas.is_empty() {
            let legacy = [
                (Source::Outlook, self.outlook.ativo, store::Tokens::load(Source::Outlook.key()).is_some()),
                (Source::Google, self.google.ativo, store::Tokens::load(Source::Google.key()).is_some()),
            ];
            for (src, ativo, has_tokens) in legacy {
                if ativo || has_tokens {
                    self.contas.push(Account {
                        id: src.key().into(),
                        servico: src,
                        nome: String::new(),
                        ativo,
                        apple_id: String::new(),
                        senha_app: String::new(),
                    });
                }
            }
            let ic = &self.icloud;
            if !ic.apple_id.trim().is_empty() && !ic.senha_app.trim().is_empty() {
                self.contas.push(Account {
                    id: Source::ICloud.key().into(),
                    servico: Source::ICloud,
                    nome: ic.apple_id.trim().into(),
                    ativo: ic.ativo,
                    apple_id: ic.apple_id.trim().into(),
                    senha_app: ic.senha_app.trim().into(),
                });
            }
        }
        self.outlook.ativo = false;
        self.google.ativo = false;
        self.icloud = ICloudCfg::default();
        if let Some(p) = self.principal.as_mut().filter(|p| p.account.is_empty()) {
            p.account = p.source.key().into();
        }
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.contas.iter().find(|a| a.id == id)
    }

    /// A conta existe e está ligada?
    pub fn active(&self, id: &str) -> bool {
        self.account(id).is_some_and(|a| a.ativo)
    }

    pub fn active_accounts(&self) -> impl Iterator<Item = &Account> {
        self.contas.iter().filter(|a| a.ativo)
    }

    /// Nome curto para mostrar: o e-mail, senão o serviço (numerado se houver vários).
    pub fn account_label(&self, id: &str) -> String {
        let Some(a) = self.account(id) else { return id.to_string() };
        if !a.nome.trim().is_empty() {
            return a.nome.trim().to_string();
        }
        let same: Vec<&Account> = self.contas.iter().filter(|x| x.servico == a.servico).collect();
        match same.iter().position(|x| x.id == a.id) {
            Some(i) if same.len() > 1 => format!("{} {}", a.servico.label(), i + 1),
            _ => a.servico.label().to_string(),
        }
    }

    /// Nome bem curto (rodapé): serviço, ou a parte do e-mail antes do @ quando há
    /// mais de uma conta do mesmo serviço.
    pub fn account_short(&self, id: &str) -> String {
        let Some(a) = self.account(id) else { return id.to_string() };
        let many = self.contas.iter().filter(|x| x.servico == a.servico).count() > 1;
        if !many {
            return a.servico.label().to_string();
        }
        match a.nome.split_once('@') {
            Some((user, _)) if !user.is_empty() => user.to_string(),
            _ => self.account_label(id),
        }
    }

    /// Adiciona a conta (ou reaproveita a do mesmo serviço e e-mail) e devolve o id.
    /// `reuse` força uma conta existente (reconectar).
    pub fn upsert_account(&mut self, src: Source, who: &str, reuse: Option<&str>) -> String {
        let who = who.trim();
        let found = reuse.and_then(|id| self.contas.iter().position(|a| a.id == id)).or_else(|| {
            (!who.is_empty())
                .then(|| self.contas.iter().position(|a| a.servico == src && a.nome.eq_ignore_ascii_case(who)))
                .flatten()
        });
        match found {
            Some(i) => {
                let a = &mut self.contas[i];
                a.ativo = true;
                if !who.is_empty() && !a.nome.eq_ignore_ascii_case(who) {
                    a.nome = who.to_string();
                }
                a.id.clone()
            }
            None => {
                use rand::distr::{Alphanumeric, SampleString};
                let id = format!("{}-{}", src.key(), Alphanumeric.sample_string(&mut rand::rng(), 6).to_ascii_lowercase());
                self.contas.push(Account {
                    id: id.clone(),
                    servico: src,
                    nome: who.to_string(),
                    ativo: true,
                    apple_id: String::new(),
                    senha_app: String::new(),
                });
                id
            }
        }
    }

    /// Tira a conta e tudo que aponta para ela (tokens, principal, bloqueio).
    pub fn remove_account(&mut self, id: &str) {
        store::Tokens::delete(id);
        self.contas.retain(|a| a.id != id);
        if self.principal.as_ref().is_some_and(|p| p.account == id) {
            self.principal = None;
        }
        self.bloqueio.origens.retain(|c| c.conta != id);
    }

    pub fn save(&self) -> Result<()> {
        let text = format!(
            "# Linha — gerado pela tela de Ajustes. Prefira mudar por lá.\n\n{}",
            toml::to_string_pretty(self)?
        );
        store::write_private(&store::config_path(), text.as_bytes())
    }

    pub fn any_enabled(&self) -> bool {
        self.contas.iter().any(|a| a.ativo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = r#"
[outlook]
ativo = true
tenant = "common"

[icloud]
ativo = true
apple_id = "eu@icloud.com"
senha_app = "abcd-efgh-ijkl-mnop"

[principal]
source = "Outlook"
id = "AAMk123"
name = "Calendário"
"#;

    #[test]
    fn migra_config_de_uma_conta_por_servico() {
        let mut cfg: Config = toml::from_str(OLD).unwrap();
        cfg.migrate();
        let outlook = cfg.account("outlook").expect("conta Outlook migrada");
        assert!(outlook.ativo && outlook.servico == Source::Outlook);
        let icloud = cfg.account("icloud").expect("conta iCloud migrada");
        assert_eq!((icloud.apple_id.as_str(), icloud.senha_app.as_str()), ("eu@icloud.com", "abcd-efgh-ijkl-mnop"));
        assert_eq!(cfg.principal.as_ref().unwrap().account, "outlook");

        // o formato antigo não volta ao gravar, e o novo relê igual
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(!text.contains("[icloud]") && !text.contains("ativo = true\ntenant"));
        let mut again: Config = toml::from_str(&text).unwrap();
        again.migrate();
        assert_eq!(again.contas, cfg.contas);
        assert_eq!(again.principal, cfg.principal);
    }

    #[test]
    fn varias_contas_do_mesmo_servico() {
        let mut cfg = Config::default();
        let a = cfg.upsert_account(Source::Google, "pessoal@gmail.com", None);
        let b = cfg.upsert_account(Source::Google, "trabalho@empresa.com", None);
        assert_ne!(a, b);
        // entrar de novo na mesma conta reaproveita o id
        assert_eq!(cfg.upsert_account(Source::Google, "PESSOAL@gmail.com", None), a);
        assert_eq!(cfg.contas.len(), 2);
        assert_eq!(cfg.account_short(&a), "pessoal");
        cfg.bloqueio.origens.push(CalRef { conta: b.clone(), id: "x".into() });
        cfg.remove_account(&b);
        assert!(cfg.bloqueio.origens.is_empty() && cfg.contas.len() == 1);
    }
}
