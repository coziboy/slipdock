//! The colour field of a profile: a button that shows the band, and under
//! it a colour wheel.
//!
//! What opens under the button is, first, sets of colours to take as they
//! are: the bands the app gives out itself, then a grid of hues from light
//! to dark. Under them is the hex field, which takes a colour that is typed
//! or pasted. "Custom colour" opens a colour wheel beside them, in the
//! same popover: the hue goes around it and the strength of the colour from
//! the middle (white) to the rim (the full colour); the slider under it
//! makes the whole wheel darker.
//!
//! GPUI concept: **canvas**. The wheel is not built from elements; it is
//! painted, cell by cell, in the `paint` step of a `canvas` element, where
//! the place of the element in the window is known. The mouse listeners are
//! set there too, for the same reason: they need that place to turn a
//! pointer position into a place on the wheel.

use std::f32::consts::TAU;

use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{ActiveTheme, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::settings::{band_hex, parse_band};

use crate::theme;
use crate::widgets;

/// The side of the square the wheel is painted in.
const SIDE: f32 = 208.;
/// The wheel is painted in square cells of this side.
const CELL: f32 = 2.;
/// The cells leave steps at the rim; a ring of the ground this wide, laid
/// over them, makes the rim round.
const RIM: f32 = 3.;

/// The hues of the grid of colours, in degrees, a column each. A column of
/// greys comes after them.
const HUES: [f32; 9] = [0., 24., 45., 90., 140., 175., 210., 260., 315.];
/// The rows of the grid, from light to dark: how strong and how light the
/// colours of a row are, then how light its grey is.
const SHADES: [(f32, f32, f32); 5] = [
    (0.30, 1.00, 0.92),
    (0.55, 0.95, 0.75),
    (0.80, 0.85, 0.55),
    (0.90, 0.65, 0.35),
    (0.90, 0.45, 0.15),
];

/// The grid of colours to take as they are, row by row.
pub(crate) fn palette() -> Vec<Vec<u32>> {
    SHADES
        .iter()
        .map(|(saturation, brightness, grey)| {
            HUES.iter()
                .map(|hue| hsv_to_rgb(hue / 360., *saturation, *brightness))
                .chain([hsv_to_rgb(0., 0., *grey)])
                .collect()
        })
        .collect()
}

/// A colour was chosen: `0xRRGGBB`.
pub struct BandPicked(pub u32);

pub struct BandPicker {
    /// The colour in the field. `None` until a profile is in the form.
    pub(crate) rgb: Option<u32>,
    /// Where the colour is on the wheel: the hue as a part of the turn,
    /// from red, and how far from the middle. Kept beside `rgb` because a
    /// grey has no hue and black has neither: the mark stays where it was.
    hue: f32,
    saturation: f32,
    /// How light the wheel is: 1 is the full colours, 0 is black.
    brightness: f32,
    pub(crate) hex: Entity<InputState>,
    slider: Entity<SliderState>,
    /// What is under the button is on show.
    pub(crate) open: bool,
    /// The wheel is on show beside the sets of colours.
    pub(crate) wheel: bool,
    /// The pointer went down on the wheel and has not come up.
    dragging: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BandPicked> for BandPicker {}

impl BandPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hex = cx.new(|cx| InputState::new(window, cx).placeholder("#RRGGBB"));
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(100.)
        });
        let subscriptions = vec![
            cx.subscribe_in(&hex, window, |this, hex, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let typed = parse_band(&hex.read(cx).value());
                    this.typed(typed, window, cx);
                }
            }),
            cx.subscribe_in(
                &slider,
                window,
                |this, _, event: &SliderEvent, window, cx| {
                    if let SliderEvent::Change(SliderValue::Single(value)) = event {
                        this.pick_brightness(value / 100., window, cx);
                    }
                },
            ),
        ];
        BandPicker {
            rgb: None,
            hue: 0.,
            saturation: 0.,
            brightness: 1.,
            hex,
            slider,
            open: false,
            wheel: false,
            dragging: false,
            _subscriptions: subscriptions,
        }
    }

    /// Shows a colour (or none) without it counting as a choice: nothing is
    /// emitted.
    pub fn set_color(&mut self, rgb: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        self.rgb = rgb;
        self.dragging = false;
        if let Some(rgb) = rgb {
            self.place(rgb);
        }
        self.show(true, window, cx);
        cx.notify();
    }

    /// Chooses a colour as a whole: a click on a swatch.
    pub(crate) fn pick(&mut self, rgb: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.place(rgb);
        self.chosen(rgb, true, window, cx);
    }

    /// Chooses the colour at a place on the wheel. `x` and `y` are from the
    /// middle, in parts of the radius, `y` downwards as on the screen. A
    /// place outside the wheel counts as the place on the rim towards it,
    /// so the pointer can leave the wheel during a drag.
    pub(crate) fn pick_at(&mut self, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        (self.hue, self.saturation) = wheel_place(x, y).unwrap_or((self.hue, 0.));
        let rgb = hsv_to_rgb(self.hue, self.saturation, self.brightness);
        self.chosen(rgb, true, window, cx);
    }

    /// Chooses how light the colour is, from 0 to 1.
    pub(crate) fn pick_brightness(
        &mut self,
        brightness: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.brightness = brightness.clamp(0., 1.);
        let rgb = hsv_to_rgb(self.hue, self.saturation, self.brightness);
        self.chosen(rgb, false, window, cx);
    }

    /// What is in the hex field changed. A whole colour is a choice; the
    /// text is left alone, because someone is typing in it.
    fn typed(&mut self, typed: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        // The same colour: the field was filled from here, not typed in.
        let Some(rgb) = typed.filter(|typed| Some(*typed) != self.rgb) else {
            return;
        };
        self.place(rgb);
        self.rgb = Some(rgb);
        let brightness = self.brightness * 100.;
        // A slider that is set from code says nothing, so this ends here.
        self.slider
            .update(cx, |slider, cx| slider.set_value(brightness, window, cx));
        cx.emit(BandPicked(rgb));
        cx.notify();
    }

    /// Puts the mark of the wheel and the slider where `rgb` is.
    fn place(&mut self, rgb: u32) {
        let (hue, saturation, brightness) = rgb_to_hsv(rgb);
        self.brightness = brightness;
        // Black is at every place of the wheel, a grey at every hue.
        if brightness > 0. {
            self.saturation = saturation;
            if saturation > 0. {
                self.hue = hue;
            }
        }
    }

    fn chosen(&mut self, rgb: u32, slider: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.rgb = Some(rgb);
        self.show(slider, window, cx);
        cx.emit(BandPicked(rgb));
        cx.notify();
    }

    /// Makes the hex field, and the slider when it is not the one being
    /// moved, say what the colour is. Neither counts as a choice.
    fn show(&mut self, slider: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.rgb.map(band_hex).unwrap_or_default();
        if self.hex.read(cx).value() != text {
            self.hex
                .update(cx, |hex, cx| hex.set_value(text, window, cx));
        }
        if slider {
            let brightness = self.brightness * 100.;
            self.slider
                .update(cx, |slider, cx| slider.set_value(brightness, window, cx));
        }
    }

    /// The pointer went down or moved at `position`, with the wheel in
    /// `bounds`.
    fn pointer(
        &mut self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = from_middle(position, bounds);
        self.pick_at(x, y, window, cx);
    }
}

