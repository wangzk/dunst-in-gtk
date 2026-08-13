//! Dunst-compatible configuration parsing (`dunstrc`, INI-like).
//!
//! Supports the new-style keys (`width`/`height`/`origin`/`offset`) used by
//! dunst 1.12+ (the format the user's real config uses) as well as the legacy
//! `geometry = WxH+X+Y` / `offset = NxN` syntax from dunst <= 1.11.
//!
//! Only the L0+L1 subset is implemented; recognized-but-unimplemented keys
//! (L2 features like `stack_duplicates`, `per_monitor_dpi`) parse into the
//! config with a warning so real dunstrc files load cleanly.

use std::fmt;
use std::path::Path;

pub const DEFAULT_PATH: &str = "dunst/dunstrc";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    TopLeft,
    TopCenter,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Markup {
    Full,
    Strip,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlignment {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ellipsize {
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPosition {
    Left,
    Right,
    Top,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    None,
    Mouse,
    Keyboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    None,
    CloseCurrent,
    CloseAll,
    DoAction,
    Context,
}

/// Size of a notification dimension: constant, (min, max) range, or percent
/// of the monitor. 0 / (0, 0) means "natural size" (content-driven).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeSpec {
    Constant(i32),
    Range(i32, i32),
    Percent(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// CSS `rgba(r, g, b, a)` string, for the style sheet.
    pub fn css_rgba(&self) -> String {
        format!("rgba({}, {}, {}, {:.3})", self.r, self.g, self.b, self.a as f32 / 255.0)
    }

    pub fn with_alpha(&self, alpha: u8) -> Self {
        Self { a: alpha, ..*self }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Monitor {
    /// Number of the monitor, starting from 0.
    Number(i32),
    /// Monitor name, e.g. "HDMI-1" (matched against connector/description).
    Name(String),
}

impl Monitor {
    #[allow(dead_code)] // reserved for follow-mode refinements
    pub fn is_number(&self, n: i32) -> bool {
        matches!(self, Monitor::Number(m) if *m == n)
    }

    /// Does this monitor spec match the given connector/description string?
    #[allow(dead_code)] // daemon matches by connector directly
    pub fn matches_name(&self, name: &str) -> bool {
        matches!(self, Monitor::Name(n) if name.contains(n.as_str()))
    }
}

#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub width: SizeSpec,
    pub height: SizeSpec,
    pub origin: Origin,
    /// (horizontal, vertical) margin from the origin edges, in pixels.
    pub offset: (i32, i32),
    pub gap_size: i32,
    pub corner_radius: i32,
    pub frame_width: i32,
    /// Default frame color; urgency sections may override it.
    pub frame_color: Color,
    pub font: String,
    pub markup: Markup,
    pub word_wrap: bool,
    pub ellipsize: Ellipsize,
    pub alignment: Alignment,
    pub vertical_alignment: VerticalAlignment,
    pub icons: bool,
    pub icon_position: IconPosition,
    pub min_icon_size: i32,
    pub max_icon_size: i32,
    pub padding: i32,
    pub horizontal_padding: i32,
    pub text_icon_padding: i32,
    pub progress_bar: bool,
    pub progress_bar_height: i32,
    pub progress_bar_frame_width: i32,
    pub progress_bar_min_width: i32,
    pub progress_bar_max_width: i32,
    pub history_length: usize,
    pub transparency: u8, // 0 = opaque .. 100 = invisible
    pub monitor: Monitor,
    pub follow: Follow,
    pub notification_limit: usize, // 0 = unlimited
    pub sort: bool,
    pub mouse_left_click: Vec<MouseAction>,
    pub mouse_middle_click: Vec<MouseAction>,
    pub mouse_right_click: Vec<MouseAction>,
    // Recognized but not yet implemented (L2); kept so real configs parse.
    pub stack_duplicates: bool,
    pub hide_duplicate_count: bool,
    pub ignore_newline: bool,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            width: SizeSpec::Constant(0),
            height: SizeSpec::Constant(0),
            origin: Origin::TopRight,
            offset: (10, 10),
            gap_size: 0,
            corner_radius: 0,
            frame_width: 0,
            frame_color: Color::rgb(0xaa, 0xaa, 0xaa),
            font: "Monospace 8".to_string(),
            markup: Markup::Full,
            word_wrap: false,
            ellipsize: Ellipsize::Middle,
            alignment: Alignment::Left,
            vertical_alignment: VerticalAlignment::Top,
            icons: true,
            icon_position: IconPosition::Left,
            min_icon_size: 32,
            max_icon_size: 64,
            padding: 8,
            horizontal_padding: 8,
            text_icon_padding: 0,
            progress_bar: true,
            progress_bar_height: 10,
            progress_bar_frame_width: 1,
            progress_bar_min_width: 150,
            progress_bar_max_width: 300,
            history_length: 20,
            transparency: 0,
            monitor: Monitor::Number(0),
            follow: Follow::None,
            notification_limit: 0,
            sort: true,
            mouse_left_click: vec![MouseAction::CloseCurrent],
            mouse_middle_click: vec![MouseAction::DoAction, MouseAction::CloseCurrent],
            mouse_right_click: vec![MouseAction::CloseAll],
            stack_duplicates: false,
            hide_duplicate_count: false,
            ignore_newline: false,
        }
    }
}

/// Per-urgency overrides. The urgency sections only allow these keys
/// (background, foreground, highlight, timeout, frame_color, icon).
#[derive(Debug, Clone)]
pub struct UrgencyConfig {
    /// Timeout in seconds; 0 = never expires.
    pub timeout: i32,
    pub background: Color,
    pub foreground: Color,
    pub frame_color: Color,
    pub highlight: Color,
}

impl Default for UrgencyConfig {
    fn default() -> Self {
        Self {
            timeout: 10,
            background: Color::rgb(0xcc, 0xcc, 0xcc),
            foreground: Color::rgb(0x00, 0x00, 0x00),
            frame_color: Color::rgb(0xaa, 0xaa, 0xaa),
            highlight: Color::rgb(0x2e, 0xcc, 0x71),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub global: GlobalConfig,
    /// [low, normal, critical]
    pub urgency: [UrgencyConfig; 3],
}

impl Default for Config {
    fn default() -> Self {
        let mut cfg = Self {
            global: GlobalConfig::default(),
            urgency: [
                UrgencyConfig::default(),
                UrgencyConfig::default(),
                UrgencyConfig::default(),
            ],
        };
        // dunst defaults: low/normal 10s, critical never expires.
        cfg.urgency[2].timeout = 0;
        cfg
    }
}

impl Config {
    pub fn urgency(&self, level: u8) -> &UrgencyConfig {
        &self.urgency[(level as usize).min(2)]
    }

    /// Load from `-config <path>` if given, else from the default dunst
    /// locations. Missing file is not an error (defaults are used, with a
    /// warning). Returns the config and any parse warnings.
    pub fn load(cli_config: Option<&str>) -> (Self, Vec<String>) {
        let path = cli_config
            .map(Path::new)
            .map(Path::to_path_buf)
            .or_else(default_config_path);
        match path {
            Some(p) if p.is_file() => match std::fs::read_to_string(&p) {
                Ok(content) => {
                    let (cfg, mut warnings) = parse(&content);
                    warnings.insert(0, format!("loaded config from {}", p.display()));
                    (cfg, warnings)
                }
                Err(e) => {
                    let cfg = Self::default();
                    (
                        cfg,
                        vec![format!("cannot read {}: {e}; using defaults", p.display())],
                    )
                }
            },
            _ => {
                let cfg = Self::default();
                let msg = match path {
                    Some(p) => format!("config {} not found; using defaults", p.display()),
                    None => "no config file found; using defaults".to_string(),
                };
                (cfg, vec![msg])
            }
        }
    }
}

fn default_config_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join(DEFAULT_PATH))
}

// ------------------------------------------------------------------ parsing

/// Parse dunstrc content into a Config, collecting warnings for anything
/// unknown or malformed. Never fails: malformed values fall back to defaults.
pub fn parse(content: &str) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warnings = Vec::new();

    let mut section: &str = "";
    let mut owned_section: String;
    for (lineno, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            owned_section = name.trim().to_lowercase();
            section = &owned_section;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("line {}: not a key=value pair: {line:?}", lineno + 1));
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = unquote(value.trim());
        if let Err(w) = apply(&mut cfg, section, &key, &value) {
            warnings.push(format!("line {} [{}] {}: {w}", lineno + 1, section, key));
        }
    }

    // dunst semantics: the global frame_color is the default; urgency
    // sections override it. (An urgency explicitly set to the default color
    // is indistinguishable and would inherit — acceptable edge case.)
    let default_frame = UrgencyConfig::default().frame_color;
    for u in &mut cfg.urgency {
        if u.frame_color == default_frame {
            u.frame_color = cfg.global.frame_color;
        }
    }

    (cfg, warnings)
}

