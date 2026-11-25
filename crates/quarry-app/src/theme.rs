// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Color palettes, syntax tokens, theme loading, and system appearance detection.
//!
//! Provides [`Theme`] definitions for Quarry Dark, Quarry Light, Graphite, Paper,
//! and user-customizable JSON themes.

use gpui::*;

#[allow(dead_code)]
#[derive(Clone)]
pub struct Theme {
    pub name: String,
    pub is_dark: bool,
    pub bg_app: Rgba,
    pub bg_sidebar: Rgba,
    pub bg_panel: Rgba,
    pub bg_surface: Rgba,
    pub bg_hover: Rgba,
    pub bg_selected: Rgba,
    pub border: Rgba,
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub null_token: Rgba,
    pub pending_cell_bg: Rgba,
    /// Painted once at the window root; differs from `bg_app` only when translucent
    pub bg_window: Rgba,
    /// Blur the desktop behind the window
    pub translucent: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Built-in themes, in picker order
pub const BUILT_IN: [&str; 4] = ["Quarry Dark", "Quarry Light", "Graphite", "Paper"];

impl Theme {
    pub fn for_mode(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// Cool, low-contrast neutrals with a periwinkle accent
    pub fn dark() -> Self {
        Self {
            name: "Quarry Dark".into(),
            is_dark: true,
            bg_app: rgb(0x111214),
            bg_sidebar: rgb(0x151619),
            bg_panel: rgb(0x17181b),
            bg_surface: rgb(0x1e2024),
            bg_hover: rgb(0x24262b),
            bg_selected: rgb(0x2a3042),
            border: rgb(0x25272c),
            text_primary: rgb(0xe6e7ea),
            text_secondary: rgb(0xa3a6ae),
            text_muted: rgb(0x9296a0),
            accent: rgb(0x7b93ff),
            accent_hover: rgb(0x98aaff),
            success: rgb(0x4cc38a),
            warning: rgb(0xe5a84b),
            danger: rgb(0xef6b73),
            null_token: rgb(0x7b7e86),
            pending_cell_bg: rgba(0xe5a84b38),
            bg_window: rgb(0x111214),
            translucent: false,
        }
    }

    pub fn light() -> Self {
        Self {
            name: "Quarry Light".into(),
            is_dark: false,
            bg_app: rgb(0xffffff),
            bg_sidebar: rgb(0xf7f7f8),
            bg_panel: rgb(0xf4f4f6),
            bg_surface: rgb(0xededf0),
            bg_hover: rgb(0xe8e8ec),
            bg_selected: rgb(0xe2e7fc),
            border: rgb(0xe3e3e7),
            text_primary: rgb(0x1b1c20),
            text_secondary: rgb(0x52555e),
            text_muted: rgb(0x5c6068),
            accent: rgb(0x4a63e8),
            accent_hover: rgb(0x3a52d4),
            success: rgb(0x1f9a62),
            warning: rgb(0xa06919),
            danger: rgb(0xd2414c),
            null_token: rgb(0x73757b),
            pending_cell_bg: rgba(0xa0691933),
            bg_window: rgb(0xffffff),
            translucent: false,
        }
    }

    /// Near-black and neutral, amber accent
    pub fn graphite() -> Self {
        Self {
            name: "Graphite".into(),
            bg_app: rgb(0x0b0b0c),
            bg_window: rgb(0x0b0b0c),
            bg_sidebar: rgb(0x101011),
            bg_panel: rgb(0x121213),
            bg_surface: rgb(0x1a1a1b),
            bg_hover: rgb(0x212122),
            bg_selected: rgb(0x2b2618),
            border: rgb(0x1f1f21),
            text_primary: rgb(0xe8e6e3),
            text_secondary: rgb(0xa29f99),
            text_muted: rgb(0x858380),
            accent: rgb(0xf2b544),
            accent_hover: rgb(0xf7c96f),
            ..Self::dark()
        }
    }

    /// Warm paper tones, ink-blue accent
    pub fn paper() -> Self {
        Self {
            name: "Paper".into(),
            bg_app: rgb(0xfbfaf7),
            bg_window: rgb(0xfbfaf7),
            bg_sidebar: rgb(0xf4f2ec),
            bg_panel: rgb(0xf1efe8),
            bg_surface: rgb(0xe9e6dd),
            bg_hover: rgb(0xe4e0d6),
            bg_selected: rgb(0xdfe4f0),
            border: rgb(0xe2ded3),
            text_primary: rgb(0x23211c),
            text_secondary: rgb(0x5c574c),
            text_muted: rgb(0x6a655c),
            accent: rgb(0x2f5bb7),
            accent_hover: rgb(0x244a9b),
            ..Self::light()
        }
    }

    /// Any theme over a blurred desktop (Settings → Window translucency). `opacity` is how much
    /// of the theme colour covers the blur. Only the window root paints it: layers that repainted
    /// `bg_app` on top would stack back up to opaque, so `bg_app` goes clear and sidebar/panels
    /// keep a faint tint to stay distinct. Surfaces, hovers, selections and text stay solid.
    pub fn with_translucency(mut self, opacity: f32) -> Self {
        let alpha = |c: Rgba, a: f32| Rgba { a, ..c };
        self.bg_window = alpha(self.bg_app, opacity);
        self.bg_sidebar = alpha(self.bg_sidebar, 0.25);
        self.bg_panel = alpha(self.bg_panel, 0.25);
        self.bg_app = alpha(self.bg_app, 0.0);
        self.translucent = true;
        self
    }

    /// The colour with its alpha dropped: for modals, menus and text-on-fill, which must stay readable
    pub fn solid(c: Rgba) -> Rgba {
        Rgba { a: 1.0, ..c }
    }

    /// Text on an accent/status fill: dark themes use their own background (white is unreadable on amber)
    pub fn on_accent(&self) -> Rgba {
        if self.is_dark {
            Self::solid(self.bg_app)
        } else {
            rgb(0xffffff)
        }
    }

    /// A translucent wash of a theme colour, for error bars and row states
    pub fn tint(color: Rgba, alpha: f32) -> Hsla {
        Hsla::from(color).opacity(alpha)
    }

    /// Lines between grid rows and columns: visible on every theme without shouting
    pub fn grid_line(&self) -> Hsla {
        Hsla::from(self.text_muted).opacity(0.22)
    }

    /// A theme by name: "system" follows the OS, then built-ins, then ~/.config/quarry/themes/<name>.json
    pub fn named(name: &str, os_dark: bool) -> Result<Self, String> {
        match name.trim().to_lowercase().as_str() {
            "" | "system" => Ok(Self::for_mode(os_dark)),
            "dark" | "quarry dark" => Ok(Self::dark()),
            "light" | "quarry light" => Ok(Self::light()),
            "graphite" => Ok(Self::graphite()),
            "paper" => Ok(Self::paper()),
            _ => {
                let path = themes_dir()
                    .ok_or("no home directory")?
                    .join(format!("{name}.json"));
                let text =
                    std::fs::read_to_string(&path).map_err(|e| format!("theme \"{name}\": {e}"))?;
                Self::from_json(name, &text).map_err(|e| format!("themes/{name}.json: {e}"))
            }
        }
    }

    /// `{"base": "dark", "accent": "#7c5cff", ...}`: a built-in base plus any token overrides
    pub fn from_json(name: &str, text: &str) -> Result<Self, String> {
        let map: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_str(text).map_err(|e| e.to_string())?;
        let base = map.get("base").and_then(|v| v.as_str()).unwrap_or("dark");
        let mut t = Self::named(base, true).map_err(|_| format!("unknown base \"{base}\""))?;
        t.name = name.to_string();
        for (key, value) in map.iter().filter(|(k, _)| *k != "base") {
            let hex = value
                .as_str()
                .ok_or_else(|| format!("\"{key}\" must be a colour string"))?;
            let color = parse_hex(hex)
                .ok_or_else(|| format!("\"{key}\": \"{hex}\" isn't #rrggbb or #rrggbbaa"))?;
            let slot = match key.as_str() {
                "bg_app" => &mut t.bg_app,
                "bg_sidebar" => &mut t.bg_sidebar,
                "bg_panel" => &mut t.bg_panel,
                "bg_surface" => &mut t.bg_surface,
                "bg_hover" => &mut t.bg_hover,
                "bg_selected" => &mut t.bg_selected,
                "border" => &mut t.border,
                "text_primary" => &mut t.text_primary,
                "text_secondary" => &mut t.text_secondary,
                "text_muted" => &mut t.text_muted,
                "accent" => &mut t.accent,
                "accent_hover" => &mut t.accent_hover,
                "success" => &mut t.success,
                "warning" => &mut t.warning,
                "danger" => &mut t.danger,
                "null_token" => &mut t.null_token,
                "pending_cell_bg" => &mut t.pending_cell_bg,
                _ => return Err(format!("unknown token \"{key}\"")),
            };
            *slot = color;
        }
        t.bg_window = t.bg_app;
        Ok(t)
    }

    /// Paints gpui-kit's widgets (inputs, buttons, menus, tables) with this palette
    pub fn apply_to_kit(&self, cx: &mut App) {
        gpui::component::theme::Theme::update(cx, |kit| {
            // Soft 6px corners and flat surfaces, like Zed
            kit.radius = px(6.0);
            kit.shadow = false;
            // Focus = accent border only; the outer glow read as a second border
            kit.focus_ring = false;
            let c = &mut kit.colors;
            let h = |r: Rgba| -> Hsla { r.into() };
            c.background = h(self.bg_app);
            c.foreground = h(self.text_primary);
            c.border = h(self.border);
            c.input = h(self.border);
            c.ring = h(self.accent);
            c.caret = h(self.accent);
            c.selection = h(self.accent).opacity(0.3);
            c.popover = h(Self::solid(self.bg_sidebar));
            c.popover_foreground = h(self.text_primary);
            c.muted = h(self.bg_surface);
            c.muted_foreground = h(self.text_muted);
            c.accent = h(self.bg_hover);
            c.accent_foreground = h(self.text_primary);
            c.primary = h(self.accent);
            c.primary_hover = h(self.accent_hover);
            c.primary_active = h(self.accent);
            c.primary_foreground = h(self.on_accent());

            // Primary action buttons (Submit, Run, Connect, etc.)
            c.button_primary = h(self.accent);
            c.button_primary_hover = h(self.accent_hover);
            c.button_primary_active = h(self.accent);
            c.button_primary_foreground = h(self.on_accent());

            // Default buttons
            c.button = h(self.bg_surface);
            c.button_hover = h(self.bg_hover);
            c.button_active = h(self.bg_selected);
            c.button_foreground = h(self.text_primary);

            // Secondary buttons
            c.secondary = h(self.bg_surface);
            c.secondary_hover = h(self.bg_hover);
            c.secondary_active = h(self.bg_selected);
            c.secondary_foreground = h(self.text_primary);
            c.button_secondary = h(self.bg_surface);
            c.button_secondary_hover = h(self.bg_hover);
            c.button_secondary_active = h(self.bg_selected);
            c.button_secondary_foreground = h(self.text_primary);

            // Danger buttons
            c.danger = h(self.danger);
            c.danger_foreground = h(rgb(0xffffff));
            c.button_danger = h(self.danger);
            c.button_danger_hover = h(self.danger).opacity(0.85);
            c.button_danger_active = h(self.danger);
            c.button_danger_foreground = h(rgb(0xffffff));

            c.list = h(self.bg_app);
            c.list_hover = h(self.bg_hover);
            c.list_active = h(self.bg_selected);
            c.list_active_border = h(self.accent);
            c.list_head = h(self.bg_panel);
            c.table = h(self.bg_app);
            c.table_head = h(self.bg_panel);
            c.table_head_foreground = h(self.text_secondary);
            c.table_hover = h(self.bg_hover);
            c.table_active = gpui::transparent_black();
            c.table_active_border = h(self.accent);
            c.table_even = h(self.bg_app);
            c.table_row_border = self.grid_line();
            c.success = h(self.success);
            c.warning = h(self.warning);
            c.link = h(self.accent);
            c.scrollbar = gpui::transparent_black();
            c.scrollbar_thumb = h(self.text_muted).opacity(0.35);
            c.scrollbar_thumb_hover = h(self.text_muted).opacity(0.6);

            kit.tokens = (&kit.colors).into();

            // The SQL editor paints its syntax theme's background; match the app surface (as the diagram does)
            let mut hl = (*kit.highlight_theme).clone();
            hl.style.editor_background = Some(h(self.bg_app));
            hl.style.editor_gutter_background = Some(h(self.bg_app));
            kit.highlight_theme = std::sync::Arc::new(hl);
        });
    }
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let hex = s.strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(rgb(v)),
        8 => Some(rgba(v)),
        _ => None,
    }
}

pub fn themes_dir() -> Option<std::path::PathBuf> {
    Some(crate::connections::config_dir()?.join("themes"))
}

/// "System", the built-ins, then every themes/*.json
pub fn available() -> Vec<String> {
    let mut names: Vec<String> = std::iter::once("System".to_string())
        .chain(BUILT_IN.iter().map(|s| s.to_string()))
        .collect();
    if let Some(entries) = themes_dir().and_then(|d| std::fs::read_dir(d).ok()) {
        let mut user: Vec<String> = entries
            .filter_map(|e| {
                e.ok()?
                    .path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .filter(|n| {
                !n.eq_ignore_ascii_case("example")
                    && !names.iter().any(|b| b.eq_ignore_ascii_case(n))
            })
            .collect();
        user.sort();
        names.extend(user);
    }
    names
}

/// Interface text
pub const UI_FONT: &str = "Space Grotesk";
/// SQL, cell values and anything that should line up; the Nerd Font build also carries icon glyphs
pub const MONO_FONT: &str = "JetBrainsMono NF";

/// Registers the bundled fonts (OFL/FFL, licences beside them in assets/fonts) and makes them the defaults
pub fn load_fonts(cx: &mut gpui::App) {
    use std::borrow::Cow;
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/Satoshi-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Satoshi-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Satoshi-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-SemiBold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf")),
        Cow::Borrowed(include_bytes!(
            "../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf"
        )),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("quarry: bundled fonts failed to load, using system fonts: {e}");
        return;
    }
    apply_fonts(cx, "", "");
}

/// gpui-kit resets fonts on every theme change, so this runs after each one.
/// `ui` overrides UI_FONT and `mono` overrides MONO_FONT (settings.json).
pub fn apply_fonts(cx: &mut gpui::App, ui: &str, mono: &str) {
    let kit = gpui::component::theme::Theme::global_mut(cx);
    kit.font_family = if ui.trim().is_empty() {
        UI_FONT.into()
    } else {
        ui.to_string().into()
    };
    kit.mono_font_family = if mono.trim().is_empty() {
        MONO_FONT.into()
    } else {
        mono.to_string().into()
    };
}

#[cfg(test)]
mod tests {
    use super::Theme;

    #[test]
    fn user_theme_overrides_a_base() {
        let t = Theme::from_json(
            "Grape",
            r##"{"base": "light", "accent": "#7c5cff", "pending_cell_bg": "#ff000033"}"##,
        )
        .ok()
        .unwrap();
        assert_eq!((t.name.as_str(), t.is_dark), ("Grape", false));
        assert_eq!(t.accent, gpui::rgb(0x7c5cff));
        assert_eq!(t.bg_app, Theme::light().bg_app);
        assert!(Theme::from_json("x", r##"{"accnet": "#fff000"}"##)
            .err()
            .unwrap()
            .contains("unknown token"));
        assert!(Theme::from_json("x", r##"{"accent": "blue"}"##).is_err());
    }
}