/// A pointer position as parts of the wheel's radius from its middle.
fn from_middle(position: Point<Pixels>, bounds: Bounds<Pixels>) -> (f32, f32) {
    let radius = radius(bounds);
    let middle = bounds.center();
    (
        (position.x - middle.x).as_f32() / radius,
        (position.y - middle.y).as_f32() / radius,
    )
}

/// The radius of the wheel painted in `bounds`.
fn radius(bounds: Bounds<Pixels>) -> f32 {
    (bounds.size.width.min(bounds.size.height).as_f32() / 2. - RIM).max(1.)
}

/// The hue and the saturation at a place on the wheel, given from the
/// middle in parts of the radius. Red is to the right and the hues go
/// round against the clock. `None` in the very middle, which has no hue.
fn wheel_place(x: f32, y: f32) -> Option<(f32, f32)> {
    let distance = x.hypot(y);
    if distance <= f32::EPSILON {
        return None;
    }
    let hue = ((-y).atan2(x) / TAU).rem_euclid(1.);
    Some((hue, distance.min(1.)))
}

/// A hue (a part of the turn), a saturation and a brightness, each from 0
/// to 1, as `0xRRGGBB`.
pub(crate) fn hsv_to_rgb(hue: f32, saturation: f32, brightness: f32) -> u32 {
    let sector = hue.rem_euclid(1.) * 6.;
    let chroma = brightness * saturation;
    let second = chroma * (1. - (sector % 2. - 1.).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, second, 0.),
        1 => (second, chroma, 0.),
        2 => (0., chroma, second),
        3 => (0., second, chroma),
        4 => (second, 0., chroma),
        _ => (chroma, 0., second),
    };
    let lift = brightness - chroma;
    let channel = |value: f32| ((value + lift).clamp(0., 1.) * 255.).round() as u32;
    channel(r) << 16 | channel(g) << 8 | channel(b)
}

