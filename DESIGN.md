---
name: Slipdock
description: A desktop app that dumps and loads MySQL and MariaDB databases, where every profile wears one flat colour band.
colors:
  ink: "#17161A"
  ground: "#F5F6F8"
  panel: "#FFFFFF"
  rule: "#D9DBE1"
  edge: "#8E8C98"
  dim: "#5E5C66"
  quiet: "#ECEDF1"
  quiet-hover: "#E2E3E9"
  rail: "#25222B"
  rail-hover: "#2D2934"
  rail-chosen: "#403A4C"
  rail-ink: "#ECEAF1"
  rail-dim: "#B1ACBD"
  ink-hover: "#34323A"
  ink-active: "#000000"
  danger: "#C92434"
  danger-hover: "#B01E2D"
  danger-active: "#98192A"
  success: "#10713C"
  warning: "#964F00"
  focus: "#2B6FE0"
  band-teal: "#0B8F8A"
  band-violet: "#7650D8"
  band-olive: "#857F00"
  band-magenta: "#C93A96"
  band-green: "#4F8A12"
  band-purple: "#9B3FB5"
typography:
  headline:
    fontFamily: "system-ui"
    fontSize: "30px"
    fontWeight: 700
  title:
    fontFamily: "system-ui"
    fontSize: "20px"
    fontWeight: 700
  body:
    fontFamily: "system-ui"
    fontSize: "16px"
    fontWeight: 400
  hint:
    fontFamily: "system-ui"
    fontSize: "14px"
    fontWeight: 400
  label:
    fontFamily: "system-ui"
    fontSize: "12px"
    fontWeight: 600
  mono:
    fontFamily: "ui-monospace, monospace"
    fontSize: "14px"
    fontWeight: 400
rounded:
  band: "1px"
  default: "4px"
spacing:
  "1": "4px"
  "2": "8px"
  "3": "12px"
  "4": "16px"
  "5": "20px"
  "6": "24px"
components:
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.panel}"
    rounded: "{rounded.default}"
  button-primary-hover:
    backgroundColor: "{colors.ink-hover}"
  button-primary-active:
    backgroundColor: "{colors.ink-active}"
  button-danger:
    backgroundColor: "{colors.danger}"
    textColor: "{colors.panel}"
    rounded: "{rounded.default}"
  button-danger-hover:
    backgroundColor: "{colors.danger-hover}"
  button-danger-active:
    backgroundColor: "{colors.danger-active}"
  button-outline:
    textColor: "{colors.ink}"
    rounded: "{rounded.default}"
  card:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.default}"
    padding: "16px"
  field-label:
    textColor: "{colors.dim}"
    typography: "{typography.hint}"
    width: "110px"
    height: "32px"
  option-row:
    backgroundColor: "{colors.panel}"
    rounded: "{rounded.default}"
    padding: "8px 12px"
  option-row-hover:
    backgroundColor: "{colors.quiet}"
  option-row-chosen:
    backgroundColor: "{colors.quiet}"
  nav-item:
    backgroundColor: "{colors.rail}"
    textColor: "{colors.rail-ink}"
    typography: "{typography.hint}"
    rounded: "{rounded.default}"
    padding: "6px 8px"
  nav-item-hover:
    backgroundColor: "{colors.rail-hover}"
  nav-item-chosen:
    backgroundColor: "{colors.rail-chosen}"
    textColor: "{colors.panel}"
  band-mark:
    rounded: "{rounded.band}"
    width: "10px"
    height: "36px"
  profile-strip:
    backgroundColor: "{colors.panel}"
    typography: "{typography.mono}"
    rounded: "{rounded.default}"
    padding: "8px 16px"
  mono-block:
    backgroundColor: "{colors.quiet}"
    textColor: "{colors.ink}"
    typography: "{typography.mono}"
    rounded: "{rounded.default}"
    padding: "12px"
  notice:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    typography: "{typography.hint}"
    rounded: "{rounded.default}"
    padding: "12px"
---

# Design System: Slipdock

## Overview

**Creative North Star: "Cartridge Label"**

