//! Tela de Ajustes: contas (login pelo navegador), calendário principal, bloqueio
//! de horários e preferências. Tudo é gravado sozinho em `config.toml`, na pasta
//! de configuração do usuário.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align, Button, Color32, CornerRadius, Layout, Margin, RichText, Ui};

use super::theme::{fade, font, font_named, Pal, MEDIUM, SEMIBOLD};
use crate::config::{Account, CalRef, Config, ICloudCfg, Principal};
use crate::model::{Cache, CalendarInfo, Source};
use crate::providers::oauth::Flow;
use crate::store::Tokens;
use crate::{daemon, providers, store, update};

const APPLE_APP_PASSWORDS: &str = "https://account.apple.com/account/manage";

/// Resultado de um login que deu certo.
struct LoginOk {
    /// Outlook/Google: tokens ainda não gravados (o id da conta sai depois).
    tokens: Option<Tokens>,
    /// E-mail da conta (ou Apple ID).
    who: String,
    /// iCloud: (Apple ID, senha de app).
    apple: Option<(String, String)>,
    note: String,
}

/// Login rodando numa thread.
struct Pending {
    src: Source,
    /// Reconectar esta conta (em vez de adicionar uma nova).
    reuse: Option<String>,
    flow: Arc<Flow>,
    result: Arc<Mutex<Option<Result<LoginOk, String>>>>,
}

impl Pending {
    /// Chave das mensagens: a conta, ou "novo:<serviço>" ao adicionar.
    fn key(&self) -> String {
        self.reuse.clone().unwrap_or_else(|| new_key(self.src))
    }
}

fn new_key(src: Source) -> String {
    format!("novo:{}", src.key())
}

/// O que a janela precisa fazer depois de desenhar os Ajustes.
#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    /// A config mudou (e já foi gravada).
    pub changed: bool,
    /// Vale sincronizar já (conta conectada, bloqueio mudou…).
    pub sync: bool,
    /// Versão nova instalada e a pessoa pediu para reiniciar.
    pub restart: bool,
}

pub struct Settings {
    pub cfg: Config,
    saved: Config,
    pending: Option<Pending>,
    /// Última mensagem por conta (ou "novo:<serviço>"): (ok, texto).
    notes: HashMap<String, (bool, String)>,
    /// Formulário "Adicionar iCloud" aberto e o que foi digitado.
    icloud_form: bool,
    apple_id: String,
    senha_app: String,
    error: Option<String>,
    /// Verificação manual de atualização em andamento / último resultado.
    update_job: Option<Arc<Mutex<Option<Result<update::Outcome, String>>>>>,
    update_note: Option<(bool, String)>,
}

impl Settings {
    pub fn new(cfg: &Config) -> Settings {
        Settings {
            cfg: cfg.clone(),
            saved: cfg.clone(),
            pending: None,
            notes: HashMap::new(),
            icloud_form: false,
            apple_id: String::new(),
            senha_app: String::new(),
            error: None,
            update_job: None,
            update_note: None,
        }
    }

    /// Cancela um login em andamento (ao fechar a tela).
    pub fn cancel(&mut self) {
        if let Some(p) = self.pending.take() {
            p.flow.cancel();
        }
    }

    fn available(&self, src: Source) -> bool {
        match src {
            Source::Outlook => self.cfg.outlook.client_id().is_some(),
            Source::Google => self.cfg.google.credentials().is_some(),
            Source::ICloud => true,
        }
    }

