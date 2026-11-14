//! Modo residente (Windows): ícone na área de notificação e janela tipo "flyout",
//! que sobe no canto da tela, acima da barra de tarefas, como o calendário do Windows.
//!
//! A janela fica carregada e escondida; mostrar é instantâneo. Ela some ao perder
//! o foco, com Esc ou no ×. Sincronização periódica e detecção de "despertar"
//! rodam numa thread, já que uma janela escondida não recebe quadros.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;

use crate::config::Config;
use crate::model::Cache;
use crate::sync;

/// Ponte entre a janela, o ícone da bandeja e a thread de fundo.
pub struct Shell {
    ctx: egui::Context,
    /// Pede à janela para tocar a animação de entrada.
    pub replay: Arc<AtomicBool>,
    /// Pede à janela para abrir os Ajustes ao aparecer.
    pub want_settings: Arc<AtomicBool>,
    visible: AtomicBool,
    last_hide: Mutex<Option<Instant>>,
}

impl Shell {
    /// Mostra a janela no canto da tela e toca a animação de entrada.
    pub fn reveal(&self) {
        self.replay.store(true, Ordering::SeqCst);
        self.visible.store(true, Ordering::SeqCst);
        imp::show();
        self.ctx.request_repaint();
    }

    /// Esconde a janela (o processo continua na bandeja).
    pub fn hide(&self) {
        self.visible.store(false, Ordering::SeqCst);
        *self.last_hide.lock().unwrap() = Some(Instant::now());
        imp::hide();
    }

    /// Clique no ícone: alterna. Se a janela acabou de sumir por causa deste mesmo
    /// clique (perdeu o foco para a barra de tarefas), não reabre.
    pub fn toggle(&self) {
        let just_hidden = self.last_hide.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_millis(350));
        if self.visible.load(Ordering::SeqCst) {
            self.hide();
        } else if !just_hidden {
            self.reveal();
        }
    }
}

/// Liga o modo residente. `None` fora do Windows.
pub fn start(
    ctx: &egui::Context,
    replay: Arc<AtomicBool>,
    want_settings: Arc<AtomicBool>,
    incoming: Arc<Mutex<Option<Cache>>>,
    syncing: Arc<AtomicBool>,
) -> Option<Arc<Shell>> {
    if !cfg!(windows) {
        return None;
    }
    let shell = Arc::new(Shell {
        ctx: ctx.clone(),
        replay,
        want_settings,
        visible: AtomicBool::new(true),
        last_hide: Mutex::new(None),
    });
    imp::tray(shell.clone());
    background(shell.clone(), incoming, syncing);
    Some(shell)
}

/// Sincroniza de tempos em tempos e mostra a janela quando o computador desperta.
fn background(shell: Arc<Shell>, incoming: Arc<Mutex<Option<Cache>>>, syncing: Arc<AtomicBool>) {
    const TICK: Duration = Duration::from_secs(5);
    std::thread::spawn(move || {
        let sync_now = |cfg: &Config| {
            if !cfg.any_enabled() || syncing.swap(true, Ordering::SeqCst) {
                return;
            }
            let cache = sync::sync_all(cfg);
            *incoming.lock().unwrap() = Some(cache);
            syncing.store(false, Ordering::SeqCst);
            shell.ctx.request_repaint();
        };
        let mut last_wall = SystemTime::now();
        let mut last_sync = Instant::now();
        loop {
            std::thread::sleep(TICK);
            let cfg = Config::load().unwrap_or_default();
            let now = SystemTime::now();
            let gap = now.duration_since(last_wall).unwrap_or_default();
            last_wall = now;
            if gap > TICK + Duration::from_secs(cfg.geral.limiar_sono_seg.max(10)) {
                // acordou: espera a rede voltar, mostra e sincroniza
                std::thread::sleep(Duration::from_secs(2));
                shell.reveal();
                sync_now(&cfg);
                last_sync = Instant::now();
            } else if last_sync.elapsed() >= Duration::from_secs(cfg.geral.intervalo_sync_min.max(1) * 60) {
                sync_now(&cfg);
                last_sync = Instant::now();
            }
        }
    });
}