/// The other way round. A grey comes back with hue 0.
pub(crate) fn rgb_to_hsv(rgb: u32) -> (f32, f32, f32) {
    let [r, g, b] = [16, 8, 0].map(|shift| ((rgb >> shift) & 0xFF) as f32 / 255.);
    let max = r.max(g).max(b);
    let spread = max - r.min(g).min(b);
    if spread <= 0. {
        return (0., 0., max);
    }
    let sector = if max == r {
        ((g - b) / spread).rem_euclid(6.)
    } else if max == g {
        (b - r) / spread + 2.
    } else {
        (r - g) / spread + 4.
    };
    (sector / 6., spread / max, max)
}

/// Paints the wheel at `brightness`, its round rim, and the mark at the
/// chosen place.
fn paint_wheel(
    bounds: Bounds<Pixels>,
    brightness: f32,
    mark: Option<(f32, f32, u32)>,
    ground: Hsla,
    window: &mut Window,
) {
    let radius = radius(bounds);
    let middle = bounds.center();
    let cells = (SIDE / CELL).ceil() as i32;
    let cell = size(px(CELL), px(CELL));
    for row in 0..cells {
        for column in 0..cells {
            // The middle of the cell, from the middle of the wheel.
            let x = (column as f32 + 0.5) * CELL - SIDE / 2.;
            let y = (row as f32 + 0.5) * CELL - SIDE / 2.;
            // A cell that is over the rim by less than the ring is wide is
            // painted: the ring covers what sticks out.
            if x.hypot(y) > radius + CELL * 0.75 {
                continue;
            }
            let (hue, saturation) = wheel_place(x / radius, y / radius).unwrap_or((0., 0.));
            let origin = point(
                middle.x + px(x - CELL / 2.),
                middle.y + px(y - CELL / 2.),
            );
            window.paint_quad(fill(
                Bounds::new(origin, cell),
                theme::color(hsv_to_rgb(hue, saturation, brightness)),
            ));
        }
    }
    let circle = |middle: Point<Pixels>, radius: f32| {
        Bounds::centered_at(middle, size(px(radius * 2.), px(radius * 2.)))
    };
    window.paint_quad(quad(
        circle(middle, radius + RIM),
        radius + RIM,
        transparent_black(),
        RIM,
        ground,
        BorderStyle::default(),
    ));
    if let Some((hue, saturation, rgb)) = mark {
        let angle = hue * TAU;
        let at = point(
            middle.x + px(angle.cos() * saturation * radius),
            middle.y - px(angle.sin() * saturation * radius),
        );
        // A dark line around a white one: the mark shows on every colour.
        window.paint_quad(quad(
            circle(at, 8.),
            8.,
            transparent_black(),
            1.,
            theme::color(theme::INK),
            BorderStyle::default(),
        ));
        window.paint_quad(quad(
            circle(at, 7.),
            7.,
            theme::color(rgb),
            2.,
            theme::color(theme::PANEL),
            BorderStyle::default(),
        ));
    }
}