Slipdock's app looks like the colour-banded labels on backup tape cartridges. Every profile (one environment, such as `staging` or `local`) wears one flat colour band, and the band follows that profile's dumps to every screen, so the user knows the environment before reading a name. Everything else is plain: a graphite navigation rail, a cool white ground, white panels with 1px lines and 4px corners, and ink for text and for the main action. The system refuses the grey database client with one blue accent.

The medium is a Rust desktop app drawn with GPUI through the `gpui-kit` crate (gpui-component 0.9.1). There is no HTML and no CSS. The tokens above are Rust constants in `crates/slipdock-app/src/theme.rs` (`theme::INK`, `theme::BANDS` and so on), not CSS variables. `theme::apply` hands the same colours to the component library as a theme file, so the library's buttons, inputs and dialogs come out in the palette of what the screens draw themselves. The pixel sizes in this file are GPUI's size steps on its 16px base. The theme is light only. macOS and Linux have equal standing.

The window cannot be screenshotted in an agent session. Layout is checked by `cargo test -p slipdock-app`, which lays the screens out in a headless window. The user checks the look by hand.

**Key Characteristics:**
- One flat colour band per profile, repeated on every dump of that profile.
- Ink, not a brand colour, for text and the main button.
- Red only for what destroys data and for what failed.
- Names of profiles, databases, tables, folders and paths in monospace, in the case they are written in.
- No shadows, no gradients, and no tint of a band or a state colour.
- Every row wraps; nothing runs out of a narrow window.

## Colors

A graphite and cool-white neutral set, three state colours, one focus blue, and six band colours that belong to profiles.

### Primary
- **Ink** (`ink`, `theme::INK`): all text on light grounds, and the fill of the main button ("Run dump", "Save"). The edge of a chosen row and of a chosen profile label is ink too.

### Secondary
- **The six bands** (`band-teal`, `band-violet`, `band-olive`, `band-magenta`, `band-green`, `band-purple`; `theme::BANDS`, in this order): six hues, because that is how many can be told apart at a glance while they stay clear of the state colours and of the focus blue. A band says whose a thing is. It is never a state.

### Tertiary
- **Danger** (`danger`, `theme::DANGER`): what destroys data ("Load…", "Delete profile", "Delete … selected") and a failed run or check. Its hover and active fills are `danger-hover` and `danger-active`.
- **Success** (`success`, `theme::SUCCESS`): a complete dump, a tool that works, the edge of the report of a run that succeeded.
- **Warning** (`warning`, `theme::WARNING`): a partial dump, a warning of a tool.
- **Focus** (`focus`, `theme::FOCUS`): the ring of the focused control. Selected text is the same blue let through (alpha `40`). The screens use this blue for nothing else.

### Neutral
- **Ground** (`ground`, `theme::GROUND`): behind the page.
- **Panel** (`panel`, `theme::PANEL`): cards, inputs, dialogs, and the text on ink and danger buttons.
- **Rule** (`rule`, `theme::RULE`): the 1px lines around cards and between rows.
- **Edge** (`edge`, `theme::EDGE`): the edge of an input and of an outlined button. It is darker than Rule, because a control has to stand out.
- **Dim** (`dim`, `theme::DIM`): hints, field labels, small titles, the neutral notice, and the outline that stands where a band is missing.
- **Quiet** (`quiet`, `theme::QUIET`): the one neutral fill: a hovered row, the chosen row, the header of a dump folder group, a command or log block, a quoted name. `quiet-hover` is the hover fill of the library's secondary controls.
- **Rail** (`rail`, `rail-hover`, `rail-chosen`, `rail-ink`, `rail-dim`): the navigation. The hover fill lies between the rail and the chosen fill. `rail-dim` is the small text on the rail, because Dim cannot be read there.
- **Ink hover and active** (`ink-hover`, `ink-active`): the main button under the pointer and pressed.

### Named Rules

**The Band Rule.** A profile's band is a number stored in `settings.toml` (`Settings::bands` in `crates/slipdock-core/src/settings.rs`), so it does not change between starts. A new profile gets the lowest band that no profile has; with more than six profiles, the band that the fewest profiles have, so a seventh profile shares a band (`Settings::assign_bands`; the test `a_profile_keeps_its_band` holds it). A dump takes the band of the profile in its folder name (`dumps::profile_of`), not of the folder it lies in.

