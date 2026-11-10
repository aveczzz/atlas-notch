//! Animações com física de mola, ao estilo da Dynamic Island.
//!
//! Porque molas e não curvas de easing: uma mola pode ser re-direcionada a meio
//! (o alvo muda enquanto ainda está a animar) sem saltos, porque mantém a velocidade.
//! É isto que dá a sensação "orgânica".
//!
//! Eficiência: o renderer só pede frames enquanto `is_settled()` for falso.
//! Quando tudo assenta, a app volta a dormir no event loop.

/// Parâmetros de mola. Use os presets em vez de inventar números.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringParams {
    /// Expandir/encolher: rápido, com um ligeiro overshoot.
    pub const BOUNCY: Self = Self { stiffness: 380.0, damping: 26.0 };
    /// Mudanças de conteúdo: sem overshoot.
    pub const SMOOTH: Self = Self { stiffness: 260.0, damping: 32.0 };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub params: SpringParams,
}

impl Spring {
    // Assenta quando o resto do movimento já não se vê: 0,2 DIP é 0,25 px a 125% e
    // 2 DIP/s anda 0,03 DIP num frame. Mais apertado só gera frames invisíveis no fim.
    const EPS_VALUE: f32 = 0.2;
    const EPS_VELOCITY: f32 = 2.0;
    const MAX_SUBSTEP: f32 = 1.0 / 240.0;

    pub fn new(value: f32, params: SpringParams) -> Self {
        Self { value, velocity: 0.0, target: value, params }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Avança `dt` segundos. Devolve `true` se ainda está em movimento.
    ///
    /// Euler semi-implícito com sub-passos: estável mesmo se um frame atrasar
    /// (ex.: a GPU acordou devagar), em vez de explodir.
    pub fn step(&mut self, dt: f32) -> bool {
        let dt = dt.clamp(0.0, 0.1); // nunca simular mais de 100 ms de uma vez
        let steps = (dt / Self::MAX_SUBSTEP).ceil().max(1.0) as u32;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let force = -self.params.stiffness * (self.value - self.target)
                - self.params.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
        }
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }
        true
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < Self::EPS_VALUE && self.velocity.abs() < Self::EPS_VELOCITY
    }
}

/// Geometria animada da island (em DIPs; o renderer converte para pixeis com o DPI).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandShape {
    pub width: Spring,
    pub height: Spring,
    pub radius: Spring,
}

impl IslandShape {
    pub fn new(width: f32, height: f32) -> Self {
        let p = SpringParams::BOUNCY;
        Self {
            width: Spring::new(width, p),
            height: Spring::new(height, p),
            radius: Spring::new(height / 2.0, p),
        }
    }

    pub fn set_target(&mut self, width: f32, height: f32, radius: f32) {
        self.width.set_target(width);
        self.height.set_target(height);
        self.radius.set_target(radius);
    }

    /// Devolve `true` se algum eixo ainda está a animar.
    pub fn step(&mut self, dt: f32) -> bool {
        // `|` e não `||`: todos os eixos têm de avançar.
        self.width.step(dt) | self.height.step(dt) | self.radius.step(dt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_until_settled(s: &mut Spring) -> (u32, f32) {
        let mut frames = 0;
        let mut peak = s.value;
        while s.step(1.0 / 60.0) {
            frames += 1;
            peak = peak.max(s.value);
            assert!(frames < 600, "mola não assentou em 10 s");
        }
        (frames, peak)
    }

    #[test]
    fn settles_exactly_on_target() {
        let mut s = Spring::new(0.0, SpringParams::SMOOTH);
        s.set_target(100.0);
        let (frames, _) = run_until_settled(&mut s);
        assert_eq!(s.value, 100.0);
        assert!(frames < 60, "SMOOTH deve assentar em menos de 1 s, demorou {frames} frames");
    }

    #[test]
    fn bouncy_overshoots_smooth_does_not() {
        let mut b = Spring::new(0.0, SpringParams::BOUNCY);
        b.set_target(100.0);
        assert!(run_until_settled(&mut b).1 > 100.5);

        let mut s = Spring::new(0.0, SpringParams::SMOOTH);
        s.set_target(100.0);
        assert!(run_until_settled(&mut s).1 <= 100.5);
    }

    #[test]
    fn retarget_mid_flight_keeps_velocity() {
        let mut s = Spring::new(0.0, SpringParams::BOUNCY);
        s.set_target(100.0);
        for _ in 0..5 {
            s.step(1.0 / 60.0);
        }
        let v = s.velocity;
        s.set_target(50.0);
        assert_eq!(s.velocity, v, "mudar o alvo não pode causar saltos");
        run_until_settled(&mut s);
        assert_eq!(s.value, 50.0);
    }

    #[test]
    fn huge_dt_is_stable() {
        let mut s = Spring::new(0.0, SpringParams::BOUNCY);
        s.set_target(100.0);
        s.step(5.0); // frame perdido durante segundos
        assert!(s.value.is_finite() && s.value.abs() < 1000.0);
    }

    #[test]
    fn settled_shape_stops_requesting_frames() {
        let mut shape = IslandShape::new(120.0, 36.0);
        assert!(!shape.step(1.0 / 60.0));
        shape.set_target(360.0, 180.0, 32.0);
        assert!(shape.step(1.0 / 60.0));
    }
}
