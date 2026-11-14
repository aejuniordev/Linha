//! Janela da linha do tempo.

mod anim;
mod fmt;
mod compose;
mod settings;
mod shell;
mod theme;
mod timeline;

use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Local, Utc};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, Id, Key, Rect, Sense, Stroke, StrokeKind, Theme,
    ThemePreference, UiBuilder, ViewportCommand,
};

use crate::config::Config;
use crate::daemon::WINDOW_PORT;
use crate::model::{Cache, Source};
use crate::{demo, providers, sync};
use anim::{ease, ease_in, progress};
use theme::{fade, font, font_named, Pal, LIGHT, MEDIUM};

const RADIUS: u8 = 24;
const FOOTER_H: f32 = 40.0;

/// Como a janela vai se comportar.
#[derive(Clone, Copy, Default)]
pub struct Mode {
    /// Agenda de exemplo.
    pub demo: bool,
    /// Abre direto nos Ajustes.
    pub settings: bool,
    /// Windows: fica na área de notificação; `Some(true)` = já aparece ao iniciar.
    pub resident: Option<bool>,
}

pub fn run(cfg: Config, mode: Mode) -> anyhow::Result<()> {
    let resident = mode.resident.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Linha")
        .with_app_id("linha")
        .with_inner_size([400.0, 740.0])
        .with_min_inner_size([340.0, 480.0])
        .with_decorations(false)
        .with_transparent(cfg.geral.cantos_arredondados)
        .with_resizable(!resident)
        .with_taskbar(!resident);
    if cfg.geral.sempre_no_topo || resident {
        viewport = viewport.with_always_on_top();
    }
    let options = eframe::NativeOptions { viewport, centered: !resident, ..Default::default() };
    eframe::run_native("Linha", options, Box::new(move |cc| Ok(Box::new(LinhaApp::new(cc, cfg, mode)))))
        .map_err(|e| anyhow::anyhow!("falha ao abrir a janela: {e}"))
}

pub struct LinhaApp {
    cfg: Config,
    cache: Cache,
    demo: bool,
    incoming: Arc<Mutex<Option<Cache>>>,
    syncing: Arc<AtomicBool>,
    replay: Arc<AtomicBool>,
    opened_at: Option<f64>,
    first_seen: HashMap<String, f64>,
    closing_at: Option<f64>,
    last_input: f64,
    last_sync: f64,
    settings: Option<settings::Settings>,
    want_settings: Arc<AtomicBool>,
    /// Modo residente (bandeja do Windows).
    shell: Option<Arc<shell::Shell>>,
    /// Aparecer já ao iniciar (modo residente).
    show_at_start: bool,
    first_frame: bool,
    had_focus: bool,
    compose: Option<compose::Compose>,
}