/// Strip one layer of matching surrounding quotes (dunst requires quotes for
/// values containing `#`).
fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 {
        let b = v.as_bytes();
        if (b[0] == b'"' && b[v.len() - 1] == b'"') || (b[0] == b'\'' && b[v.len() - 1] == b'\'') {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

fn apply(cfg: &mut Config, section: &str, key: &str, value: &str) -> Result<(), String> {
    match section {
        "" => Err(format!("key {key:?} outside any section")),
        "global" => apply_global(&mut cfg.global, key, value),
        "urgency_low" => apply_urgency(&mut cfg.urgency[0], key, value),
        "urgency_normal" => apply_urgency(&mut cfg.urgency[1], key, value),
        "urgency_critical" => apply_urgency(&mut cfg.urgency[2], key, value),
        other => {
            if matches!(other, "shortcuts" | "experimental" | "frame" | "rules") {
                Err(format!("section [{other}] not implemented"))
            } else {
                Err(format!("unknown section [{other}]"))
            }
        }
    }
}

fn apply_global(g: &mut GlobalConfig, key: &str, value: &str) -> Result<(), String> {
    match key {
        "width" => g.width = parse_size(value)?,
        "height" => g.height = parse_size(value)?,
        "origin" => g.origin = parse_origin(value)?,
        "offset" => g.offset = parse_offset(value)?,
        "geometry" => {
            // Legacy: WxH+X+Y (with signs). Sets width/height/origin/offset.
            let (w, h, x, y) = parse_geometry(value)?;
            g.width = SizeSpec::Constant(w);
            g.height = SizeSpec::Constant(h);
            g.origin = origin_from_signs(x, y);
            g.offset = (x.abs(), y.abs());
        }
        "gap_size" => g.gap_size = parse_int(value)?,
        "corner_radius" => g.corner_radius = parse_int(value)?,
        "frame_width" => g.frame_width = parse_int(value)?,
        "frame_color" => g.frame_color = parse_color(value)?,
        "font" => g.font = value.to_string(),
        "markup" => g.markup = parse_markup(value)?,
        "word_wrap" => g.word_wrap = parse_bool(value)?,
        "ellipsize" => g.ellipsize = parse_ellipsize(value)?,
        "alignment" => g.alignment = parse_alignment(value)?,
        "vertical_alignment" => g.vertical_alignment = parse_vertical_alignment(value)?,
        "icons" => g.icons = parse_bool(value)?,
        "icon_position" => g.icon_position = parse_icon_position(value)?,
        "min_icon_size" => g.min_icon_size = parse_int(value)?,
        "max_icon_size" => g.max_icon_size = parse_int(value)?,
        "padding" => g.padding = parse_int(value)?,
        "horizontal_padding" => g.horizontal_padding = parse_int(value)?,
        "text_icon_padding" => g.text_icon_padding = parse_int(value)?,
        "progress_bar" => g.progress_bar = parse_bool(value)?,
        "progress_bar_height" => g.progress_bar_height = parse_int(value)?,
        "progress_bar_frame_width" => g.progress_bar_frame_width = parse_int(value)?,
        "progress_bar_min_width" => g.progress_bar_min_width = parse_int(value)?,
        "progress_bar_max_width" => g.progress_bar_max_width = parse_int(value)?,
        "history_length" => {
            let n = parse_int(value)?;
            g.history_length = n.max(0) as usize;
        }
        "transparency" => {
            let n = parse_int(value)?;
            g.transparency = n.clamp(0, 100) as u8;
        }
        "monitor" => {
            g.monitor = if value.chars().all(|c| c.is_ascii_digit()) {
                Monitor::Number(parse_int(value)?)
            } else {
                Monitor::Name(value.to_string())
            };
        }
        "follow" => g.follow = parse_follow(value)?,
        "notification_limit" => {
            let n = parse_int(value)?;
            g.notification_limit = n.max(0) as usize;
        }
        "sort" => g.sort = parse_bool(value)?,
        "mouse_left_click" => g.mouse_left_click = parse_mouse_actions(value)?,
        "mouse_middle_click" => g.mouse_middle_click = parse_mouse_actions(value)?,
        "mouse_right_click" => g.mouse_right_click = parse_mouse_actions(value)?,
        // Recognized L2 keys: parsed, not acted on yet.
        "stack_duplicates" => g.stack_duplicates = parse_bool(value)?,
        "hide_duplicate_count" => g.hide_duplicate_count = parse_bool(value)?,
        "ignore_newline" => g.ignore_newline = parse_bool(value)?,
        "scale" => {
            warn_unimplemented("scale (GTK handles HiDPI)", value);
        }
        _ => return Err(format!("unknown key {key:?}")),
    }
    Ok(())
}

fn apply_urgency(u: &mut UrgencyConfig, key: &str, value: &str) -> Result<(), String> {
    match key {
        "timeout" => {
            let n = parse_int(value)?;
            u.timeout = n.max(0);
        }
        "background" => u.background = parse_color(value)?,
        "foreground" => u.foreground = parse_color(value)?,
        "frame_color" => u.frame_color = parse_color(value)?,
        "highlight" => u.highlight = parse_color(value)?,
        "icon" => {
            warn_unimplemented("urgency icon override", value);
        }
        _ => return Err(format!("unknown key {key:?}")),
    }
    Ok(())
}

fn warn_unimplemented(what: &str, value: &str) {
    log::warn!("config key {what} ({value:?}) parsed but not implemented yet");
}

// ------------------------------------------------------------- value parsers

fn parse_int(v: &str) -> Result<i32, String> {
    v.trim()
        .parse::<i32>()
        .map_err(|_| format!("expected integer, got {v:?}"))
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v.trim().to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(format!("expected boolean, got {v:?}")),
    }
}

/// "300" | "(0, 300)" | "62%"
fn parse_size(v: &str) -> Result<SizeSpec, String> {
    let v = v.trim();
    if let Some(pct) = v.strip_suffix('%') {
        let n: f64 = pct
            .trim()
            .parse()
            .map_err(|_| format!("expected percentage, got {v:?}"))?;
        return Ok(SizeSpec::Percent(n.clamp(0.0, 200.0) / 100.0));
    }
    if v.starts_with('(') && v.ends_with(')') {
        let inner = &v[1..v.len() - 1];
        let mut parts = inner.split(',');
        let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!("expected (min, max), got {v:?}"));
        };
        return Ok(SizeSpec::Range(parse_int(a)?, parse_int(b)?));
    }
    Ok(SizeSpec::Constant(parse_int(v)?))
}