    /// Abre o login. `reuse` = reconectar essa conta; `None` = adicionar uma nova.
    fn start_login(&mut self, src: Source, reuse: Option<String>, ctx: &egui::Context) {
        self.cancel();
        let flow = Arc::new(Flow::default());
        let result = Arc::new(Mutex::new(None));
        let (cfg, f, r, ctx) = (self.cfg.clone(), flow.clone(), result.clone(), ctx.clone());
        let apple = (self.apple_id.trim().to_string(), self.senha_app.trim().to_string());
        std::thread::spawn(move || {
            let res = match src {
                Source::Outlook => providers::outlook::login(&cfg.outlook, &f)
                    .map(|(t, who)| LoginOk { tokens: Some(t), who, apple: None, note: "Conectado".into() }),
                Source::Google => providers::google::login(&cfg.google, &f)
                    .map(|(t, who)| LoginOk { tokens: Some(t), who, apple: None, note: "Conectado".into() }),
                Source::ICloud => {
                    let ic = ICloudCfg { ativo: true, apple_id: apple.0.clone(), senha_app: apple.1.clone() };
                    providers::icloud::login(&ic).map(|cals| LoginOk {
                        tokens: None,
                        who: apple.0.clone(),
                        apple: Some(apple.clone()),
                        note: match cals.len() {
                            1 => "Conectado · 1 calendário".into(),
                            n => format!("Conectado · {n} calendários"),
                        },
                    })
                }
            };
            *r.lock().unwrap() = Some(res.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        let p = Pending { src, reuse, flow, result };
        self.notes.remove(&p.key());
        self.pending = Some(p);
    }

    /// Recolhe o resultado de um login que terminou.
    fn poll(&mut self, out: &mut Outcome) {
        let Some(p) = &self.pending else { return };
        let Some(res) = p.result.lock().unwrap().take() else { return };
        let (src, reuse, key) = (p.src, p.reuse.clone(), p.key());
        self.pending = None;
        match res {
            Ok(ok) => {
                let id = self.cfg.upsert_account(src, &ok.who, reuse.as_deref());
                if let Some(t) = &ok.tokens {
                    if let Err(e) = t.save(&id) {
                        self.notes.insert(id, (false, format!("Não consegui gravar o acesso: {e:#}")));
                        return;
                    }
                }
                if let Some((apple_id, senha)) = ok.apple {
                    if let Some(a) = self.cfg.contas.iter_mut().find(|a| a.id == id) {
                        a.apple_id = apple_id;
                        a.senha_app = senha;
                    }
                    self.icloud_form = false;
                    self.apple_id.clear();
                    self.senha_app.clear();
                }
                self.notes.remove(&key);
                self.notes.insert(id, (true, ok.note));
                out.sync = true;
            }
            Err(e) if e.ends_with("login cancelado") => {
                self.notes.remove(&key);
            }
            Err(e) => {
                self.notes.insert(key, (false, e));
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, pal: &Pal, cache: &Cache) -> Outcome {
        let mut out = Outcome::default();
        self.poll(&mut out);
        let ctx = ui.ctx().clone();
        let calendars = &cache.calendars;

        // contas antigas (de antes das várias contas) ganham o e-mail na primeira sincronização
        for a in self.cfg.contas.iter_mut().filter(|a| a.nome.is_empty()) {
            if let Some(who) = cache.status.get(&a.id).and_then(|s| s.who.clone()).filter(|w| !w.is_empty()) {
                a.nome = who;
            }
        }

        // ---- barra do topo ----
        ui.horizontal(|ui| {
            ui.label(RichText::new("Ajustes").font(font_named(22.0, SEMIBOLD)).color(pal.primary));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(Button::new(RichText::new("Concluir").font(font_named(14.5, MEDIUM)))).clicked() {
                    out.close = true;
                }
            });
        });
        ui.add_space(10.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;

            section(ui, pal, "Contas");
            for acct in self.cfg.contas.clone() {
                self.account(ui, pal, &ctx, &acct, cache);
            }
            self.add_account(ui, pal, &ctx);

            section(ui, pal, "Calendário principal");
            card(ui, pal, |ui| self.principal_ui(ui, pal, calendars));

            section(ui, pal, "Bloquear horários");
            card(ui, pal, |ui| self.blocking_ui(ui, pal, calendars));

            section(ui, pal, "Linha do tempo");
            card(ui, pal, |ui| {
                let g = &mut self.cfg.geral;
                row(ui, pal, "Dias exibidos", |ui| {
                    ui.add(egui::Slider::new(&mut g.dias, 1..=14));
                });
                row(ui, pal, "Sincronizar a cada", |ui| {
                    ui.add(egui::Slider::new(&mut g.intervalo_sync_min, 1..=120).suffix(" min"));
                });
                row(ui, pal, "Tema", |ui| {
                    let label = match g.tema.as_str() {
                        "claro" => "Claro",
                        "escuro" => "Escuro",
                        _ => "Automático",
                    };
                    egui::ComboBox::from_id_salt("tema").selected_text(label).show_ui(ui, |ui| {
                        ui.selectable_value(&mut g.tema, "sistema".to_string(), "Automático");
                        ui.selectable_value(&mut g.tema, "claro".to_string(), "Claro");
                        ui.selectable_value(&mut g.tema, "escuro".to_string(), "Escuro");
                    });
                });
            });

            section(ui, pal, "Janela");
            card(ui, pal, |ui| {
                let g = &mut self.cfg.geral;
                ui.checkbox(&mut g.sempre_no_topo, "Manter por cima das outras janelas");
                ui.checkbox(&mut g.cantos_arredondados, "Cantos arredondados (vale ao reabrir)");
                row(ui, pal, "Fechar sozinha após", |ui| {
                    ui.add(egui::Slider::new(&mut g.fechar_apos_seg, 0..=600).suffix(" s"));
                });
                hint(ui, pal, "0 = nunca fecha sozinha.");
            });

            section(ui, pal, "Inicialização");
            card(ui, pal, |ui| {
                let mut on = self.cfg.geral.iniciar_com_sistema;
                if ui.checkbox(&mut on, "Iniciar com o sistema").changed() {
                    match daemon::autostart(on) {
                        Ok(()) => {
                            self.cfg.geral.iniciar_com_sistema = on;
                            if on {
                                daemon::spawn();
                            }
                        }
                        Err(e) => self.error = Some(format!("Não consegui mudar o início automático: {e:#}")),
                    }
                }
                let g = &mut self.cfg.geral;
                ui.add_enabled(g.iniciar_com_sistema, egui::Checkbox::new(&mut g.abrir_ao_iniciar, "Mostrar a linha do tempo ao entrar"));
                hint(ui, pal, "O Linha fica em segundo plano, sincroniza e aparece sozinho quando o computador volta do modo de suspensão.");
                row(ui, pal, "Considerar suspensão após", |ui| {
                    ui.add(egui::Slider::new(&mut g.limiar_sono_seg, 10..=600).suffix(" s"));
                });
            });

            if let Some(e) = &self.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(pal.red).font(font(12.5)));
            }

            section(ui, pal, "Atualizações");
            card(ui, pal, |ui| self.updates(ui, pal, &ctx, &mut out));

            section(ui, pal, "Armazenamento");
            card(ui, pal, |ui| {
                let dir = store::config_dir();
                ui.label(RichText::new(dir.display().to_string()).font(egui::FontId::monospace(11.5)).color(pal.secondary));
                ui.horizontal(|ui| {
                    if ui.button("Abrir pasta").clicked() {
                        store::reveal(&dir);
                    }
                    if ui.button("Abrir pasta de dados").clicked() {
                        store::reveal(&store::data_dir());
                    }
                });
                hint(ui, pal, "Preferências, tokens de acesso e cache ficam só neste computador.");
            });

            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(concat!("Linha ", env!("CARGO_PKG_VERSION"))).font(font(11.5)).color(pal.tertiary));
            });
            ui.add_space(10.0);
        });

        // grava assim que a pessoa solta o mouse (sem gravar a cada quadro de um arrasto)
        let dragging = ctx.input(|i| i.pointer.any_down());
        if self.cfg != self.saved && !dragging {
            match self.cfg.save() {
                Ok(()) => {
                    // contas ligadas/desligadas ou bloqueio mudado: aplica já
                    let toggled = |c: &Config| c.contas.iter().map(|a| (a.id.clone(), a.ativo)).collect::<Vec<_>>();
                    if self.cfg.bloqueio != self.saved.bloqueio
                        || self.cfg.principal != self.saved.principal
                        || toggled(&self.cfg) != toggled(&self.saved)
                    {
                        out.sync = true;
                    }
                    self.saved = self.cfg.clone();
                    out.changed = true;
                }
                Err(e) => self.error = Some(format!("Não consegui gravar os ajustes: {e:#}")),
            }
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        out
    }

    /// Cartão de uma conta conectada.
    fn account(&mut self, ui: &mut Ui, pal: &Pal, ctx: &egui::Context, acct: &Account, cache: &Cache) {
        let src = acct.servico;
        let connected = providers::connected(acct);
        let waiting = self.pending.as_ref().is_some_and(|p| p.reuse.as_deref() == Some(acct.id.as_str()));
        let busy = self.pending.is_some();
        let is_principal = self.cfg.principal.as_ref().is_some_and(|p| p.account == acct.id);
        let status = cache.status.get(&acct.id);

        card(ui, pal, |ui| {
            ui.horizontal(|ui| {
                let dot = match (self.notes.get(&acct.id), status) {
                    (Some((false, _)), _) => pal.red,
                    _ if !connected => pal.tertiary,
                    (_, Some(s)) if !s.ok && acct.ativo => pal.red,
                    _ if acct.ativo => pal.green,
                    _ => pal.tertiary,
                };
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(self.cfg.account_label(&acct.id)).font(font_named(15.0, MEDIUM)).color(pal.primary));
                    let mut sub = src.label().to_string();
                    if is_principal {
                        sub.push_str(" · principal");
                    }
                    ui.label(RichText::new(sub).font(font(12.0)).color(pal.secondary));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if waiting {
                        if ui.button("Cancelar").clicked() {
                            self.cancel();
                        }
                        ui.add(egui::Spinner::new().size(14.0));
                    } else {
                        if ui.add_enabled(!busy, Button::new("Desconectar")).clicked() {
                            self.cfg.remove_account(&acct.id);
                            self.notes.remove(&acct.id);
                        }
                        // OAuth sem tokens (ou sem permissão de escrita): entrar de novo nesta conta
                        let needs_login = src != Source::ICloud && (!connected || !providers::can_write(acct));
                        if needs_login && ui.add_enabled(!busy && self.available(src), Button::new("Reconectar")).clicked() {
                            self.start_login(src, Some(acct.id.clone()), ctx);
                        }
                    }
                });
            });

            if waiting {
                self.browser_hint(ui, pal);
            } else if src != Source::ICloud && !connected {
                hint(ui, pal, "Sessão perdida: clique em Reconectar.");
            } else if src != Source::ICloud && !providers::can_write(acct) {
                hint(ui, pal, "Falta permissão para criar eventos: clique em Reconectar.");
            }

            if let Some(a) = self.cfg.contas.iter_mut().find(|a| a.id == acct.id) {
                ui.checkbox(&mut a.ativo, "Mostrar na linha do tempo");
            }
            let note = self.notes.get(&acct.id).cloned().or_else(|| {
                status.filter(|s| !s.ok && acct.ativo).and_then(|s| s.message.clone()).map(|m| (false, m))
            });
            if let Some((ok, msg)) = note {
                ui.label(RichText::new(msg).font(font(12.0)).color(if ok { pal.secondary } else { pal.red }));
            }
        });
    }

    /// Cartão "Adicionar conta": um botão por serviço (pode repetir o serviço).
    fn add_account(&mut self, ui: &mut Ui, pal: &Pal, ctx: &egui::Context) {
        let adding = self.pending.as_ref().filter(|p| p.reuse.is_none()).map(|p| p.src);
        let busy = self.pending.is_some();
        card(ui, pal, |ui| {
            ui.label(RichText::new("Adicionar conta").font(font_named(15.0, MEDIUM)).color(pal.primary));
            ui.horizontal_wrapped(|ui| {
                for src in [Source::Outlook, Source::Google] {
                    let b = Button::new(RichText::new(format!("+ {}", src.label())).color(Color32::WHITE)).fill(pal_blue(pal));
                    if ui.add_enabled(!busy && self.available(src), b).clicked() {
                        self.start_login(src, None, ctx);
                    }
                }
                let b = Button::new(format!("+ {}", Source::ICloud.label())).selected(self.icloud_form);
                if ui.add_enabled(!busy, b).clicked() {
                    self.icloud_form = !self.icloud_form;
                }
            });

            if let Some(src) = adding {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(format!("Entrando no {}…", src.label())).font(font(12.5)).color(pal.secondary));
                    if ui.button("Cancelar").clicked() {
                        self.cancel();
                    }
                });
                if src != Source::ICloud {
                    self.browser_hint(ui, pal);
                }
            }

            // credenciais do iCloud (a Apple não oferece OAuth para agenda)
            if self.icloud_form {
                let waiting = adding == Some(Source::ICloud);
                ui.add_enabled(
                    !waiting,
                    egui::TextEdit::singleline(&mut self.apple_id).hint_text("Apple ID (voce@icloud.com)").desired_width(f32::INFINITY),
                );
                ui.add_enabled(
                    !waiting,
                    egui::TextEdit::singleline(&mut self.senha_app)
                        .hint_text("Senha de app (xxxx-xxxx-xxxx-xxxx)")
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                let filled = !self.apple_id.trim().is_empty() && !self.senha_app.trim().is_empty();
                let clicked = ui
                    .horizontal(|ui| {
                        let b = Button::new(RichText::new("Conectar").color(Color32::WHITE)).fill(pal_blue(pal));
                        let clicked = ui.add_enabled(filled && !busy, b).clicked();
                        if ui.link("Criar senha de app ↗").clicked() {
                            let _ = webbrowser::open(APPLE_APP_PASSWORDS);
                        }
                        clicked
                    })
                    .inner;
                if clicked {
                    self.start_login(Source::ICloud, None, ctx);
                }
                hint(ui, pal, "Em account.apple.com → Entrar e segurança → Senhas de app. A Apple não permite login por navegador para agendas.");
            }

            for src in Source::ALL {
                if let Some((ok, msg)) = self.notes.get(&new_key(src)) {
                    ui.label(RichText::new(msg).font(font(12.0)).color(if *ok { pal.secondary } else { pal.red }));
                }
            }
            if !self.available(Source::Outlook) || !self.available(Source::Google) {
                hint(ui, pal, "Esta versão foi compilada sem o app OAuth de algum serviço.");
            }
            hint(ui, pal, "Dá para conectar várias contas do mesmo serviço (por exemplo, pessoal e trabalho).");
        });
    }

    /// "Confirme no navegador" + links para abrir de novo / copiar.
    fn browser_hint(&self, ui: &mut Ui, pal: &Pal) {
        hint(ui, pal, "Confirme o acesso na janela do navegador que abriu. Para outra conta, troque de conta por lá.");
        if let Some(url) = self.pending.as_ref().and_then(|p| p.flow.url()) {
            ui.horizontal(|ui| {
                if ui.link("Abrir de novo").clicked() {
                    let _ = webbrowser::open(&url);
                }
                if ui.link("Copiar link").clicked() {
                    ui.ctx().copy_text(url.clone());
                }
            });
        }
    }

    fn principal_ui(&mut self, ui: &mut Ui, pal: &Pal, calendars: &[CalendarInfo]) {
        let writable: Vec<&CalendarInfo> = calendars.iter().filter(|c| c.writable && self.cfg.active(&c.account)).collect();
        if writable.is_empty() {
            hint(ui, pal, "Conecte uma conta e sincronize para escolher onde os eventos novos serão criados.");
            return;
        }
        let label = |cfg: &Config, c: &CalendarInfo| format!("{} · {}", c.name, cfg.account_label(&c.account));
        let current = self
            .cfg
            .principal
            .as_ref()
            .map(|p| format!("{} · {}", p.name, self.cfg.account_label(&p.account)))
            .unwrap_or_else(|| "Escolha…".into());
        let mut pick = None;
        egui::ComboBox::from_id_salt("principal").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
            for c in &writable {
                let sel = self.cfg.principal.as_ref().is_some_and(|p| p.account == c.account && p.id == c.id);
                if ui.selectable_label(sel, label(&self.cfg, c)).clicked() {
                    pick = Some(Principal { account: c.account.clone(), source: c.source, id: c.id.clone(), name: c.name.clone() });
                }
            }
        });
        if let Some(p) = pick {
            // o calendário principal não pode ser origem dos próprios bloqueios
            self.cfg.bloqueio.origens.retain(|o| o.conta != p.account);
            self.cfg.principal = Some(p);
        }
        hint(ui, pal, "Os eventos criados pelo botão + vão para este calendário. Com o bloqueio ligado, é nele que os horários das outras contas são reservados.");
    }

    fn blocking_ui(&mut self, ui: &mut Ui, pal: &Pal, calendars: &[CalendarInfo]) {
        let Some(principal) = self.cfg.principal.clone() else {
            hint(ui, pal, "Escolha um calendário principal para bloquear nele os horários das outras contas.");
            return;
        };
        let mut on = self.cfg.bloqueio.ativo;
        if ui.checkbox(&mut on, "Bloquear horários das outras contas").changed() {
            self.cfg.bloqueio.ativo = on;
            if on && self.cfg.bloqueio.origens.is_empty() {
                // começa com o calendário padrão de cada outra conta
                self.cfg.bloqueio.origens = calendars
                    .iter()
                    .filter(|c| c.primary && c.account != principal.account && self.cfg.active(&c.account))
                    .map(|c| CalRef { conta: c.account.clone(), id: c.id.clone() })
                    .collect();
            }
        }
        hint(
            ui,
            pal,
            &format!(
                "Para cada compromisso dos calendários marcados abaixo, o Linha cria um evento \"Ocupado\" (particular, sem detalhes e sem lembrete) em {} · {} e o mantém em dia: muda se o original mudar e some se ele for apagado. Eventos de dia inteiro e marcados como disponíveis ficam de fora. Os bloqueios vão até {} dias à frente.",
                principal.name,
                self.cfg.account_label(&principal.account),
                crate::blocks::HORIZON_DAYS
            ),
        );
        if !self.cfg.bloqueio.ativo {
            return;
        }
        if let Some(acct) = self.cfg.account(&principal.account) {
            if !providers::can_write(acct) {
                ui.label(
                    RichText::new("A conta principal não tem permissão para criar eventos: reconecte-a acima.").font(font(12.0)).color(pal.red),
                );
            }
        }

        let others: Vec<&Account> =
            self.cfg.contas.iter().filter(|a| a.ativo && a.id != principal.account).collect();
        if others.is_empty() {
            hint(ui, pal, "Conecte outra conta para ter de onde copiar os horários.");
            return;
        }
        let mut toggles: Vec<(CalRef, bool)> = Vec::new();
        for acct in others {
            ui.add_space(2.0);
            ui.label(RichText::new(self.cfg.account_label(&acct.id)).font(font_named(13.0, MEDIUM)).color(pal.primary));
            let cals: Vec<&CalendarInfo> = calendars.iter().filter(|c| c.account == acct.id).collect();
            if cals.is_empty() {
                hint(ui, pal, "Ainda não sincronizada.");
            }
            for c in cals {
                let r = CalRef { conta: c.account.clone(), id: c.id.clone() };
                let mut on = self.cfg.bloqueio.origens.contains(&r);
                let text = if c.primary { format!("{} (padrão)", c.name) } else { c.name.clone() };
                if ui.checkbox(&mut on, text).changed() {
                    toggles.push((r, on));
                }
            }
        }
        for (r, on) in toggles {
            self.cfg.bloqueio.origens.retain(|x| x != &r);
            if on {
                self.cfg.bloqueio.origens.push(r);
            }
        }
        hint(ui, pal, "Marque só calendários seus: agendas de outras pessoas ou feriados bloqueariam horários à toa.");
    }
}

