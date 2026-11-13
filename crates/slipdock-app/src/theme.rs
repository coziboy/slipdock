//! The look of the app: its colours, its corners, and the band of a profile.
//!
//! The component library draws from a theme. `apply` gives it Slipdock's own
//! one, as a theme file built from the colours below, so buttons, inputs and
//! dialogs come out in the same palette as what the screens draw themselves.
//!
//! The one thing here that is more than paint is the **band**: every profile
//! has a flat colour, and wherever that profile or one of its dumps is on
//! show, a block of that colour stands beside it. Which band a profile has
//! is a setting in the core (`Settings::bands`), so it does not change from
//! one start to the next.

use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::*;
use slipdock_core::profile;

use crate::state::Core;

/// Text, and the fill of the main button.
pub const INK: u32 = 0x17161A;
/// Behind the page.
pub const GROUND: u32 = 0xF5F6F8;
/// Cards, inputs and dialogs.
pub const PANEL: u32 = 0xFFFFFF;
/// Lines between and around things.
pub const RULE: u32 = 0xD9DBE1;
/// The edge of an input and of an outlined button.
pub const EDGE: u32 = 0x8E8C98;
/// Explanations and labels.
pub const DIM: u32 = 0x5E5C66;
/// A quiet fill: a hovered row, the chosen row, a quoted name.
pub const QUIET: u32 = 0xECEDF1;
/// The navigation.
pub const RAIL: u32 = 0x25222B;
/// Under the pointer in the navigation: between the rail and the chosen one.
pub const RAIL_HOVER: u32 = 0x2D2934;
pub const RAIL_CHOSEN: u32 = 0x403A4C;
pub const RAIL_INK: u32 = 0xECEAF1;
pub const RAIL_DIM: u32 = 0xB1ACBD;
/// What destroys data, and a failed run.
pub const DANGER: u32 = 0xC92434;
/// A complete dump, a tool that works.
pub const SUCCESS: u32 = 0x10713C;
/// A partial dump, a warning of a tool.
pub const WARNING: u32 = 0x964F00;
/// The ring of the focused control.
pub const FOCUS: u32 = 0x2B6FE0;

/// The bands, in the order profiles get them. Six, because that is how many
/// hues can be told apart at a glance while staying clear of the red, green
/// and amber above (those say how something went, a band says whose it is)
/// and of the blue of the focus ring. A seventh profile shares a band.
pub const BANDS: [u32; 6] = [
    0x0B8F8A, // teal
    0x7650D8, // violet
    0x857F00, // olive
    0xC93A96, // magenta
    0x4F8A12, // green
    0x9B3FB5, // purple
];

/// The corners of what the screens draw themselves; the theme file below
/// gives the components the same.
pub const RADIUS: Pixels = px(4.);

pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn hex(color: u32) -> String {
    format!("#{color:06X}")
}

/// The theme file. The keys are the ones the component library reads; a
/// colour that is not named falls back to the library's light theme.
fn theme_file() -> String {
    let pairs = [
        ("background", PANEL),
        ("foreground", INK),
        ("border", RULE),
        ("input.border", EDGE),
        ("caret", INK),
        ("ring", FOCUS),
        ("link", INK),
        ("muted.background", QUIET),
        ("muted.foreground", DIM),
        ("accent.background", QUIET),
        ("accent.foreground", INK),
        ("secondary.background", QUIET),
        ("secondary.foreground", INK),
        ("secondary.hover.background", 0xE2E3E9),
        ("secondary.active.background", RULE),
        ("primary.background", INK),
        ("primary.foreground", PANEL),
        ("primary.hover.background", 0x34323A),
        ("primary.active.background", 0x000000),
        ("danger.background", DANGER),
        ("danger.foreground", PANEL),
        ("danger.hover.background", 0xB01E2D),
        ("danger.active.background", 0x98192A),
        ("success.background", SUCCESS),
        ("success.foreground", PANEL),
        ("warning.background", WARNING),
        ("warning.foreground", PANEL),
        ("info.background", FOCUS),
        ("info.foreground", PANEL),
        ("popover.background", PANEL),
        ("popover.foreground", INK),
        ("list.hover.background", QUIET),
        ("list.active.background", QUIET),
        ("list.active.border", INK),
        ("sidebar.background", RAIL),
        ("sidebar.foreground", RAIL_INK),
        ("sidebar.accent.background", RAIL_CHOSEN),
        ("sidebar.accent.foreground", PANEL),
        ("sidebar.border", RAIL),
    ];
    let mut colors: Vec<String> = pairs
        .iter()
        .map(|(key, color)| format!("\"{key}\": \"{}\"", hex(*color)))
        .collect();
    // Selected text: the focus colour, let through.
    colors.push(format!("\"selection.background\": \"{}40\"", hex(FOCUS)));
    format!(
        "{{\"name\": \"Slipdock\", \"themes\": [{{\"name\": \"{NAME}\", \"mode\": \"light\", \
         \"radius\": 4, \"radius.lg\": 4, \"shadow\": false, \"colors\": {{{}}}}}]}}",
        colors.join(", ")
    )
}