/// "(10, 100)" (new format) or "10x300" (legacy).
fn parse_offset(v: &str) -> Result<(i32, i32), String> {
    let v = v.trim();
    if v.starts_with('(') && v.ends_with(')') {
        let inner = &v[1..v.len() - 1];
        let mut parts = inner.split(',');
        let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!("expected (x, y), got {v:?}"));
        };
        return Ok((parse_int(a)?, parse_int(b)?));
    }
    // Legacy NxN (or Nx-N with signs handled by the caller via geometry).
    if let Some((a, b)) = v.split_once('x') {
        return Ok((parse_int(a)?, parse_int(b)?));
    }
    Err(format!("expected (x, y) or NxN, got {v:?}"))
}

/// Legacy geometry: "WxH+X+Y" (signs significant, e.g. "0x0-10+10").
fn parse_geometry(v: &str) -> Result<(i32, i32, i32, i32), String> {
    let v = v.trim();
    let (w, rest) = split_signed(v)?;
    let rest = rest
        .strip_prefix('x')
        .ok_or_else(|| format!("expected WxH+X+Y, got {v:?}"))?;
    let (h, rest) = split_signed(rest)?;
    let (x, rest) = split_signed(rest)?;
    let (y, rest) = split_signed(rest)?;
    if !rest.is_empty() {
        return Err(format!("trailing data in geometry {v:?}"));
    }
    Ok((w, h, x, y))
}