impl LinhaApp {
    fn new(cc: &eframe::CreationContext<'_>, cfg: Config, mode: Mode) -> Self {
        let demo = mode.demo;
        theme::install_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_theme(theme_pref(&cfg.geral.tema));

        let replay = Arc::new(AtomicBool::new(false));
        let want_settings = Arc::new(AtomicBool::new(false));
        let incoming: Arc<Mutex<Option<Cache>>> = Arc::default();
        let syncing: Arc<AtomicBool> = Arc::default();
        let shell = match mode.resident {
            Some(_) if !demo => shell::start(&cc.egui_ctx, replay.clone(), want_settings.clone(), incoming.clone(), syncing.clone()),
            _ => None,
        };

        // Escuta pedidos de "aparecer" vindos de outro processo (`linha`, daemon).
        // O primeiro byte diz o que fazer: 's' = abrir nos Ajustes, '-' = nada.
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", WINDOW_PORT)) {
            let (ctx, flag, ws, sh) = (cc.egui_ctx.clone(), replay.clone(), want_settings.clone(), shell.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                    let mut b = [0u8; 1];
                    let _ = std::io::Read::read(&mut stream, &mut b);
                    match b[0] {
                        b'-' => continue, // só conferindo se já estamos rodando
                        b's' => ws.store(true, Ordering::SeqCst),
                        _ => {}
                    }
                    match &sh {
                        Some(sh) => sh.reveal(),
                        None => {
                            flag.store(true, Ordering::SeqCst);
                            ctx.request_repaint();
                        }
                    }
                }
            });
        }

        let panel = mode.settings.then(|| settings::Settings::new(&cfg));
        let mut app = LinhaApp {
            cache: if demo { demo::cache() } else { Cache::load() },
            cfg,
            demo,
            incoming,
            syncing,
            replay,
            want_settings,
            shell,
            show_at_start: mode.resident.unwrap_or(true) || mode.settings,
            first_frame: true,
            had_focus: false,
            compose: None,
            opened_at: None,
            first_seen: HashMap::new(),
            closing_at: None,
            last_input: 0.0,
            last_sync: 0.0,
            settings: panel,
        };
        app.start_sync(&cc.egui_ctx, false, 0.0);
        app
    }

    fn start_sync(&mut self, ctx: &egui::Context, force: bool, now: f64) {
        if self.demo || !self.cfg.any_enabled() {
            return;
        }
        let fresh = self.cache.updated.is_some_and(|u| (Utc::now() - u).num_seconds() < 60);
        if (!force && fresh) || self.syncing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.last_sync = now;
        let cfg = Config::load().unwrap_or_else(|_| self.cfg.clone());
        let (incoming, syncing, ctx) = (self.incoming.clone(), self.syncing.clone(), ctx.clone());
        std::thread::spawn(move || {
            let cache = sync::sync_all(&cfg);
            *incoming.lock().unwrap() = Some(cache);
            syncing.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    fn close(&mut self, now: f64) {
        if self.closing_at.is_none() {
            self.closing_at = Some(now);
        }
    }

    fn open_settings(&mut self) {
        self.compose = None;
        if self.settings.is_none() {
            self.settings = Some(settings::Settings::new(&self.cfg));
        }
    }

    fn close_settings(&mut self) {
        if let Some(mut s) = self.settings.take() {
            s.cancel();
        }
    }

    /// Aplica na hora o que dá para mudar com a janela aberta (tema e "sempre no topo").
    fn apply_window(&self, ctx: &egui::Context) {
        ctx.set_theme(theme_pref(&self.cfg.geral.tema));
        let level = if self.cfg.geral.sempre_no_topo { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(level));
    }

    /// Tela de Ajustes, ocupando a janela toda.
    fn settings_page(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Pal, alpha: f32, now: f64) {
        let ctx = ui.ctx().clone();
        let top = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 64.0));
        if ui.interact(top, Id::unique("arrastar-ajustes"), Sense::click_and_drag()).drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let area = Rect::from_min_max(pos2(rect.left() + 20.0, rect.top() + 22.0), pos2(rect.right() - 20.0, rect.bottom() - 8.0));
        let Some(panel) = self.settings.as_mut() else { return };
        let cals = &self.cache.calendars;
        let out = ui
            .scope_builder(UiBuilder::new().max_rect(area), |ui| {
                ui.multiply_opacity(alpha);
                panel.show(ui, pal, cals)
            })
            .inner;
        if out.changed {
            self.cfg = panel.cfg.clone();
            self.apply_window(&ctx);
        }
        if out.sync {
            self.start_sync(&ctx, true, now);
        }
        if out.close {
            self.close_settings();
        }
    }

    fn open_compose(&mut self) {
        if !self.demo && self.cfg.any_enabled() && self.compose.is_none() {
            self.close_settings();
            self.compose = Some(compose::Compose::new(&self.cfg, Local::now().date_naive()));
        }
    }

    /// Tela "Novo evento", ocupando a janela toda.
    fn compose_page(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Pal, alpha: f32, now: f64) {
        let ctx = ui.ctx().clone();
        let top = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 64.0));
        if ui.interact(top, Id::unique("arrastar-novo"), Sense::click_and_drag()).drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let area = Rect::from_min_max(pos2(rect.left() + 20.0, rect.top() + 22.0), pos2(rect.right() - 20.0, rect.bottom() - 8.0));
        let Some(page) = self.compose.as_mut() else { return };
        let (cfg, cals) = (&self.cfg, &self.cache.calendars);
        let out = ui
            .scope_builder(UiBuilder::new().max_rect(area), |ui| {
                ui.multiply_opacity(alpha);
                page.show(ui, pal, cfg, cals)
            })
            .inner;
        if let Some(p) = out.principal {
            self.cfg.principal = Some(p);
            if let Err(e) = self.cfg.save() {
                eprintln!("não consegui gravar o calendário principal: {e:#}");
            }
        }
        if out.saved {
            self.start_sync(&ctx, true, now);
        }
        if out.close {
            self.compose = None;
        }
    }
}

