//! Login OAuth 2.0 pelo navegador: authorization code + PKCE, com o retorno
//! capturado num servidor HTTP temporário em 127.0.0.1 (porta aleatória).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::distr::{Alphanumeric, SampleString};
use sha2::{Digest, Sha256};
use url::Url;

/// Quanto tempo esperamos a pessoa confirmar no navegador.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Estado compartilhado de um login em andamento (a interface lê e pode cancelar).
#[derive(Default)]
pub struct Flow {
    cancel: AtomicBool,
    url: Mutex<Option<String>>,
}

impl Flow {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Link de autorização (para "copiar link" caso o navegador não abra).
    pub fn url(&self) -> Option<String> {
        self.url.lock().unwrap().clone()
    }
}

/// Resultado do passo no navegador: o código e o que é preciso para trocá-lo por tokens.
pub struct Grant {
    pub code: String,
    pub redirect_uri: String,
    pub verifier: String,
}

/// Abre o navegador em `auth_endpoint` e espera o redirecionamento com o código.
///
/// `host` é o nome usado no redirect_uri (`127.0.0.1` ou `localhost`, conforme o
/// que o provedor aceita); o servidor local sempre escuta em 127.0.0.1.
pub fn authorize(flow: &Flow, auth_endpoint: &str, host: &str, params: &[(&str, &str)]) -> Result<Grant> {
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| anyhow!("servidor local: {e}"))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| anyhow!("sem porta local"))?;
    let redirect_uri = format!("http://{host}:{port}");

    let mut rng = rand::rng();
    let verifier = Alphanumeric.sample_string(&mut rng, 64);
    let state = Alphanumeric.sample_string(&mut rng, 24);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let mut all: Vec<(&str, &str)> = vec![
        ("redirect_uri", redirect_uri.as_str()),
        ("response_type", "code"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state.as_str()),
    ];
    all.extend_from_slice(params);
    let auth_url = Url::parse_with_params(auth_endpoint, &all)?;

    *flow.url.lock().unwrap() = Some(auth_url.to_string());
    eprintln!("Abrindo o navegador. Se não abrir, use este link:\n{auth_url}");
    let _ = webbrowser::open(auth_url.as_str());

    let deadline = Instant::now() + TIMEOUT;
    loop {
        if flow.cancelled() {
            bail!("login cancelado");
        }
        if Instant::now() >= deadline {
            bail!("tempo esgotado esperando a confirmação no navegador");
        }
        let Some(req) = server.recv_timeout(Duration::from_millis(400))? else { continue };
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{}", req.url())) else { continue };
        let get = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());
        if get("state").as_deref() != Some(state.as_str()) {
            // favicon, pré-carregamento do navegador etc.
            let _ = req.respond(tiny_http::Response::from_string("").with_status_code(404));
            continue;
        }
        let code = get("code");
        let _ = req.respond(page(code.is_some()));
        return match code {
            Some(code) => Ok(Grant { code, redirect_uri, verifier }),
            None => {
                let err = get("error_description").or_else(|| get("error")).unwrap_or_else(|| "login cancelado".into());
                bail!("{err}")
            }
        };
    }
}

fn page(ok: bool) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let (title, text) = if ok {
        ("Pronto ✓", "Conta conectada ao Linha. Pode fechar esta aba.")
    } else {
        ("Login cancelado", "Nada foi conectado. Pode fechar esta aba e tentar de novo pelo Linha.")
    };
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Linha</title>\
         <body style='font-family:-apple-system,system-ui,sans-serif;display:grid;place-items:center;height:90vh;margin:0'>\
         <div style='text-align:center'><h2 style='font-weight:500'>{title}</h2><p style='color:#888'>{text}</p></div>"
    );
    let header = tiny_http::Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
    tiny_http::Response::from_string(html).with_header(header)
}