const NAME: &str = "Slipdock Light";

/// Makes Slipdock's theme the theme of the app. Call it once, after
/// `gpui_kit::init`.
pub fn apply(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    registry
        .load_themes_from_str(&theme_file())
        .expect("the theme file is made by this module");
    let Some(config) = registry.themes().get(NAME).cloned() else {
        return;
    };
    Theme::update(cx, |theme| theme.apply_config(&config));
}

/// Gives every profile a band and forgets the bands of deleted profiles.
/// Called when the window opens and when a screen is shown, so a profile
/// made a moment ago has its band on the next screen.
pub fn assign_bands(cx: &mut App) {
    cx.update_global::<Core, _>(|core, _| {
        let names = profile::names(&core.0.config_dir);
        if core.0.settings.assign_bands(&names, BANDS.len() as u8) {
            // When the settings cannot be written, the bands still hold
            // until the app is closed. A screen that saves settings for its
            // own reason then says what is wrong with the file.
            let _ = core.0.settings.save(&core.0.config_dir);
        }
    });
}

/// The band colour of a profile. `None` for a name that is not a profile:
/// a dump of a deleted profile, or a folder named by hand.
pub fn band(profile: &str, cx: &App) -> Option<Hsla> {
    let band = cx.global::<Core>().0.settings.band(profile)?;
    BANDS.get(band as usize).map(|hex| color(*hex))
}

#[cfg(test)]
mod tests {
    // Named one by one: a `*` would bring in `gpui_kit::test`, which takes
    // the place of the plain `#[test]` these tests want.
    use super::{
        BANDS, DANGER, DIM, EDGE, FOCUS, GROUND, INK, PANEL, QUIET, RAIL, RAIL_CHOSEN, RAIL_DIM,
        RAIL_HOVER, RAIL_INK, SUCCESS, WARNING,
    };

    fn channels(color: u32) -> [f32; 3] {
        [16, 8, 0].map(|shift| ((color >> shift) & 0xFF) as f32 / 255.)
    }

    /// The contrast of two colours as WCAG counts it, from 1 to 21.
    fn contrast(a: u32, b: u32) -> f32 {
        let luminance = |color: u32| {
            let [r, g, b] = channels(color).map(|c| {
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Where a colour is on the colour wheel, in degrees.
    fn hue(color: u32) -> f32 {
        let [r, g, b] = channels(color);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let delta = max - min;
        let hue = if max == r {
            ((g - b) / delta).rem_euclid(6.)
        } else if max == g {
            (b - r) / delta + 2.
        } else {
            (r - g) / delta + 4.
        };
        hue * 60.
    }

    #[test]
    fn text_can_be_read_on_its_ground() {
        let pairs = [
            ("text on a card", INK, PANEL),
            ("text on the page", INK, GROUND),
            ("a hint on a card", DIM, PANEL),
            ("a hint on the page", DIM, GROUND),
            ("a hint on a quiet fill", DIM, QUIET),
            ("the main button", PANEL, INK),
            ("navigation", RAIL_INK, RAIL),
            ("the chosen screen", PANEL, RAIL_CHOSEN),
            ("a shortcut in the navigation", RAIL_DIM, RAIL),
            ("a shortcut on the chosen screen", RAIL_DIM, RAIL_CHOSEN),
            ("navigation under the pointer", RAIL_INK, RAIL_HOVER),
            ("a shortcut under the pointer", RAIL_DIM, RAIL_HOVER),
            ("danger text in a chosen row", DANGER, QUIET),
            ("success text in a chosen row", SUCCESS, QUIET),
            ("warning text in a chosen row", WARNING, QUIET),
            ("a danger button", PANEL, DANGER),
            ("danger text", DANGER, PANEL),
            ("success text", SUCCESS, PANEL),
            ("warning text", WARNING, PANEL),
        ];
        for (what, text, ground) in pairs {
            let found = contrast(text, ground);
            assert!(found >= 4.5, "{what}: {found:.2}");
        }
        // An edge is not text: it has to stand out, not to be read.
        assert!(contrast(EDGE, PANEL) >= 3.);
        // A band is on a card or on the quiet fill of a chosen row.
        for band in BANDS {
            assert!(contrast(band, PANEL) >= 3., "band {band:06X}");
            assert!(contrast(band, QUIET) >= 3., "band {band:06X} on quiet");
        }
    }

    #[test]
    fn a_band_is_not_taken_for_a_state() {
        for (index, band) in BANDS.iter().enumerate() {
            assert!(!BANDS[..index].contains(band), "{band:06X} is there twice");
            // Two bands are told apart by their hue.
            for other in &BANDS[..index] {
                let apart = (hue(*band) - hue(*other)).abs();
                let apart = apart.min(360. - apart);
                assert!(apart >= 25., "{band:06X} and {other:06X} look alike");
            }
            for state in [DANGER, SUCCESS, WARNING, FOCUS] {
                let apart = (hue(*band) - hue(state)).abs();
                let apart = apart.min(360. - apart);
                assert!(apart >= 25., "{band:06X} is {apart:.0}° from {state:06X}");
            }
        }
    }
}