/// Ícone 32×32 desenhado em código: folhinha de calendário com faixa vermelha.
#[cfg_attr(not(windows), allow(dead_code))]
fn icon_rgba() -> (Vec<u8>, u32) {
    const N: u32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let (x0, y0, x1, y1, r) = (3.0f32, 4.0f32, 29.0f32, 29.0f32, 5.0f32);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // distância até o retângulo arredondado (para borda suave)
            let dx = (x0 + r - fx).max(fx - (x1 - r)).max(0.0);
            let dy = (y0 + r - fy).max(fy - (y1 - r)).max(0.0);
            let d = (dx * dx + dy * dy).sqrt() - r;
            let inside = fx >= x0 - 1.0 && fx <= x1 + 1.0 && fy >= y0 - 1.0 && fy <= y1 + 1.0;
            let a = if inside { (0.5 - d).clamp(0.0, 1.0) } else { 0.0 };
            if a <= 0.0 {
                continue;
            }
            let rgb: [u8; 3] = if fy < 12.0 {
                [255, 59, 48] // faixa do topo
            } else if (fy - 20.5).abs() < 1.6 && fx > 9.0 && fx < 23.0 {
                [255, 59, 48] // "linha do tempo"
            } else {
                [255, 255, 255]
            };
            let i = ((y * N + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], (a * 255.0) as u8]);
        }
    }
    (px, N)
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;

    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{icon_rgba, Shell};

    thread_local! {
        static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    }

    pub fn tray(shell: Arc<Shell>) {
        let menu = Menu::new();
        let _ = menu.append_items(&[
            &MenuItem::with_id("abrir", "Abrir o Linha", true, None),
            &MenuItem::with_id("ajustes", "Ajustes…", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("sair", "Sair", true, None),
        ]);
        let (rgba, n) = icon_rgba();
        let built = TrayIconBuilder::new()
            .with_tooltip("Linha")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(Icon::from_rgba(rgba, n, n).expect("ícone"))
            .build();
        match built {
            Ok(t) => TRAY.with(|cell| *cell.borrow_mut() = Some(t)),
            Err(e) => eprintln!("não consegui criar o ícone da bandeja: {e}"),
        }

        let s = shell.clone();
        TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                s.toggle();
            }
        }));
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            if ev.id == "abrir" {
                shell.reveal();
            } else if ev.id == "ajustes" {
                shell.want_settings.store(true, std::sync::atomic::Ordering::SeqCst);
                shell.reveal();
            } else if ev.id == "sair" {
                // o menu roda na thread da janela: dá para tirar o ícone antes de sair
                TRAY.with(|cell| cell.borrow_mut().take());
                std::process::exit(0);
            }
        }));
    }

    // ---- Win32 mínimo, sem dependências extras ----

    type Hwnd = isize;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MonitorInfo {
        cb_size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: extern "system" fn(Hwnd, isize) -> i32, lparam: isize) -> i32;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
        fn GetWindowTextW(hwnd: Hwnd, buf: *mut u16, max: i32) -> i32;
        fn ShowWindowAsync(hwnd: Hwnd, cmd: i32) -> i32;
        fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
        fn GetCursorPos(p: *mut Point) -> i32;
        fn MonitorFromPoint(p: Point, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
        fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    }

    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_ASYNCWINDOWPOS: u32 = 0x4000;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    static HWND: AtomicIsize = AtomicIsize::new(0);

    extern "system" fn find(hwnd: Hwnd, _: isize) -> i32 {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != std::process::id() {
            return 1;
        }
        let mut buf = [0u16; 16];
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if String::from_utf16_lossy(&buf[..n.max(0) as usize]) == "Linha" {
            HWND.store(hwnd, Ordering::SeqCst);
            return 0;
        }
        1
    }

    /// A janela principal deste processo (título "Linha").
    fn hwnd() -> Option<Hwnd> {
        let h = HWND.load(Ordering::SeqCst);
        if h != 0 {
            return Some(h);
        }
        unsafe { EnumWindows(find, 0) };
        Some(HWND.load(Ordering::SeqCst)).filter(|h| *h != 0)
    }

    /// Põe a janela no canto da área de trabalho do monitor do mouse, encostada
    /// na barra de tarefas (onde quer que ela esteja).
    fn place(h: Hwnd) {
        unsafe {
            let mut wr = Rect::default();
            let mut cur = Point::default();
            if GetWindowRect(h, &mut wr) == 0 || GetCursorPos(&mut cur) == 0 {
                return;
            }
            let mut mi = MonitorInfo { cb_size: std::mem::size_of::<MonitorInfo>() as u32, ..Default::default() };
            if GetMonitorInfoW(MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
                return;
            }
            let (m, w) = (mi.monitor, mi.work);
            let dpi = GetDpiForWindow(h).max(96) as i32;
            let margin = 12 * dpi / 96;
            let width = wr.right - wr.left;
            let height = (wr.bottom - wr.top).min(w.bottom - w.top - 2 * margin);

            let x = if w.left > m.left { w.left + margin } else { w.right - width - margin };
            let y = if w.top > m.top { w.top + margin } else { w.bottom - height - margin };
            SetWindowPos(h, 0, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
        }
    }

    pub fn show() {
        let Some(h) = hwnd() else { return };
        place(h);
        unsafe {
            ShowWindowAsync(h, SW_SHOW);
            SetForegroundWindow(h);
        }
    }

    pub fn hide() {
        if let Some(h) = hwnd() {
            unsafe { ShowWindowAsync(h, SW_HIDE) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::Arc;

    pub fn tray(_shell: Arc<super::Shell>) {}
    pub fn show() {}
    pub fn hide() {}
}
