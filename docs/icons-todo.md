# Icons to do

The 18 page icons (sidebar, page titles, Settings and Colors buttons) are
done: PNGs in `assets/icons/dark` and `assets/icons/light`, drawn by
`src/widgets/icons.rs`. This file lists every other icon the app shows, so we
can decide which ones to redraw in the same building-block style (sockets,
modules, bricks and studs; grey ink with one green piece).

**Redo?** is a proposal: **Yes** = a real pictogram people look at, worth the
new style; **Maybe** = small or decorative, decide case by case; **No** = a
status dot, a control, a separator or an egui built-in. Change it as you
decide.

Paths are relative to the repository root; line numbers are from 2026-11-19.

## Status (2026-11-19)

Done: 28 glyphs in `assets/glyphs/{dark,light}` (made from `sheet-dark.png` with
`slice_glyphs.py`), drawn by `icons::draw_glyph`. They replace the whole Glyph
set (Power & Freq tree, search fields, Disk Space "Parent"), the Performance
device icons, the requirement icons, the UAC shield, Refresh and the tick.
Still drawn by hand: Record / Stop (kept as the universal symbols), and every
item of sections 2-4 below. `record` and `stop` glyphs exist if we want them later.

## 1. Pictograms (proposal: redo)

| Icon | Where it shows | Drawn by | Redo? |
|---|---|---|---|
| **Glyph set**, 20 line icons on a 24 grid: Thermometer, Gauge, Clock, Bolt, Wave, Sliders, ShieldCheck, Trend, Info, Fan, Monitor, Chip, GraphicsCard, Hexagon, Board, MemoryStick, Drive, Battery, Search, Up | Power & Freq sensor tree (group and device rows); Disk Space "Parent" button (Up); search fields (Search) | `src/widgets.rs:518` `glyph()`, enum at `:493`; painter lines, 16 px in trees, 15 px in buttons | Yes |
| Performance device icons: CPU (chip), Memory (stick), Disk (ring and dot), Network (monitor), GPU (screen), Power (bolt) | Performance device list | `src/page_perf.rs:1025` `paint_icon()`, enum `Icon` at `:118`; about 15 px, series colour | Yes (could reuse the Glyph set) |
| Requirement icons: Admin (shield), Internet (globe), Program (terminal) | Settings › Add-ons list rows and the add-on's detail | `src/widgets.rs:457` `need_icon()`; about 13 px | Yes |
| UAC shield | Every `admin_button` (Drivers ×7, Power & Freq "Restart as administrator"), Package Manager batch items that need admin | `src/widgets/parts.rs:467` `shield()`, placed by `admin_button()` at `:82`; 14×12 | Yes (one shield with the Admin requirement icon) |
| Refresh (circular arrow) | "Refresh" button in the header of every page | `src/widgets.rs:203` `refresh_button()`; 12 px | Yes |
| Search magnifier | Search fields: Connections, Package Manager catalog, Power & Freq "Find a sensor" | `Glyph::Search` via `src/widgets.rs:929` `search_field()`; 15 px | Yes |
| Record dot / Stop square | Flight Recorder "Record" and "Stop" buttons | `src/widgets/parts.rs:1311` `marked_button()` (`ButtonMark`); 10 px | Maybe (they are the universal symbols) |
| Check mark (tick) | Status bands; Add-on requirements met; Drivers "Up to date" / "Nothing newer"; Installed Apps "Up to date on winget"; Package Manager "Installed" | `src/widgets.rs:449` `tick()`; about 12×9, green | Maybe |
| Wireframe globe with routes | Connections side panel (an illustration, not a small icon) | `src/addons/connections/globe.rs:101` `draw()` | No (it is a visualisation) |

## 2. Small marks and controls (proposal: keep)

| Icon | Where it shows | Drawn by | Redo? |
|---|---|---|---|
| Status band marks: Busy (spinner), Dot, Tick | Status band of most pages | `src/widgets/parts.rs:209` enum, drawn at `:291-303` | No |
| Status bar health dot | Status bar, far left | `src/app.rs:708` | No |
| Inline coloured dot | Benchmarks figure cards, Power & Freq sources, Package Manager notes | `src/widgets.rs:484` `dot()` | No |
| Run-state dot | Benchmarks run band | `src/addons/benchmarks.rs:681` | No |
| Outcome dots | Package Manager headlines | `src/addons/package_manager/page.rs:140` | No |
| Consumption intensity dot | Settings › Add-ons detail | `src/page_settings.rs:357` | No |
| Category swatches | Disk Space file-type legend and rows | `src/addons/disk_space/page.rs:602` `swatch()` | No |
| LIVE tag dot / "Flight Recorder" dot | Flight Recorder header and Summary tile | `src/widgets/parts.rs:1337`, `src/addons/flight_recorder/page.rs:381` | No |
| Tree caret and leaf dot | Power & Freq sensor tree | `src/widgets.rs:862`, `:879` (`RowLead`) | Maybe (caret) |
| Expand chevron on an account | Users | `src/page_users.rs:124` `account_row()` | Maybe |
| "More actions" chevron | Installed Apps and Drivers action rows (the same code twice) | `src/addons/installed_apps/page.rs:425`, `src/addons/drivers/page.rs:292` | Maybe (one shared chevron) |
| "CPUs Expanded" dropdown chevron | Performance | `src/page_perf.rs:556` | Maybe |
| Graph line-end marker | Performance graphs | `src/page_perf.rs:944` | No |
| Painted checkbox | Installed Apps list rows | `src/addons/installed_apps/page.rs:1196` `tick_box()` | No |
| Switch | Settings | `src/widgets/parts.rs:675` | No |
| "+" / "−" stepper | Settings numbers | `src/widgets/parts.rs` `number_stepper()` | No |
| "+" of "+ Run new task" | Header of every page | `src/app.rs:785`, `src/widgets.rs:654` (text) | Maybe |
| Alert "!" | Benchmarks warning banner | `src/widgets/parts.rs:1289` `alert_banner()` (text) | Maybe |