fn theme_pref(tema: &str) -> ThemePreference {
    match tema {
        "claro" => ThemePreference::Light,
        "escuro" => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

impl eframe::App for LinhaApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let opened = *self.opened_at.get_or_insert(now);

        if std::mem::take(&mut self.first_frame) {
            if let Some(sh) = &self.shell {
                if self.show_at_start {
                    sh.reveal();
                } else {
                    sh.hide();
                }
            }
        }
        if self.want_settings.swap(false, Ordering::SeqCst) && !self.demo {
            self.open_settings();
        }

        if self.replay.swap(false, Ordering::SeqCst) {
            self.opened_at = Some(now);
            self.first_seen.clear();
            self.closing_at = None;
            self.last_input = now;
            self.had_focus = false;
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.start_sync(&ctx, false, now);
        }
        let opened = self.opened_at.unwrap_or(opened);

        if let Some(new) = self.incoming.lock().unwrap().take() {
            let old: HashSet<&str> = self.cache.events.iter().map(|e| e.id.as_str()).collect();
            for e in &new.events {
                if !old.contains(e.id.as_str()) {
                    self.first_seen.insert(e.id.clone(), now);
                }
            }
            self.cache = new;
        }

        let (esc, refresh, new_event, active) = ctx.input(|i| {
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::F5) || (i.modifiers.command && i.key_pressed(Key::R)),
                i.modifiers.command && i.key_pressed(Key::N),
                i.pointer.is_moving() || !i.events.is_empty(),
            )
        });
        if new_event {
            self.open_compose();
        }
        if active {
            self.last_input = now;
        }
        if esc {
            if self.compose.is_some() {
                self.compose = None;
            } else if self.settings.is_some() {
                self.close_settings();
            } else {
                self.close(now);
            }
        }
        if refresh {
            self.start_sync(&ctx, true, now);
        }
        let auto_close = self.cfg.geral.fechar_apos_seg as f64;
        if auto_close > 0.0 && self.settings.is_none() && self.compose.is_none() && now - self.last_input.max(opened) > auto_close {
            self.close(now);
        }
        // no modo residente quem sincroniza de tempos em tempos é a thread de fundo
        if self.shell.is_none() && now - self.last_sync > (self.cfg.geral.intervalo_sync_min.max(1) * 60) as f64 {
            self.start_sync(&ctx, false, now);
        }

        if self.shell.is_some() {
            // Alt+F4 e afins só escondem; sair é pelo menu do ícone
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.close(now);
            }
            // como o calendário do Windows: clicou fora, some
            match ctx.input(|i| i.viewport().focused) {
                Some(true) => self.had_focus = true,
                Some(false) if self.had_focus && self.settings.is_none() && self.compose.is_none() && self.closing_at.is_none() => {
                    self.had_focus = false;
                    self.close(now);
                }
                _ => {}
            }
        }

        // ---- opacidade global: entra com fade e sobe um pouco; sai com fade ----
        let appear = ease(progress(now, opened, 0.0, 0.42));
        let vanish = self.closing_at.map(|t| ease_in(progress(now, t, 0.0, 0.24))).unwrap_or(0.0);
        if self.closing_at.is_some() && vanish >= 1.0 {
            match &self.shell {
                Some(sh) => {
                    sh.hide();
                    self.closing_at = None;
                    self.close_settings();
                    // próximo quadro já nasce transparente, pronto para a próxima entrada
                    self.opened_at = Some(f64::INFINITY);
                }
                None => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
        let alpha = appear * (1.0 - vanish);
        let pal = if ctx.theme() == Theme::Dark { Pal::dark() } else { Pal::light() };

        let slide = if self.shell.is_some() { (1.0 - appear) * 18.0 } else { 0.0 };
        let rect = ui.max_rect().translate(vec2(0.0, slide));
        let radius = if self.cfg.geral.cantos_arredondados { RADIUS } else { 0 };
        // janela opaca não tem como "sumir" no fundo da tela: aí só o conteúdo faz fade
        let bg_alpha = if radius > 0 { alpha } else { 1.0 };
        {
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(radius), fade(pal.bg, bg_alpha));
            if radius > 0 {
                let edge = if pal.dark { Color32::from_white_alpha(28) } else { Color32::from_black_alpha(22) };
                p.rect_stroke(rect, CornerRadius::same(radius), Stroke::new(1.0, fade(edge, alpha)), StrokeKind::Inside);
            }
        }

        if self.compose.is_some() {
            self.compose_page(ui, rect, &pal, alpha, now);
            if self.closing_at.is_some() || (0.0..1.0).contains(&(now - opened)) {
                ctx.request_repaint();
            }
            return;
        }
        if self.settings.is_some() {
            self.settings_page(ui, rect, &pal, alpha, now);
            if self.closing_at.is_some() || (0.0..1.0).contains(&(now - opened)) {
                ctx.request_repaint();
            }
            return;
        }

        let header_bottom = self.header(ui, rect, &pal, alpha, now, opened, vanish);

        // ---- linha do tempo (rolável) ----
        let area = Rect::from_min_max(pos2(rect.left() + 20.0, header_bottom), pos2(rect.right() - 20.0, rect.bottom() - FOOTER_H));
        let mut want_settings = false;
        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(area.width() - 8.0);
                if !self.demo && !self.cfg.any_enabled() {
                    want_settings = onboarding(ui, &pal, alpha * ease(progress(now, opened, 0.2, 0.5)));
                } else {
                    let view = timeline::View {
                        pal,
                        alpha,
                        now,
                        opened,
                        first_seen: &self.first_seen,
                        cache: &self.cache,
                        days: self.cfg.geral.dias.clamp(1, 14),
                    };
                    timeline::show(ui, &view);
                }
            });
        });
        timeline::edge_fade(ui.painter(), Rect::from_min_max(pos2(rect.left() + 1.0, area.top()), pos2(rect.right() - 1.0, area.bottom())), fade(pal.bg, bg_alpha), 16.0);
        if want_settings {
            self.open_settings();
        }

        self.footer(ui, rect, &pal, alpha * ease(progress(now, opened, 0.5, 0.5)), now);

        // ---- quando repintar ----
        let recent = self.first_seen.values().any(|t| now - t < 1.2);
        if (0.0..5.0).contains(&(now - opened)) || self.closing_at.is_some() || recent || self.syncing.load(Ordering::SeqCst) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(1000));
        }
    }
}

