use api::settings::{
    HexColor, SettingsView, TerminalTheme, Theme as ThemeSetting, ThemedColor, UiColors,
};
use gpui::{Global, Rgba};

const TEXT_CONTRAST: f64 = 4.5;
const TONE_STEP: f64 = 0.5;
const TONE_STEPS: usize = 200;
const GAMUT_JND: f64 = 0.02;
const GAMUT_EPSILON: f64 = 0.0001;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scheme {
    Light,
    Dark,
}

impl Scheme {
    pub fn resolve(setting: ThemeSetting, system_dark: bool) -> Scheme {
        match setting {
            ThemeSetting::Light => Scheme::Light,
            ThemeSetting::Dark => Scheme::Dark,
            ThemeSetting::System if system_dark => Scheme::Dark,
            ThemeSetting::System => Scheme::Light,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
    pub a: f64,
}

impl Oklch {
    pub const fn new(l: f64, c: f64, h: f64, a: f64) -> Oklch {
        Oklch { l, c, h, a }
    }

    pub const fn with_alpha(self, a: f64) -> Oklch {
        Oklch { a, ..self }
    }
}

#[allow(dead_code)]
pub struct Terminal {
    pub background: Rgba,
    pub foreground: Rgba,
    pub cursor: Rgba,
    pub cursor_accent: Rgba,
    pub selection_background: Rgba,
    pub selection_foreground: Rgba,
    pub ansi: [Rgba; 16],
}

include!(concat!(env!("OUT_DIR"), "/tokens.rs"));
include!(concat!(env!("OUT_DIR"), "/terminals.rs"));

#[allow(dead_code)]
pub struct Theme {
    pub scheme: Scheme,
    pub colors: Colors,
    pub terminal: &'static Terminal,
}

impl Global for Theme {}

impl Theme {
    pub fn resolve(settings: &SettingsView, system_dark: bool) -> Theme {
        let scheme = Scheme::resolve(settings.theme, system_dark);
        Theme {
            scheme,
            colors: colors(scheme, &settings.colors),
            terminal: terminal(&settings.terminal_theme, scheme),
        }
    }
}

pub fn terminal(saved: &TerminalTheme, scheme: Scheme) -> &'static Terminal {
    let mode = if saved.match_dark {
        Scheme::Dark
    } else {
        scheme
    };
    let (chosen, default) = match mode {
        Scheme::Dark => (&saved.dark, DEFAULT_DARK),
        Scheme::Light => (&saved.light, DEFAULT_LIGHT),
    };
    let find = |name: &str| {
        TERMINAL_THEMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, theme)| theme)
    };
    chosen
        .as_deref()
        .and_then(find)
        .or_else(|| find(default))
        .unwrap_or(&TERMINAL_THEMES[0].1)
}

pub fn colors(scheme: Scheme, overrides: &UiColors) -> Colors {
    let mut seeds = match scheme {
        Scheme::Light => Seeds::LIGHT,
        Scheme::Dark => Seeds::DARK,
    };
    let base = resolve(scheme, &seeds);
    let pick = |themed: &ThemedColor| {
        match scheme {
            Scheme::Light => themed.light.as_ref(),
            Scheme::Dark => themed.dark.as_ref(),
        }
        .and_then(hex_channels)
    };
    let text = Some(TEXT_CONTRAST);
    tone(&mut seeds.warn, pick(&overrides.warn), base.warn_ink, text);
    tone(&mut seeds.run, pick(&overrides.run), base.surface, text);
    tone(&mut seeds.ok, pick(&overrides.ok), base.surface, text);
    tone(&mut seeds.err, pick(&overrides.err), base.err_ink, text);
    tone(&mut seeds.edge, pick(&overrides.edge), base.bg, None);
    tone(&mut seeds.repo_line, pick(&overrides.repo), base.bg, None);
    tone(
        &mut seeds.group_line,
        pick(&overrides.worktree),
        base.bg,
        None,
    );
    tone(
        &mut seeds.notice,
        pick(&overrides.notice),
        base.notice_ink,
        text,
    );
    tone(
        &mut seeds.accent,
        pick(&overrides.accent),
        base.accent_ink,
        text,
    );
    resolve(scheme, &seeds)
}

