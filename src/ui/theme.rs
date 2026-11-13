//! Paleta inspirada nas cores de sistema do iOS e fontes Inter embutidas.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId};

#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    pub bg: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub primary: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub separator: Color32,
    pub red: Color32,
    pub green: Color32,
    pub orange: Color32,
    pub shadow: Color32,
}

impl Pal {
    pub fn light() -> Pal {
        Pal {
            dark: false,
            bg: Color32::from_rgb(242, 242, 247),
            card: Color32::WHITE,
            card_hover: Color32::from_rgb(250, 250, 252),
            primary: Color32::from_rgb(28, 28, 30),
            secondary: Color32::from_rgb(138, 138, 142),
            tertiary: Color32::from_rgb(199, 199, 204),
            separator: Color32::from_rgb(216, 216, 222),
            red: Color32::from_rgb(255, 59, 48),
            green: Color32::from_rgb(52, 199, 89),
            orange: Color32::from_rgb(255, 149, 0),
            shadow: Color32::from_black_alpha(16),
        }
    }

    pub fn dark() -> Pal {
        Pal {
            dark: true,
            bg: Color32::from_rgb(0, 0, 0),
            card: Color32::from_rgb(28, 28, 30),
            card_hover: Color32::from_rgb(36, 36, 38),
            primary: Color32::from_rgb(255, 255, 255),
            secondary: Color32::from_rgb(152, 152, 159),
            tertiary: Color32::from_rgb(72, 72, 74),
            separator: Color32::from_rgb(44, 44, 46),
            red: Color32::from_rgb(255, 69, 58),
            green: Color32::from_rgb(48, 209, 88),
            orange: Color32::from_rgb(255, 159, 10),
            shadow: Color32::TRANSPARENT,
        }
    }
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Cor com opacidade multiplicada (0..1).
pub fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

/// Mistura `c` sobre `base` (t = 0 → base, 1 → c). Útil para fundos levemente tingidos.
pub fn tint(base: Color32, c: Color32, t: f32) -> Color32 {
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(l(base.r(), c.r()), l(base.g(), c.g()), l(base.b(), c.b()))
}

pub const LIGHT: &str = "light";
pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn font_named(size: f32, name: &'static str) -> FontId {
    FontId::new(size, FontFamily::Name(name.into()))
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();

    let files: [(&str, &'static [u8]); 4] = [
        ("inter", include_bytes!("../../assets/fonts/Inter-Regular.ttf")),
        ("inter-light", include_bytes!("../../assets/fonts/Inter-Light.ttf")),
        ("inter-medium", include_bytes!("../../assets/fonts/Inter-Medium.ttf")),
        ("inter-semibold", include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")),
    ];
    for (name, bytes) in files {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }

    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    for (family, file) in [(LIGHT, "inter-light"), (MEDIUM, "inter-medium"), (SEMIBOLD, "inter-semibold")] {
        let mut list = vec![file.to_string()];
        list.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(fonts);
}