impl LinhaApp {
    /// Data, relógio grande e resumo. Devolve o y onde o cabeçalho termina.
    #[allow(clippy::too_many_arguments)]
    fn header(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Pal, alpha: f32, now: f64, opened: f64, vanish: f32) -> f32 {
        let ctx = ui.ctx().clone();
        let header = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 214.0));

        // arrastar a janela pelo cabeçalho
        let drag = ui.interact(header, Id::unique("arrastar"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        // botões discretos (aparecem com o mouse sobre a janela)
        let hovering = ctx.input(|i| i.pointer.hover_pos().is_some_and(|p| rect.contains(p)));
        let show = ctx.animate_bool(Id::unique("botoes"), hovering) * alpha;
        let close_r = Rect::from_center_size(pos2(rect.right() - 26.0, rect.top() + 24.0), vec2(26.0, 26.0));
        let sync_r = close_r.translate(vec2(-32.0, 0.0));
        let gear_r = sync_r.translate(vec2(-32.0, 0.0));
        let close_resp = ui.interact(close_r, Id::unique("fechar"), Sense::click());
        let sync_resp = ui.interact(sync_r, Id::unique("sincronizar"), Sense::click());
        let gear_resp = ui.interact(gear_r, Id::unique("ajustes"), Sense::click()).on_hover_text("Ajustes");
        if close_resp.clicked() {
            self.close(now);
        }
        if sync_resp.clicked() {
            self.start_sync(&ctx, true, now);
        }
        if gear_resp.clicked() && !self.demo {
            self.open_settings();
        }
        let p = ui.painter();
        let mut buttons = vec![(close_r, &close_resp), (sync_r, &sync_resp)];
        if !self.demo {
            buttons.push((gear_r, &gear_resp));
        }
        for (r, resp) in buttons {
            let h = ctx.animate_bool(resp.id, resp.hovered());
            let bg = if pal.dark { Color32::from_white_alpha(22) } else { Color32::from_black_alpha(12) };
            p.circle_filled(r.center(), 13.0, fade(bg, show * (0.6 + 0.4 * h)));
        }
        let ink = fade(pal.secondary, show);
        let c = close_r.center();
        let k = 4.2;
        p.line_segment([c + vec2(-k, -k), c + vec2(k, k)], Stroke::new(1.6, ink));
        p.line_segment([c + vec2(-k, k), c + vec2(k, -k)], Stroke::new(1.6, ink));
        // ícone de sincronizar: arco + seta (gira enquanto sincroniza)
        let spin = if self.syncing.load(Ordering::SeqCst) { (now * 5.0) as f32 } else { 0.0 };
        let sc = sync_r.center();
        let pts: Vec<_> = (0..=24)
            .map(|i| {
                let a = spin + 0.5 + i as f32 / 24.0 * 5.0;
                sc + vec2(a.cos(), a.sin()) * 5.5
            })
            .collect();
        let tip = *pts.last().unwrap();
        let ang = spin + 5.5;
        p.add(egui::Shape::line(pts, Stroke::new(1.6, ink)));
        let dir = vec2(-(ang).sin(), (ang).cos());
        let nrm = vec2(ang.cos(), ang.sin());
        p.add(egui::Shape::convex_polygon(
            vec![tip + dir * 3.2, tip - nrm * 3.0 - dir * 0.6, tip + nrm * 3.0 - dir * 0.6],
            ink,
            Stroke::NONE,
        ));
        // ícone de ajustes: engrenagem
        if !self.demo {
            let gc = gear_r.center();
            p.circle_stroke(gc, 3.6, Stroke::new(1.6, ink));
            for i in 0..8 {
                let a = i as f32 / 8.0 * std::f32::consts::TAU;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([gc + d * 5.0, gc + d * 7.2], Stroke::new(2.0, ink));
            }
        }

        // "+" (novo evento), sempre visível no canto esquerdo
        if !self.demo && self.cfg.any_enabled() {
            let plus_r = Rect::from_center_size(pos2(rect.left() + 26.0, rect.top() + 24.0), vec2(26.0, 26.0));
            let resp = ui.interact(plus_r, Id::unique("novo-evento"), Sense::click()).on_hover_text("Novo evento (Ctrl+N)");
            let h = ctx.animate_bool(resp.id, resp.hovered());
            let blue = if pal.dark { Color32::from_rgb(10, 132, 255) } else { Color32::from_rgb(0, 122, 255) };
            let p = ui.painter();
            p.circle_filled(plus_r.center(), 13.0, fade(blue, alpha * (0.14 + 0.12 * h)));
            let (pc, k) = (plus_r.center(), 5.5);
            p.line_segment([pc + vec2(-k, 0.0), pc + vec2(k, 0.0)], Stroke::new(1.8, fade(blue, alpha)));
            p.line_segment([pc + vec2(0.0, -k), pc + vec2(0.0, k)], Stroke::new(1.8, fade(blue, alpha)));
            if resp.clicked() {
                self.open_compose();
            }
        }
        let p = ui.painter();

        // data
        let local = Local::now();
        let cx = rect.center().x;
        let lift = vanish * -8.0;
        let pd = ease(progress(now, opened, 0.0, 0.55));
        p.text(
            pos2(cx, rect.top() + 56.0 + (1.0 - pd) * 8.0 + lift),
            Align2::CENTER_CENTER,
            fmt::long_date(local.date_naive()),
            font_named(17.0, MEDIUM),
            fade(pal.secondary, alpha * pd),
        );

        // relógio
        let pc = ease(progress(now, opened, 0.06, 0.7));
        let scale = 0.94 + 0.06 * pc;
        p.text(
            pos2(cx, rect.top() + 118.0 + (1.0 - pc) * 14.0 + lift),
            Align2::CENTER_CENTER,
            local.format("%H:%M").to_string(),
            font_named(86.0 * scale, LIGHT),
            fade(pal.primary, alpha * pc),
        );

        // resumo do dia
        let ps = ease(progress(now, opened, 0.18, 0.55));
        let (text, color) = summary(&self.cache, pal, self.demo || self.cfg.any_enabled());
        p.text(
            pos2(cx, rect.top() + 178.0 + (1.0 - ps) * 8.0 + lift),
            Align2::CENTER_CENTER,
            text,
            font(14.5),
            fade(color, alpha * ps),
        );
        header.bottom()
    }

    fn footer(&self, ui: &mut egui::Ui, rect: Rect, pal: &Pal, a: f32, now: f64) {
        let y = rect.bottom() - FOOTER_H / 2.0 - 2.0;
        let syncing = self.syncing.load(Ordering::SeqCst);
        let mut items: Vec<(String, Color32, Option<String>)> = Vec::new();
        for src in Source::ALL {
            if !self.demo && !providers::enabled(&self.cfg, src) {
                continue;
            }
            let st = self.cache.status.get(&src);
            let (color, tip) = match st {
                _ if syncing => (pal.tertiary, Some("sincronizando…".to_string())),
                Some(s) if s.ok => (pal.green, Some(format!("{} evento(s)", s.count))),
                Some(s) => (pal.red, s.message.clone()),
                None => (pal.tertiary, Some("ainda não sincronizado".into())),
            };
            items.push((src.label().to_string(), color, tip));
        }
        let right_text = if self.demo {
            "demonstração".to_string()
        } else if syncing {
            "sincronizando…".to_string()
        } else if let Some(u) = self.cache.updated {
            format!("atualizado {}", fmt::ago((Utc::now() - u).num_minutes()))
        } else if self.cfg.any_enabled() {
            "aguardando".into()
        } else {
            "nenhuma conta".into()
        };

        let p = ui.painter().clone();
        let f = font(12.0);
        let galleys: Vec<_> = items
            .iter()
            .map(|(l, _, _)| p.layout_no_wrap(l.clone(), f.clone(), fade(pal.secondary, a)))
            .collect();
        let rg = p.layout_no_wrap(right_text, f.clone(), fade(pal.secondary, a));
        let gap = 14.0;
        let total: f32 = galleys.iter().map(|g| g.size().x + 11.0).sum::<f32>() + gap * galleys.len() as f32 + rg.size().x;
        let mut x = rect.center().x - total / 2.0;
        for ((label, color, tip), g) in items.iter().zip(galleys) {
            let pulse = if syncing { 0.55 + 0.45 * ((now * 4.0).sin() as f32).abs() } else { 1.0 };
            p.circle_filled(pos2(x + 3.5, y), 3.5, fade(*color, a * pulse));
            let r = Rect::from_min_size(pos2(x, y - 8.0), vec2(g.size().x + 11.0, 16.0));
            p.galley(pos2(x + 11.0, y - g.size().y / 2.0), g.clone(), Color32::PLACEHOLDER);
            if let Some(t) = tip {
                ui.interact(r, Id::unique(("status", label)), Sense::hover()).on_hover_text(t);
            }
            x += g.size().x + 11.0 + gap;
        }
        p.galley(pos2(x, y - rg.size().y / 2.0), rg, Color32::PLACEHOLDER);
    }
}