pub(super) fn pal_blue(pal: &Pal) -> Color32 {
    if pal.dark { Color32::from_rgb(10, 132, 255) } else { Color32::from_rgb(0, 122, 255) }
}

pub(super) fn section(ui: &mut Ui, pal: &Pal, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title.to_uppercase()).font(font(11.5)).color(pal.secondary));
}

pub(super) fn card(ui: &mut Ui, pal: &Pal, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(pal.card)
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui);
        });
}

pub(super) fn row(ui: &mut Ui, pal: &Pal, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).font(font(13.5)).color(pal.primary));
        ui.with_layout(Layout::right_to_left(Align::Center), add);
    });
}

pub(super) fn hint(ui: &mut Ui, pal: &Pal, text: &str) {
    ui.label(RichText::new(text).font(font(11.5)).color(fade(pal.secondary, 0.9)));
}

impl Settings {
    fn updates(&mut self, ui: &mut Ui, pal: &Pal, ctx: &egui::Context, out: &mut Outcome) {
        // resultado de uma verificação manual
        if let Some(res) = self.update_job.as_ref().and_then(|j| j.lock().unwrap().take()) {
            self.update_job = None;
            self.update_note = Some(match res {
                Ok(update::Outcome::UpToDate) => (true, format!("Você já está na versão mais recente ({}).", update::CURRENT)),
                Ok(update::Outcome::Installed(v)) => (true, format!("Versão {v} instalada.")),
                Err(e) => (false, format!("Não consegui atualizar: {e}")),
            });
        }

        ui.checkbox(&mut self.cfg.geral.atualizar_auto, "Atualizar automaticamente");
        let busy = self.update_job.is_some();
        ui.horizontal(|ui| {
            let can = update::supported() && !busy && update::pending().is_none();
            if ui.add_enabled(can, Button::new("Procurar atualização")).clicked() {
                let job = Arc::new(Mutex::new(None));
                let (j, ctx) = (job.clone(), ctx.clone());
                std::thread::spawn(move || {
                    *j.lock().unwrap() = Some(update::check_and_install().map_err(|e| format!("{e:#}")));
                    ctx.request_repaint();
                });
                self.update_job = Some(job);
            }
            if busy {
                ui.add(egui::Spinner::new().size(14.0));
            }
            if update::pending().is_some() {
                let b = Button::new(RichText::new("Reiniciar agora").color(Color32::WHITE)).fill(pal_blue(pal));
                if ui.add(b).clicked() {
                    out.restart = true;
                }
            }
        });
        if let Some(v) = update::pending() {
            hint(ui, pal, &format!("A versão {v} já foi baixada e entra em uso quando o Linha reiniciar."));
        } else if let Some((ok, msg)) = &self.update_note {
            ui.label(RichText::new(msg).font(font(12.0)).color(if *ok { pal.secondary } else { pal.red }));
        }
        if !update::supported() {
            hint(ui, pal, "Builds de desenvolvimento não se atualizam.");
        } else {
            hint(ui, pal, &format!("Versão atual: {}. As novas versões vêm das releases do GitHub.", update::CURRENT));
        }
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