**The Solid Band Rule.** A band is always a solid block of its flat colour. It is never tinted, and never drawn as an outline. An outlined block in Dim means there is no band: the dump of a deleted profile, or a folder named by hand.

**The Readable Pair Rule.** Text has a contrast of at least 4.5:1 on its ground: on Panel, Ground and Quiet, on the rail in its three fills, and on the ink and danger buttons. Edge has at least 3:1 on Panel. Every band has at least 3:1 on Panel and on Quiet. The test `text_can_be_read_on_its_ground` in `theme.rs` fails when a new colour breaks this.

**The Band Is Not A State Rule.** Every band is at least 25° of hue from Danger, Success, Warning and Focus. Any two bands differ by at least 25° of hue. The test `a_band_is_not_taken_for_a_state` in `theme.rs` holds both.

**The Red Rule.** Red is for what destroys data and for what failed. Nothing else is red.

## Typography

**Display Font:** the platform UI font
**Body Font:** the platform UI font
**Label/Mono Font:** the platform monospace font

**Character:** The app takes both fonts from the platform, so it reads as native on macOS and on Linux. The contrast in the type is between the UI font for what Slipdock says and monospace for what the user or the server named.

The `fontFamily` values in the frontmatter (`system-ui`, `ui-monospace, monospace`) stand for the platform fonts. The code never names a family: it uses the component library's `font_family` and `mono_font_family`, which resolve to fonts that are installed. GPUI panics on a family that is missing.

### Hierarchy
- **Headline** (bold 700, 30px): the name of a screen, once at its top (`widgets::header`).
- **Title** (bold 700, 20px): the name "Slipdock" on the rail. The name of a tool on the Tools screen is one step down (semibold 600, 18px).
- **Body** (regular 400, 16px): the lines of a summary and a report, the title of a dump row (medium 500). The verdict lines of a report are semibold 600.
- **Hint** (regular 400, 14px): explanations and field labels, in Dim. The navigation items and the text of a notice have this size in the colour of their ground's text.
- **Label** (semibold 600, 12px, Dim, uppercase): the small title of a card or of a group of fields (`widgets::subheading`). It is the only title of its group, never a line above a larger heading. The line under "Slipdock" on the rail is 12px regular in `rail-dim`.
- **Mono** (regular 400, 14px): names of profiles, databases, tables, folders and paths; commands and logs. The folder name under a dump's date is 12px. The chosen profile's name is bold 700.

### Named Rules

**The Written Case Rule.** A name of a profile, database, table, folder or path is in monospace and in the case it is written in. Profile names are file names, and on Linux `Staging` and `staging` are two profiles, so a name gets no capitals.

**The Platform Font Rule.** Never name a font family. Use the theme's UI font and its monospace font.

## Layout

The window opens at 1040 by 760 and does not shrink below 520 by 420. The navigation rail is 210px wide at the left. The screen beside it scrolls on its own, keeps a width of at most 980px, sits in the middle of a wider window, and has 24px of padding.

On macOS the window has no title bar of its own: it is drawn up to its top edge, and the three window buttons stand on the rail at 20px from the left and 13px from the top, in line with the navigation icons. The top 40px are the title strip (`shell::TITLE_STRIP`). The system owns it: a drag there moves the window and a double click zooms it, so nothing that can be clicked is drawn in it. The rail starts its content below the strip, and beside the rail the strip is Ground, with the page scrolling below it and keeping 8px above its header. A full-screen window has no strip. On Linux the desktop draws the title bar and the app leaves it alone (`platform::draws_under_title_bar`).

Below a window width of 760px the rail moves to the top as a wrapping row, drops its second line and its shortcut hints, and the page padding becomes 16px. This is the only switch of layout. Everything else adapts by wrapping: a row that may hold more than fits wraps, and a flexible child may get narrower than its content so that a long path or host name wraps.