/// Frase curta embaixo do relógio.
fn summary(cache: &Cache, pal: &Pal, connected: bool) -> (String, Color32) {
    if !connected {
        return ("Conecte uma agenda para começar".into(), pal.secondary);
    }
    let now = Local::now();
    let today: Vec<_> = cache.events_on(now.date_naive()).into_iter().filter(|e| !e.is_all_day()).collect();
    if let Some(e) = today.iter().find(|e| e.local_start() <= now && now < e.local_end()) {
        let left = (e.local_end() - now).num_minutes();
        return (format!("Agora: {} · termina em {}", short(&e.title), fmt::span(left)), pal.primary);
    }
    if let Some(e) = today.iter().find(|e| e.local_start() > now) {
        let mins = (e.local_start() - now).num_minutes();
        let color = if mins <= 15 { pal.orange } else { pal.secondary };
        return (format!("Próximo: {} · em {}", short(&e.title), fmt::span(mins)), color);
    }
    if today.is_empty() {
        ("Dia livre".into(), pal.secondary)
    } else {
        ("Por hoje é só".into(), pal.secondary)
    }
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 26 {
        format!("{}…", s.chars().take(25).collect::<String>().trim_end())
    } else {
        s.to_string()
    }
}

/// Cartão de boas-vindas. Devolve `true` se a pessoa pediu para abrir os Ajustes.
fn onboarding(ui: &mut egui::Ui, pal: &Pal, a: f32) -> bool {
    let mut open = false;
    ui.scope(|ui| {
        ui.multiply_opacity(a);
        egui::Frame::new()
            .fill(pal.card)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("Conecte suas agendas").font(font_named(17.0, theme::SEMIBOLD)).color(pal.primary));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Outlook, Google e iCloud numa linha só. O login é feito no navegador; nada passa por servidores do Linha.")
                        .font(font(13.5))
                        .color(pal.secondary),
                );
                ui.add_space(10.0);
                let blue = if pal.dark { Color32::from_rgb(10, 132, 255) } else { Color32::from_rgb(0, 122, 255) };
                let b = egui::Button::new(egui::RichText::new("Abrir Ajustes").font(font_named(14.5, MEDIUM)).color(Color32::WHITE))
                    .fill(blue)
                    .corner_radius(CornerRadius::same(10));
                open = ui.add(b).clicked();
            });
    });
    open
}
