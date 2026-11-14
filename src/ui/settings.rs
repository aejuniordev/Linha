//! Tela de Ajustes: contas (login pelo navegador) e preferências.
//! Tudo é gravado sozinho em `config.toml`, na pasta de configuração do usuário.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align, Button, Color32, CornerRadius, Layout, Margin, RichText, Ui};

use super::theme::{fade, font, font_named, Pal, MEDIUM, SEMIBOLD};
use crate::config::{Config, Principal};
use crate::model::{CalendarInfo, Source};
use crate::providers::oauth::Flow;
use crate::{daemon, providers, store, update};

const APPLE_APP_PASSWORDS: &str = "https://account.apple.com/account/manage";

/// Login rodando numa thread.
struct Pending {
    src: Source,
    flow: Arc<Flow>,
    result: Arc<Mutex<Option<Result<String, String>>>>,
}

/// O que a janela precisa fazer depois de desenhar os Ajustes.
#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    /// A config mudou (e já foi gravada).
    pub changed: bool,
    /// Uma conta acabou de ser conectada: vale sincronizar já.
    pub sync: bool,
    /// Versão nova instalada e a pessoa pediu para reiniciar.
    pub restart: bool,
}

pub struct Settings {
    pub cfg: Config,
    saved: Config,
    pending: Option<Pending>,
    /// Última mensagem por conta: (ok, texto).
    notes: HashMap<Source, (bool, String)>,
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

    fn connected(&self, src: Source) -> bool {
        match src {
            Source::Outlook | Source::Google => store::Tokens::load(src).is_some(),
            Source::ICloud => self.cfg.icloud.ativo && !self.cfg.icloud.senha_app.trim().is_empty(),
        }
    }