A screen is a column with 20px between its sections: the header, then cards. A card has 16px of padding and 12px between its parts. A form row has a label column of 110px that all cards share, then the control, which wants 220px and takes what is left; in a narrow window the control moves under its label. Rows of buttons and choices have 8px between items. The spacing steps in use are 4, 8, 12, 16, 20 and 24px.

A list that can be long scrolls in its own bordered box (the databases at most 180px high, the tables at most 220px). The empty profile label is drawn before a profile is chosen, so that choosing one does not move the form.

## Elevation & Depth

The system is flat. The theme file turns the component library's shadows off (`"shadow": false`), and nothing the screens draw has a shadow or a gradient. Depth is said by three things: the white Panel on the Ground, a 1px line in Rule, and the Quiet fill for what is under the pointer or chosen.

### Named Rules

**The Flat Rule.** No shadows and no gradients, on any surface, in any state.

**The No Wash Rule.** In what the screens draw themselves, a band or a state colour is never thinned into a background (the component library keeps its own hover and disabled fills). A state is said by a 1px edge in its colour and an icon in its colour on a white panel; the verdict of a run is said by the edge of its card and the mark on its first line. Quiet is the only fill the screens draw, and it is neutral. The one colour let through is the focus blue behind selected text.

## Shapes

Corners are 4px (`theme::RADIUS`): cards, rows, navigation items, notices, command blocks, and the library's buttons, inputs and dialogs (the theme file sets `radius` and `radius.lg` to 4). Three shapes differ, each for a reason. A band block has 1px corners, so it reads as a printed block and not as a pill. The mark of a choice in a list is a circle. The size bars are square.

Lines are 1px. A card, a row and a list box have a line in Rule all round; rows inside a dump folder group are parted by a line above each. Chosen is said by the line turning ink, not by a thicker line.

## Components

### Buttons
The library's buttons, in four forms.
- **Shape:** 4px corners.
- **Primary:** ink fill, white text: the action a form is for ("Run dump", "Save", "Create", "New profile"). A chosen choice button (`widgets::choice`) is primary too.
- **Danger:** red fill, white text: "Load…" and "Start load", "Delete profile", "Delete … selected".
- **Outline:** a line in Edge, ink text: every second action ("Show command", "Test connection", "List tables", "Cancel" in a dialog).
- **Ghost:** no line: the icon buttons on a dump row, and "Show log".
- **Hover / Active:** primary goes to `ink-hover`, then `ink-active`; danger to `danger-hover`, then `danger-active`.

### Tags
- **Style:** the library's tag, outlined and small (`widgets::tag`), in the colour of its tone: success ("ready"), danger ("not found"), warning ("partial", "TLS check off"), and a neutral one ("keychain", "unsaved").

### Cards / Containers
- **Corner Style:** 4px.
- **Background:** Panel.
- **Shadow Strategy:** none (see Elevation & Depth).
- **Border:** 1px in Rule. The report of a run has its edge in Success or Danger.
- **Internal Padding:** 16px, with 12px between parts. A card may open with a Label title (`widgets::section`).
- **Dump folder group:** a card without padding. Its header is on Quiet (12px by 16px), and each dump row under it has a line above.

### Inputs / Fields
- **Style:** the library's input: Panel fill, a 1px line in Edge, 4px corners. An input that takes a name or a path (the database, the target database, the output folder) is set in the monospace font.
- **Focus:** the library's ring in Focus.
- **Field row** (`widgets::field`): a label in Hint type, 110px wide and as high as an input (32px), then the control.

### Navigation
- **Rail** (`shell.rs`): 210px, the Rail colour, text in `rail-ink`. Each of the five screens is a row with a small icon from the library's set and its name in Hint type (6px by 8px padding, 4px corners, 4px between rows).
- **States:** `rail-hover` under the pointer; the chosen screen has the `rail-chosen` fill, white text and semibold weight.
- **Compact:** below 760px the rail is a wrapping row at the top. With a title strip, the name stands in the strip after the window buttons (from 92px) and the screens are a row below it.