/// Split "300-10+20" into (300, "-10+20"); helper for signed parsing.
fn split_signed(s: &str) -> Result<(i32, &str), String> {
    let s = s.trim();
    let mut end = s.len();
    for (i, c) in s.char_indices().skip(1) {
        if c == '+' || c == '-' || c == 'x' {
            end = i;
            break;
        }
    }
    let (num, rest) = s.split_at(end);
    let n: i32 = num
        .parse()
        .map_err(|_| format!("expected number in {s:?}"))?;
    Ok((n, rest))
}

fn origin_from_signs(x: i32, y: i32) -> Origin {
    match (x < 0, y < 0) {
        (false, false) => Origin::TopLeft,
        (true, false) => Origin::TopRight,
        (false, true) => Origin::BottomLeft,
        (true, true) => Origin::BottomRight,
    }
}

fn parse_origin(v: &str) -> Result<Origin, String> {
    match v.trim().to_lowercase().replace('_', "-").as_str() {
        "top-left" => Ok(Origin::TopLeft),
        "top-center" | "top" => Ok(Origin::TopCenter),
        "top-right" => Ok(Origin::TopRight),
        "left" => Ok(Origin::Left),
        "center" => Ok(Origin::Center),
        "right" => Ok(Origin::Right),
        "bottom-left" => Ok(Origin::BottomLeft),
        "bottom-center" | "bottom" => Ok(Origin::BottomCenter),
        "bottom-right" => Ok(Origin::BottomRight),
        _ => Err(format!("unknown origin {v:?}")),
    }
}