    fn start_login(&mut self, src: Source, ctx: &egui::Context) {
        self.cancel();
        let flow = Arc::new(Flow::default());
        let result = Arc::new(Mutex::new(None));
        let (cfg, f, r, ctx) = (self.cfg.clone(), flow.clone(), result.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = match src {
                Source::Outlook => providers::outlook::login(&cfg.outlook, &f).map(|_| "Conectado".to_string()),
                Source::Google => providers::google::login(&cfg.google, &f).map(|_| "Conectado".to_string()),
                Source::ICloud => providers::icloud::login(&cfg.icloud).map(|cals| match cals.len() {
                    1 => "Conectado · 1 calendário".to_string(),
                    n => format!("Conectado · {n} calendários"),
                }),
            };
            *r.lock().unwrap() = Some(res.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.notes.remove(&src);
        self.pending = Some(Pending { src, flow, result });
    }

    fn disconnect(&mut self, src: Source) {
        store::Tokens::delete(src);
        match src {
            Source::Outlook => self.cfg.outlook.ativo = false,
            Source::Google => self.cfg.google.ativo = false,
            Source::ICloud => {
                self.cfg.icloud.ativo = false;
                self.cfg.icloud.senha_app.clear();
            }
        }
        self.notes.insert(src, (true, "Desconectado".into()));
    }

    /// Recolhe o resultado de um login que terminou.
    fn poll(&mut self, out: &mut Outcome) {
        let Some(p) = &self.pending else { return };
        let Some(res) = p.result.lock().unwrap().take() else { return };
        let src = p.src;
        self.pending = None;
        match res {
            Ok(msg) => {
                match src {
                    Source::Outlook => self.cfg.outlook.ativo = true,
                    Source::Google => self.cfg.google.ativo = true,
                    Source::ICloud => self.cfg.icloud.ativo = true,
                }
                self.notes.insert(src, (true, msg));
                out.sync = true;
            }
            Err(e) if e.ends_with("login cancelado") => {
                self.notes.remove(&src);
            }
            Err(e) => {
                self.notes.insert(src, (false, e));
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, pal: &Pal, calendars: &[CalendarInfo]) -> Outcome {
        let mut out = Outcome::default();
        self.poll(&mut out);
        let ctx = ui.ctx().clone();

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
            self.account(ui, pal, &ctx, Source::Outlook, "Microsoft 365, Outlook.com e Hotmail");
            self.account(ui, pal, &ctx, Source::Google, "Google Agenda / Gmail");
            self.account(ui, pal, &ctx, Source::ICloud, "Agenda do iPhone, iPad e Mac");

            section(ui, pal, "Calendário principal");
            card(ui, pal, |ui| {
                let writable: Vec<&CalendarInfo> =
                    calendars.iter().filter(|c| c.writable && providers::enabled(&self.cfg, c.source)).collect();
                if writable.is_empty() {
                    hint(ui, pal, "Conecte uma conta e sincronize para escolher onde os eventos novos serão criados.");
                } else {
                    let current = self.cfg.principal.as_ref().map(|p| format!("{} · {}", p.name, p.source.label()));
                    egui::ComboBox::from_id_salt("principal")
                        .selected_text(current.unwrap_or_else(|| "Escolha…".into()))
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for c in &writable {
                                let sel = self.cfg.principal.as_ref().is_some_and(|p| p.source == c.source && p.id == c.id);
                                if ui.selectable_label(sel, format!("{} · {}", c.name, c.source.label())).clicked() {
                                    self.cfg.principal =
                                        Some(Principal { source: c.source, id: c.id.clone(), name: c.name.clone() });
                                }
                            }
                        });
                    hint(ui, pal, "Os eventos criados pelo botão + vão para este calendário.");
                }
                // logins antigos (só leitura) precisam conectar de novo para escrever
                for src in [Source::Outlook, Source::Google] {
                    if self.connected(src) && !providers::can_write(src) {
                        let waiting = self.pending.as_ref().is_some_and(|p| p.src == src);
                        ui.horizontal(|ui| {
                            hint(ui, pal, &format!("{}: falta permissão para criar eventos.", src.label()));
                            if ui.add_enabled(!waiting, Button::new("Reconectar")).clicked() {
                                self.start_login(src, &ctx);
                            }
                        });
                    }
                }
            });

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

    fn account(&mut self, ui: &mut Ui, pal: &Pal, ctx: &egui::Context, src: Source, subtitle: &str) {
        let connected = self.connected(src);
        let waiting = self.pending.as_ref().is_some_and(|p| p.src == src);
        let other_busy = self.pending.is_some() && !waiting;
        let available = match src {
            Source::Outlook => self.cfg.outlook.client_id().is_some(),
            Source::Google => self.cfg.google.credentials().is_some(),
            Source::ICloud => true,
        };

        card(ui, pal, |ui| {
            ui.horizontal(|ui| {
                let dot = match self.notes.get(&src) {
                    Some((false, _)) => pal.red,
                    _ if connected => pal.green,
                    _ => pal.tertiary,
                };
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(src.label()).font(font_named(15.5, MEDIUM)).color(pal.primary));
                    ui.label(RichText::new(subtitle).font(font(12.0)).color(pal.secondary));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if waiting {
                        if ui.button("Cancelar").clicked() {
                            self.cancel();
                        }
                        ui.add(egui::Spinner::new().size(14.0));
                    } else if connected {
                        if ui.add_enabled(!other_busy, Button::new("Desconectar")).clicked() {
                            self.disconnect(src);
                        }
                    } else if src != Source::ICloud {
                        let b = Button::new(RichText::new("Conectar").color(Color32::WHITE)).fill(pal_blue(pal));
                        if ui.add_enabled(available && !other_busy, b).clicked() {
                            self.start_login(src, ctx);
                        }
                    }
                });
            });

            // credenciais do iCloud (a Apple não oferece OAuth para agenda)
            if src == Source::ICloud && !connected {
                ui.add_space(4.0);
                let ic = &mut self.cfg.icloud;
                ui.add_enabled(
                    !waiting,
                    egui::TextEdit::singleline(&mut ic.apple_id).hint_text("Apple ID (voce@icloud.com)").desired_width(f32::INFINITY),
                );
                ui.add_enabled(
                    !waiting,
                    egui::TextEdit::singleline(&mut ic.senha_app)
                        .hint_text("Senha de app (xxxx-xxxx-xxxx-xxxx)")
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                let filled = !ic.apple_id.trim().is_empty() && !ic.senha_app.trim().is_empty();
                let clicked = ui
                    .horizontal(|ui| {
                        let b = Button::new(RichText::new("Conectar").color(Color32::WHITE)).fill(pal_blue(pal));
                        let clicked = ui.add_enabled(filled && !waiting && !other_busy, b).clicked();
                        if ui.link("Criar senha de app ↗").clicked() {
                            let _ = webbrowser::open(APPLE_APP_PASSWORDS);
                        }
                        clicked
                    })
                    .inner;
                if clicked {
                    self.start_login(src, ctx);
                }
                hint(ui, pal, "Em account.apple.com → Entrar e segurança → Senhas de app. A Apple não permite login por navegador para agendas.");
            }

            if waiting && src != Source::ICloud {
                hint(ui, pal, "Confirme o acesso na janela do navegador que abriu…");
                let url = self.pending.as_ref().and_then(|p| p.flow.url());
                if let Some(url) = url {
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
            if !available && src != Source::ICloud && !connected {
                hint(ui, pal, "Esta versão foi compilada sem o app OAuth deste serviço.");
            }

            if connected && !waiting {
                let shown = match src {
                    Source::Outlook => &mut self.cfg.outlook.ativo,
                    Source::Google => &mut self.cfg.google.ativo,
                    Source::ICloud => &mut self.cfg.icloud.ativo,
                };
                if src != Source::ICloud {
                    ui.checkbox(shown, "Mostrar na linha do tempo");
                }
            }
            if let Some((ok, msg)) = self.notes.get(&src) {
                let c = if *ok { pal.secondary } else { pal.red };
                ui.label(RichText::new(msg).font(font(12.0)).color(c));
            }
        });
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
