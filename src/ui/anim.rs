//! Curvas de animação.

/// Ritmo geral das animações (1.0 = durações originais; menor = mais rápido).
pub const PACE: f64 = 0.55;

/// Progresso 0..1 de uma animação que começa em `start + delay` e dura `dur` segundos
/// (ambos escalados por [`PACE`]).
pub fn progress(now: f64, start: f64, delay: f64, dur: f64) -> f32 {
    (((now - start - delay * PACE) / (dur * PACE)).clamp(0.0, 1.0)) as f32
}

/// Desaceleração suave (parecida com a curva padrão do iOS).
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Aceleração suave para saídas.
pub fn ease_in(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}