fn parse_color(v: &str) -> Result<Color, String> {
    let v = v.trim();
    let hex = v.strip_prefix('#').ok_or_else(|| format!("expected #RGB color, got {v:?}"))?;
    if hex.len() != 3 && hex.len() != 4 && hex.len() != 6 && hex.len() != 8 {
        return Err(format!("expected #RGB/#RGBA/#RRGGBB/#RRGGBBAA, got {v:?}"));
    }
    let expand = |h: &str| -> Result<u8, String> {
        u8::from_str_radix(h, 16).map_err(|_| format!("invalid hex in color {v:?}"))
    };
    let (r, g, b, a) = match hex.len() {
        3 => (
            expand(&hex[0..1]).map(|n| n * 17)?,
            expand(&hex[1..2]).map(|n| n * 17)?,
            expand(&hex[2..3]).map(|n| n * 17)?,
            255,
        ),
        4 => (
            expand(&hex[0..1]).map(|n| n * 17)?,
            expand(&hex[1..2]).map(|n| n * 17)?,
            expand(&hex[2..3]).map(|n| n * 17)?,
            expand(&hex[3..4]).map(|n| n * 17)?,
        ),
        6 => (
            expand(&hex[0..2])?,
            expand(&hex[2..4])?,
            expand(&hex[4..6])?,
            255,
        ),
        _ => (
            expand(&hex[0..2])?,
            expand(&hex[2..4])?,
            expand(&hex[4..6])?,
            expand(&hex[6..8])?,
        ),
    };
    Ok(Color { r, g, b, a })
}