impl Render for BandPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picker = cx.entity();
        let mono = cx.theme().mono_font_family.clone();
        let border = cx.theme().border;
        let ground = cx.theme().popover;
        let swatch = |rgb: Option<u32>| {
            div()
                .size_4()
                .flex_shrink_0()
                .rounded(px(2.))
                .border_1()
                .border_color(border)
                .when_some(rgb, |this, rgb| this.bg(theme::color(rgb)))
        };
        let trigger = Button::new("band")
            .outline()
            .accessibility_label("Choose the colour of this profile")
            .tooltip("Choose the colour of this profile")
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(swatch(self.rgb))
                    .child(
                        div()
                            .font_family(mono)
                            .child(self.rgb.map(band_hex).unwrap_or_else(|| "No colour".into())),
                    ),
            );
        let brightness = self.brightness;
        let mark = self
            .rgb
            .map(|rgb| (self.hue, self.saturation, rgb));
        let wheel = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_wheel(bounds, brightness, mark, ground, window);
                // A press on the wheel starts a drag; the drag goes on
                // outside it, and ends when the button comes up anywhere.
                let down = picker.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    let (x, y) = from_middle(event.position, bounds);
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || x.hypot(y) > 1. + RIM / radius(bounds)
                    {
                        return;
                    }
                    down.update(cx, |picker, cx| {
                        picker.dragging = true;
                        picker.pointer(event.position, bounds, window, cx);
                    });
                });
                let moved = picker.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || !moved.read(cx).dragging {
                        return;
                    }
                    moved.update(cx, |picker, cx| {
                        if event.dragging() {
                            picker.pointer(event.position, bounds, window, cx);
                        } else {
                            // The button came up outside the window.
                            picker.dragging = false;
                        }
                    });
                });
                let up = picker.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    if up.read(cx).dragging {
                        up.update(cx, |picker, _| picker.dragging = false);
                    }
                });
            },
        )
        .size(px(SIDE));

        // A colour to take as it is. The one that is the band has a line
        // around it.
        let chosen = self.rgb;
        let ink = cx.theme().foreground;
        let pick = |id: ElementId, rgb: u32, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(18.))
                .flex_shrink_0()
                .rounded(px(2.))
                .bg(theme::color(rgb))
                .border_1()
                .border_color(if chosen == Some(rgb) { ink } else { border })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.pick(rgb, window, cx)))
        };
        let colours = v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(widgets::hint("Slipdock", cx))
                    .child(h_flex().gap(px(3.)).children(
                        theme::BANDS.iter().enumerate().map(|(index, band)| {
                            pick(("slipdock-band", index).into(), *band, cx)
                        }),
                    )),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(widgets::hint("More colours", cx))
                    .child(v_flex().gap(px(3.)).children(
                        palette().into_iter().enumerate().map(|(row, colours)| {
                            h_flex().gap(px(3.)).children(
                                colours.into_iter().enumerate().map(|(column, rgb)| {
                                    let place = row * 16 + column;
                                    pick(("palette", place).into(), rgb, cx)
                                        // Named, so that a test can find it.
                                        .debug_selector(|| format!("band-colour-{place}"))
                                }),
                            )
                        }),
                    )),
            );
        let on_wheel = v_flex()
            .gap_3()
            // Named, so that a test can find where it is.
            .child(div().debug_selector(|| "band-wheel".into()).child(wheel))
            .child(
                v_flex()
                    .gap_1()
                    .child(widgets::hint("Brightness", cx))
                    .child(
                        Slider::new(&self.slider).accessibility_label("Brightness of the colour"),
                    ),
            );
        let with_wheel = self.wheel;
        let wheel_button = Button::new("band-wheel-button")
            .outline()
            .small()
            .w_full()
            .label(if with_wheel {
                "Hide custom colour"
            } else {
                "Custom colour"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.wheel = !this.wheel;
                this.dragging = false;
                cx.notify();
            }));

        let open = cx.entity();
        Popover::new("band-picker")
            .open(self.open)
            .on_open_change(move |is_open: &bool, _, cx| {
                open.update(cx, |picker, cx| {
                    picker.open = *is_open;
                    picker.dragging = false;
                    cx.notify();
                });
            })
            .trigger(trigger)
            // The popover drops what is in it while it is closed.
            .when(self.open, |this| {
                this.child(
                    h_flex()
                        .items_start()
                        .gap_4()
                        .child(
                            v_flex()
                                .w(px(SIDE))
                                .gap_3()
                                .child(colours)
                                .child(Input::new(&self.hex).small())
                                .child(wheel_button),
                        )
                        .when(with_wheel, |this| this.child(on_wheel.w(px(SIDE)))),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{hsv_to_rgb, palette, rgb_to_hsv, wheel_place};

    #[test]
    fn no_colour_is_in_the_grid_twice() {
        let colours: Vec<u32> = palette().into_iter().flatten().collect();
        assert_eq!(colours.len(), 50);
        for (index, colour) in colours.iter().enumerate() {
            assert!(!colours[..index].contains(colour), "{colour:06X}");
        }
    }

    #[test]
    fn a_colour_comes_back_from_the_wheel_as_it_went() {
        // Every channel value in a few mixes, and the corners of the cube.
        for step in 0..=255u32 {
            for rgb in [
                step << 16 | (255 - step) << 8 | step / 2,
                step << 8 | (step * 7 % 256),
                step * 0x010101,
                (step * 37 % 256) << 16 | (step * 101 % 256) << 8 | (step * 13 % 256),
            ] {
                let (hue, saturation, brightness) = rgb_to_hsv(rgb);
                assert_eq!(
                    hsv_to_rgb(hue, saturation, brightness),
                    rgb,
                    "{rgb:06X} by way of {hue} {saturation} {brightness}"
                );
            }
        }
    }

    #[test]
    fn the_wheel_has_red_to_the_right_and_white_in_the_middle() {
        let at = |x: f32, y: f32| {
            let (hue, saturation) = wheel_place(x, y).unwrap_or((0., 0.));
            hsv_to_rgb(hue, saturation, 1.)
        };
        assert_eq!(at(1., 0.), 0xFF0000);
        assert_eq!(at(0., 0.), 0xFFFFFF);
        // Up is a quarter of the turn on, between yellow and green; left is
        // the colour across from red.
        assert_eq!(at(0., -1.), 0x80FF00);
        assert_eq!(at(-1., 0.), 0x00FFFF);
        // Outside the wheel counts as the rim.
        assert_eq!(at(3., 0.), 0xFF0000);
        // Half way out the colour is half as strong.
        assert_eq!(at(0.5, 0.), 0xFF8080);
    }
}
