use std::sync::atomic::{AtomicU32, Ordering};

use api::settings::FloatSize;
use gpui::Pixels;

static STEP: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());

#[allow(clippy::disallowed_methods)]
pub fn fixed(value: f32) -> Pixels {
    gpui::px(value)
}

pub fn zoom(size: FloatSize) -> f32 {
    match size {
        FloatSize::Small => 0.85,
        FloatSize::Normal => 1.0,
        FloatSize::Large => 1.15,
        FloatSize::Huge => 1.3,
    }
}

pub fn set(size: FloatSize) {
    STEP.store(zoom(size).to_bits(), Ordering::Relaxed);
}

pub fn get() -> f32 {
    f32::from_bits(STEP.load(Ordering::Relaxed))
}

pub fn px(value: f32) -> Pixels {
    fixed(value * get())
}