fn parse_markup(v: &str) -> Result<Markup, String> {
    match v.trim().to_lowercase().as_str() {
        "full" => Ok(Markup::Full),
        "strip" => Ok(Markup::Strip),
        "no" | "none" => Ok(Markup::No),
        _ => Err(format!("expected full/strip/no, got {v:?}")),
    }
}

fn parse_ellipsize(v: &str) -> Result<Ellipsize, String> {
    match v.trim().to_lowercase().as_str() {
        "start" => Ok(Ellipsize::Start),
        "middle" => Ok(Ellipsize::Middle),
        "end" => Ok(Ellipsize::End),
        _ => Err(format!("expected start/middle/end, got {v:?}")),
    }
}

fn parse_alignment(v: &str) -> Result<Alignment, String> {
    match v.trim().to_lowercase().as_str() {
        "left" => Ok(Alignment::Left),
        "center" => Ok(Alignment::Center),
        "right" => Ok(Alignment::Right),
        _ => Err(format!("expected left/center/right, got {v:?}")),
    }
}

fn parse_vertical_alignment(v: &str) -> Result<VerticalAlignment, String> {
    match v.trim().to_lowercase().as_str() {
        "top" => Ok(VerticalAlignment::Top),
        "center" => Ok(VerticalAlignment::Center),
        "bottom" => Ok(VerticalAlignment::Bottom),
        _ => Err(format!("expected top/center/bottom, got {v:?}")),
    }
}

fn parse_icon_position(v: &str) -> Result<IconPosition, String> {
    match v.trim().to_lowercase().as_str() {
        "left" => Ok(IconPosition::Left),
        "right" => Ok(IconPosition::Right),
        "top" => Ok(IconPosition::Top),
        "off" => Ok(IconPosition::Off),
        _ => Err(format!("expected left/right/top/off, got {v:?}")),
    }
}

fn parse_follow(v: &str) -> Result<Follow, String> {
    match v.trim().to_lowercase().as_str() {
        "none" => Ok(Follow::None),
        "mouse" => Ok(Follow::Mouse),
        "keyboard" | "focus" => Ok(Follow::Keyboard),
        _ => Err(format!("expected none/mouse/keyboard, got {v:?}")),
    }
}

/// "close_current" | "do_action, close_current" | "none"
fn parse_mouse_actions(v: &str) -> Result<Vec<MouseAction>, String> {
    let mut out = Vec::new();
    for part in v.split(',') {
        match part.trim().to_lowercase().as_str() {
            "none" => out.push(MouseAction::None),
            "close_current" => out.push(MouseAction::CloseCurrent),
            "close_all" => out.push(MouseAction::CloseAll),
            "do_action" => out.push(MouseAction::DoAction),
            "context" => out.push(MouseAction::Context),
            other => return Err(format!("unknown mouse action {other:?}")),
        }
    }
    Ok(out)
}