### Band mark (signature)
`widgets::band_mark`. One block of the band colour at the start of a row, 10px wide and 36px high (as high as two lines of text), with 1px corners. A partial dump's band is broken in three blocks of 10px with 3px between them, so the shape says "partial" as well as the warning tag does. Without a band the block is an outline in Dim.

### Profile label and strip (signature)
- **Profile choice** (`widgets::profile_choice`): an outlined button that holds a band block 16px high and the profile's name in monospace. The band of the chosen one is 28px wide instead of 10px, its name is bold, and its edge is ink.
- **Profile strip** (`widgets::profile_strip`): the label that heads a form. A band 44px wide across the start, as high as the strip (at least 40px), then the name in bold monospace and the server in Dim at the end of the line. Without a profile it says "No profile chosen" in Dim, and a line marks the place of the band.

### Dump row and size bars
- **Dump summary** (`widgets::dump_summary`): the band mark, then the date the dump was taken (medium weight), a "partial" tag when it is one, and the folder name under it in 12px monospace in Dim. The delete confirmation lists its dumps with this same row, in a box with a line in Rule (12px padding, 8px between rows).
- **Size bars** (`widgets::dump_stats`): "65 tables · 15M" in Hint type, then five bars 3px wide that rise from 4px to 12px, with 2px between them. They are filled in Dim up to the dump's step (under 10 MB, under 100 MB, under 1 GB, under 10 GB, more) and in Rule above it.

### Option row
`widgets::option_row`. A whole row that can be clicked: Panel fill, a line in Rule, 8px by 12px padding. Under the pointer it is on Quiet. Chosen, it is on Quiet with an ink line. Its mark (`widgets::option_mark`) is a 14px circle: a 1px line in Dim, a 4px ink line when chosen, a line in Rule when the row is off.

### Notice
`widgets::notice`. A message on Panel with a 1px edge and a small icon in the colour of its tone, 12px padding, text in Hint type. The neutral notice uses Dim.

### Command and log blocks
- **Mono block** (`widgets::mono`): a command on Quiet with a line in Rule, 12px padding, 14px monospace, one element per line so that breaks and leading spaces are kept.
- **Quoted name:** a name the user has to type or confirm is set off on Quiet in bold or semibold monospace.
- **Log box** (`log_view.rs`): the library's read-only editor with a copy button above it. Scroll bars are always on show, so the box says at a glance that there is more.

### Report lines
`widgets::lines` draws the report that the core builds: a label column of 90px in Dim, then the value; a verdict line with a small icon in Success, Danger or Warning; quoted tool output in Dim.

## Do's and Don'ts

### Do:
- **Do** put the profile's band beside every thing that belongs to a profile: its label, its row, its dumps, the confirmation of a load or a delete.
- **Do** take a colour from a constant in `theme.rs` or from the theme (`cx.theme()`). A new colour goes in `theme.rs`, in the theme file, and in the pairs of `text_can_be_read_on_its_ground`.
- **Do** say a state with a 1px edge and an icon in Success, Warning or Danger on a white panel.
- **Do** set names of profiles, databases, tables, folders and paths in the theme's monospace font, in the case they are written in.
- **Do** give every row that may be too wide `flex_wrap()` and every flexible child `min_w_0()`, and check the layout with `cargo test -p slipdock-app`.
- **Do** use `theme::RADIUS` (4px) for every corner except a band block (1px).
- **Do** say a second thing with shape when colour says the first: a partial dump's band is broken in three.

### Don't:
- **Don't** draw a band as an outline, tint it, or use a band colour for a state. An outlined block means there is no band.
- **Don't** add a band colour within 25° of hue of Danger, Success, Warning, Focus or another band.
- **Don't** use red for anything that does not destroy data and did not fail.
- **Don't** add a shadow, a gradient, or a tinted background of a band or state colour to anything a screen draws.
- **Don't** name a font family. GPUI panics on one that is missing.
- **Don't** write a profile, database or table name in capitals or in title case.
- **Don't** look for CSS variables or add a dark theme by guessing. The tokens are Rust constants in `theme.rs`, and the app is light only.
- **Don't** claim the window looks right from an agent session. The tests check layout; the user checks the look.