## 3. Unicode characters used as icons (proposal: keep, or unify)

| Character | Where it shows | Code |
|---|---|---|
| ↑ ↓ (U+2191/2193) | Connections "System traffic" tile; Installed Apps sort headings | `src/addons/connections/band.rs:97,101`; `src/addons/installed_apps/page.rs:681` |
| ▴ ▾ (U+25B4/25BE) | Package Manager catalog sort headings | `src/addons/package_manager/page.rs:484-485` |
| → (U+2192) | "old → new" versions: Installed Apps, Drivers; update dialog lists | `src/addons/installed_apps/page.rs:785,1130`; `src/addons/drivers/page.rs:448` |
| › (U+203A) | Breadcrumbs: Settings › Add-ons, Disk Space path bar, "Open Windows Settings › Installed apps" | `src/page_settings.rs:150`; `src/addons/disk_space/canvas.rs:295`; `src/addons/installed_apps/page.rs:432` |
| • (U+2022) | Current choice in menus (update speed, priority, graph mode); separators and bullets in texts | `src/app.rs:576`; `src/widgets.rs:180`; `src/page_perf.rs:319,324`; others in Flight Recorder, Disk Space, Installed Apps, Package Manager |

egui built-ins (checkboxes, combo box and submenu arrows, collapsing
triangles, spinners) follow the theme and are not listed one by one.

## 4. Inconsistencies to fix whatever we redraw

- **Sort indicator** is drawn three ways: a painter triangle (`src/widgets/parts.rs:589`
  `sort_heading`, used by Processes and Connections), ▴▾ in Package Manager, ↑↓ in
  Installed Apps. One shared indicator.
- **Bolt** is drawn three times: `Glyph::Bolt`, Performance `Icon::Power`, and the old
  Power & Freq page icon (now only a fallback).
- **Shield** twice: `need_icon` Admin and `parts::shield`.
- **Search fields without a magnifier**: Processes (`src/page_processes.rs:201`), Services
  (`src/page_services.rs:74`), Drivers, Installed Apps and Settings "Find a setting"
  (`src/page_settings.rs:174`) are plain text boxes, unlike Connections, Package Manager and
  Power & Freq.

## 5. Already done, now only a fallback

The old painter page icons are drawn only when a page has no PNG (e.g. a test
add-on): `src/widgets.rs:322` `nav_icon()`, `src/widgets.rs:402` `plain_icon()`
(the default `Addon::icon`), and each add-on's `fn icon` (Power & Freq, Flight
Recorder, Connections, Installed Apps, Drivers, Disk Space, Benchmarks). They can
be removed once every page is sure to have a PNG.

## Other images

Only brand items: the window icon `assets/brand/icon-256.png`, the exe icon
(`build.rs`) and the sidebar logo. Nothing else is loaded as an image.

## 6. Round 2 analysis (2026-11-19, from screenshots of the app)

Decisions per remaining icon. **Redraw** = a new glyph in the kit style
(round 2 sheet); **Glyph** = use a glyph we already have; **Code** = no new art,
a code fix; **Keep** = stays as it is.

| Icon | Seen on | Verdict | Why |
|---|---|---|---|
| Record dot on the Record button | Flight Recorder | Glyph (`record`, tinted) | A red dot on the neon-green button clashes; the glyph keeps the kit look. |
| Stop square | Flight Recorder | Glyph (`stop`, tinted) | Same family as Record. |
| Chevron of "More actions" and "CPUs Expanded" | Installed Apps, Drivers, Performance | Redraw: `chevron-down` | A thin 1.5 px stroke; drawn twice by copy-paste. |
| Tree caret (Power & Freq) and account triangle (Users) | Power & Freq, Users | Redraw: `chevron-right` / `chevron-down` | Two different shapes for the same "open / closed"; the Users triangle is 8 x 5 px and barely visible. |
| Sort indicator | Processes (triangle), Installed Apps (↑↓), Package Manager (▴▾) | Redraw: `sort-up` / `sort-down` | Three implementations of one thing. |
| "+" of "+ Run new task" | Every page header | Redraw: `plus` | The only header button whose mark is a typed character next to Refresh's glyph. |
| Version arrow "old → new" | Installed Apps, Drivers | Redraw: `arrow-right` | It is the key information of an update card; a glyph reads better than "→" at 18 px. |
| Alert "!" | Benchmarks banner | Redraw: `warning` | A bare "!" character; a warning mark should look like one. |
| Search fields without a magnifier | Processes, Services, Drivers, Installed Apps, Settings "Find a setting" | Code | Use `search_field()` (already has the `search` glyph) like Connections and Package Manager. |
| Status band marks, status bar health dot, inline dots, run-state and outcome dots, intensity dots, LIVE dot, swatches | Many | Keep | Colour carries the meaning; a dot is the right form. |
| Spinners, checkboxes, switch, stepper, combo arrows | Many | Keep | Controls, not pictograms; they follow the theme. |
| Graph line-end marker | Performance | Keep | Part of the graph. |
| "›" breadcrumbs, "•" bullets and menu marks | Many | Keep | Typography, not icons. |
| Globe illustration | Connections | Keep | A visualisation. |

Also seen: the sidebar's last item (Package Manager) is partly covered by the
logo at the default window height. Not an icon, but worth a fix.