fn resolve(scheme: Scheme, seeds: &Seeds) -> Colors {
    match scheme {
        Scheme::Light => Colors::light(seeds),
        Scheme::Dark => Colors::dark(seeds),
    }
}

fn hex_channels(color: &HexColor) -> Option<[f64; 3]> {
    let digits = color.as_str().strip_prefix('#')?;
    let channel = |i: usize| {
        u8::from_str_radix(digits.get(i..i + 2)?, 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn tone(seed: &mut Oklch, chosen: Option<[f64; 3]>, against: Rgba, min: Option<f64>) {
    let Some(srgb) = chosen else { return };
    let (c, h) = chroma_hue(srgb);
    let against_luminance = luminance_of(against);
    let other = against_luminance.cbrt() * 100.0;
    let mut at = seed.l * 100.0;
    let away = if at >= other { TONE_STEP } else { -TONE_STEP };
    for _ in 0..TONE_STEPS {
        let Some(min) = min else { break };
        if at <= 0.0 || at >= 100.0 || ratio(luminance(at / 100.0, c, h), against_luminance) >= min
        {
            break;
        }
        at += away;
    }
    *seed = Oklch::new(at / 100.0, c, h.to_degrees(), 1.0);
}

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn gamma(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn oklab(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn chroma_hue(srgb: [f64; 3]) -> (f64, f64) {
    let [_, a, b] = oklab(srgb.map(linear));
    (a.hypot(b), b.atan2(a))
}

fn linear_srgb(l: f64, c: f64, h: f64) -> [f64; 3] {
    let a = c * h.cos();
    let b = c * h.sin();
    let long = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let medium = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let short = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * long - 3.3077115913 * medium + 0.2309699292 * short,
        -1.2684380046 * long + 2.6097574011 * medium - 0.3413193965 * short,
        -0.0041960863 * long - 0.7034186147 * medium + 1.7076147010 * short,
    ]
}

fn luminance(l: f64, c: f64, h: f64) -> f64 {
    let [r, g, b] = linear_srgb(l, c, h).map(|v| v.clamp(0.0, 1.0));
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn luminance_of(color: Rgba) -> f64 {
    0.2126 * linear(f64::from(color.r))
        + 0.7152 * linear(f64::from(color.g))
        + 0.0722 * linear(f64::from(color.b))
}

fn ratio(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    rgb.iter().all(|v| (-1e-6..=1.0 + 1e-6).contains(v))
}

fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (a, b) = (oklab(a), oklab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn clip(rgb: [f64; 3]) -> [f64; 3] {
    rgb.map(|v| v.clamp(0.0, 1.0))
}

fn gamut_mapped(color: Oklch) -> [f64; 3] {
    if color.l >= 1.0 {
        return [1.0; 3];
    }
    if color.l <= 0.0 {
        return [0.0; 3];
    }
    let h = color.h.to_radians();
    let at = |chroma: f64| linear_srgb(color.l, chroma, h);
    let origin = at(color.c);
    if in_gamut(origin) {
        return origin;
    }
    let clipped = clip(origin);
    if delta_e(clipped, origin) < GAMUT_JND {
        return clipped;
    }
    let (mut low, mut high, mut low_inside) = (0.0, color.c, true);
    let mut best = clipped;
    while high - low > GAMUT_EPSILON {
        let chroma = (low + high) / 2.0;
        let current = at(chroma);
        if low_inside && in_gamut(current) {
            low = chroma;
            best = current;
            continue;
        }
        let clipped = clip(current);
        let error = delta_e(clipped, current);
        best = clipped;
        if error < GAMUT_JND {
            if GAMUT_JND - error < GAMUT_EPSILON {
                break;
            }
            low_inside = false;
            low = chroma;
        } else {
            high = chroma;
        }
    }
    best
}

pub fn to_rgba(color: Oklch) -> Rgba {
    let [r, g, b] = clip(gamut_mapped(color)).map(gamma);
    Rgba {
        r: r as f32,
        g: g as f32,
        b: b as f32,
        a: color.a as f32,
    }
}