// --------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.global.origin, Origin::TopRight);
        assert_eq!(c.global.offset, (10, 10));
        assert_eq!(c.urgency[0].timeout, 10);
        assert_eq!(c.urgency[2].timeout, 0, "critical never expires");
        assert_eq!(c.global.font, "Monospace 8");
    }

    #[test]
    fn parses_new_format_like_user_config() {
        let src = r##"
            [global]
            monitor = 0
            follow = mouse
            width = (500, 1000)
            height = (0,1000)
            origin = top-center
            offset = (0,100)
            padding = 20
            horizontal_padding = 20
            font = 鸿蒙黑体 14
            frame_width = 1
            frame_color = "#AAAAAA"
            alignment = center
            vertical_alignment = center
            word_wrap = yes
            ellipsize = middle
            icon_position = left
            min_icon_size = 64
            max_icon_size = 128
            mouse_left_click = close_current
            mouse_middle_click = do_action, close_current
            mouse_right_click = close_all

            [urgency_low]
            background = "#000000CC"
            foreground = "#CCCC00"
            timeout = 10

            [urgency_normal]
            background = "#000000AA"
            foreground = "#FFFFFF"
            timeout = 0

            [urgency_critical]
            background = "#000000CC"
            foreground = "#FFFF00"
            frame_color = "#FF0000"
            timeout = 0
        "##;
        let (c, warnings) = parse(src);
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(c.global.width, SizeSpec::Range(500, 1000));
        assert_eq!(c.global.height, SizeSpec::Range(0, 1000));
        assert_eq!(c.global.origin, Origin::TopCenter);
        assert_eq!(c.global.offset, (0, 100));
        assert_eq!(c.global.font, "鸿蒙黑体 14");
        assert_eq!(c.global.frame_width, 1);
        assert_eq!(c.global.alignment, Alignment::Center);
        assert!(c.global.word_wrap);
        assert_eq!(c.global.ellipsize, Ellipsize::Middle);
        assert_eq!(c.global.icon_position, IconPosition::Left);
        assert_eq!(c.global.max_icon_size, 128);
        assert_eq!(c.global.follow, Follow::Mouse);
        assert_eq!(
            c.global.mouse_middle_click,
            vec![MouseAction::DoAction, MouseAction::CloseCurrent]
        );
        assert_eq!(
            c.urgency[0].background,
            Color { r: 0, g: 0, b: 0, a: 0xcc }
        );
        assert_eq!(c.urgency[1].timeout, 0);
        assert_eq!(
            c.urgency[2].frame_color,
            Color { r: 0xff, g: 0, b: 0, a: 0xff }
        );
    }

    #[test]
    fn parses_legacy_geometry() {
        let src = "[global]\ngeometry = \"300x100-10+20\"\n";
        let (c, warnings) = parse(src);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(c.global.width, SizeSpec::Constant(300));
        assert_eq!(c.global.height, SizeSpec::Constant(100));
        assert_eq!(c.global.origin, Origin::TopRight);
        assert_eq!(c.global.offset, (10, 20));

        let src = "[global]\ngeometry = 0x0+10-10\n";
        let (c, _) = parse(src);
        assert_eq!(c.global.origin, Origin::BottomLeft);
        assert_eq!(c.global.offset, (10, 10));
    }

    #[test]
    fn parses_legacy_offset_and_percent() {
        let (c, w) = parse("[global]\noffset = 10x300\nwidth = \"62%\"\n");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.global.offset, (10, 300));
        assert_eq!(c.global.width, SizeSpec::Percent(0.62));
    }

    #[test]
    fn comments_quotes_and_case() {
        let src = r#"
            # full line comment
            ; also this
            [GLOBAL]
            FONT = "Sans 12"
            MARKUP = FULL
            word_wrap = no
        "#;
        let (c, warnings) = parse(src);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(c.global.font, "Sans 12");
        assert_eq!(c.global.markup, Markup::Full);
        assert!(!c.global.word_wrap);
    }

    #[test]
    fn colors_all_formats() {
        let (c, w) = parse(
            "[urgency_low]\nbackground = #f00\nforeground = #00ff00\nframe_color = #0000FFFF\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.urgency[0].background, Color { r: 255, g: 0, b: 0, a: 255 });
        assert_eq!(c.urgency[0].foreground, Color { r: 0, g: 255, b: 0, a: 255 });
        assert_eq!(c.urgency[0].frame_color, Color { r: 0, g: 0, b: 255, a: 255 });
    }

    #[test]
    fn unknown_keys_and_sections_warn_but_keep_defaults() {
        let (c, warnings) = parse("[global]\nbogus_key = 1\n[urgency_low]\nnot_a_key = x\n");
        assert!(warnings.len() >= 2, "{warnings:?}");
        assert_eq!(c.global.gap_size, 0);
    }

    #[test]
    fn malformed_values_warn() {
        let (_, warnings) = parse("[global]\nframe_width = notanumber\n");
        assert!(!warnings.is_empty());
    }

    #[test]
    fn missing_config_file_is_not_fatal() {
        let (cfg, warnings) = Config::load(Some("/nonexistent/dunstrc"));
        assert_eq!(cfg.global.origin, Origin::TopRight);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn css_rgba_output() {
        let c = Color { r: 0, g: 0, b: 0, a: 0xcc };
        assert_eq!(c.css_rgba(), "rgba(0, 0, 0, 0.800)");
    }
}
