---
version: 1
slug: "crates-slipdock-app-src"
primary_target: "crates/slipdock-app/src"
related_targets: []
---

# Slipdock desktop app (all five screens)

Scope: the GPUI desktop app in `crates/slipdock-app/src`. Mode: Operate.

Audience and job: developers and database administrators who dump, load and keep MySQL/MariaDB dumps. Tasks: run a dump, run a load behind the load guard, manage dumps, edit profiles, check tools. Constraints: GPUI, no HTML; macOS and Linux equal; layout, navigation, copy and behaviour stay as they were; report text comes from the core.

Chosen direction: Cartridge Label (the colour-banded labels on backup tape cartridges). Memorable moment: the profile's colour band meets the user again on every dump of that profile.

Unresolved: dark mode (the app is light-only); a colour the user picks per profile (a core and CLI feature, not built).

## Direction contract

THESIS: Every profile wears one flat colour band, and the band follows its dumps to every screen, so the environment is known before a name is read. Refuses the grey database client with one blue accent.

OWN-WORLD: Graphite rail (#25222B), cool white ground (#F5F6F8), white panels with 1px rules and 4px corners, ink (#17161A) for text and the primary action. Six flat band colours, never tinted. Names of profiles, databases, tables, folders and paths in monospace, in the case they are written in (profile names are file names, and on Linux `Staging` and `staging` are two profiles, so no capitals). No washes, gradients or shadows in what the screens draw themselves (the component library keeps its own hover and disabled fills).

STORY: The user picks a label, sees its band head the form, and trusts that a dump row with the same band came from that environment. Red is kept for what destroys data and for what failed.

FIRST VIEWPORT: Rail 210px at the left with the five screens. Dump form on the ground: title, then the profile label strip across the form width (empty until a profile is chosen, so the form does not jump), then Source, Tables, Output panels with one shared 110px label column, then Run dump (ink) and Show command.

FORM: Cartridge Label, candidate 5 of 7 on the grounded list. seed:7ed52c8ca4edc6e6

SIGNATURE: the band. Solid for a complete dump, broken into segments for a partial one; a five-step bar for dump size.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
