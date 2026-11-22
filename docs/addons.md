# Add-on: proposal

Ultimate PC Manager must become the only tool needed to manage a PC, a sort of
PowerToys for maintenance and system control. Features beyond the core arrive as
**add-ons** that the user turns on or off; in the future some of them may be
**paid**.

This document proposes how to do it, starting from today's code. The open
decisions are at the end.

## 1. Core and add-ons

Starting proposal: the core is what every task manager has; the add-ons are the
7 pages listed below, plus those that will come later.

| Core, always active | Add-on |
|---|---|
| Summary | Power & Freq |
| Performance | Flight Recorder |
| Processes | Connections |
| System Info | Installed Apps (with winget updates) |
| App history | Drivers (with Windows Update) |
| Startup apps | Disk Space |
| Users | Benchmarks |
| Services | Package Manager (winget catalog) |
| Settings, Colors | *future: cleanup, backup, network, …* |

A turned-off add-on:

- does not appear in the side menu nor in `--page`;
- does not start threads, PowerShell scripts, winget, Windows Update or scans;
- keeps its settings, so turning it back on does not lose them.

## 2. Phase 1: modules included, turned on or off (like PowerToys)

All add-ons are compiled into the executable. This is the PowerToys approach and
the simplest to maintain: one build, one update, no binary compatibility issues
between versions.

### The common interface

```rust
/// An add-on: a page, its background work and its settings.
pub trait Addon {
    /// Stable identifier, used in settings and licenses: "drivers", "disk-space".
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// What it asks of the system; the Add-ons page shows it before the user turns it on.
    fn requirements(&self) -> Requirements; // see below
    /// The price: free, or tied to a product on sale (see section 3).
    fn tier(&self) -> Tier;

    /// Called when the user turns the add-on on or off, and at startup if it is on.
    fn start(&mut self, ctx: &AddonContext);
    fn stop(&mut self);
    /// Optional: work that turning it off would lose (e.g. a recording in progress).
    /// If present, the host does not turn it off right away: the app asks with the
    /// common Save / Discard / Cancel dialog (`AddonHost::off_question` / `answer_off`),
    /// then calls `stop`.
    /// Closing the app (X, File › Exit, Alt+F4) also asks this question for every add-on
    /// that is on and has unsaved work (`request_close` / `close_question` / `answer_close`):
    /// a successful Save or Discard (which calls `stop`) closes the app; a failed Save or
    /// Cancel leaves it open. The add-on stays on in the settings.
    fn unsaved_work(&self) -> Option<&'static str> { None }
    /// Save for that question; false if it was not saved (the add-on stays on).
    fn save_work(&mut self) -> bool { true }
    /// Its own threads that are still alive; the contract requires 0 when the add-on is off.
    fn threads(&self) -> usize { 0 }

    /// The page, drawn only when it is the open one.
    fn page(&mut self, ui: &mut egui::Ui, ctx: &AddonContext);
    /// Optional: a section in the settings.
    fn settings(&mut self, _ui: &mut egui::Ui) {}
    /// Optional: a tile in Summary.
    fn summary_tile(&mut self, _ui: &mut egui::Ui) {}

    /// Optional: the title of its menu in the page header ("Disk Space").
    fn menu_title(&self) -> Option<&'static str> { None }
    /// The entries of that menu, drawn while it is open.
    fn header_menu(&mut self, _ui: &mut egui::Ui, _page: &mut PageContext) {}
    /// Optional: that menu sits in the app's menu bar (after View) instead of the header,
    /// and is always shown only while its page is open (e.g. "Benchmarks").
    fn menu_in_menu_bar(&self) -> bool { false }
    /// The command-line arguments, once at startup and only if it is on, for its own
    /// flags (e.g. `--scan` for Disk Space).
    fn command_line(&mut self, _args: &[String]) {}
    /// The name of its row that owns `path` (e.g. the installed app whose folder contains
    /// it), so that another add-on's page can link to it. Asked only if it is on; it must
    /// be cheap.
    fn owner_of(&self, _path: &Path) -> Option<String> { None }
    /// Shows on its page the row that owns `path` (after `owner_of`); the app then opens
    /// the page.
    fn reveal(&mut self, _path: &Path) {}
    /// Optional: its column in the Processes table (hidden until the user shows it) and
    /// its item at the end of a process's menu ("Go to ..."). Asked only if it is on.
    fn process_column(&self) -> Option<ProcessColumn> { None }
    /// The column's figure per PID, asked once per frame while the column shows; it may
    /// start reading then. `process_column_hidden` tells it the column is gone.
    fn process_figures(&mut self, _settings: &Settings) -> Option<HashMap<u32, usize>> { None }
    fn process_column_hidden(&mut self) {}
    /// Opens its page on the process `pid` (its menu item was chosen).
    fn reveal_process(&mut self, _pid: u32) {}
    /// Optional: "packages changed". A job of the install queue has ended; `by` is the id
    /// of the add-on that ran it. Called only if it is on, once per job.
    fn packages_changed(&mut self, _by: &'static str) {}
    /// Optional: what the app asks before closing while the add-on works (e.g. a series
    /// of installs). `None`, the default, closes without asking.
    fn close_question(&self) -> Option<String> { None }

    /// Optional, for the add-on that records sensors for other pages (Flight Recorder):
    /// what it can do now at the update speed `interval_ms`. `None`, the default, for
    /// every other add-on. Asked even while it is off (the host then says
    /// `Recording::Off`); it must be cheap.
    fn sensor_recording(&self, _interval_ms: u64) -> Option<Recording> { None }
    /// Starts (`on`) or stops that recording for another add-on's page, through its own
    /// Record and Stop. Called only while it is on (see `AddonHost::record_sensors`);
    /// refused by default with the host's generic `RECORDING_OFF` reason.
    fn set_sensor_recording(&mut self, _on: bool, _page: &mut PageContext) -> Result<(), &'static str> { Err(RECORDING_OFF) }
    /// Before its page is drawn: what the recording add-on can do now, so the page can
    /// offer to record or stop with `Action::RecordSensors` (optional).
    fn recording_shown(&mut self, _recording: Recording) {}
}

/// What the add-on that records sensors can do now, as the host tells a page.
pub enum Recording {
    Off,                    // off, stopping or not registered: nothing can be recorded (default)
    Locked(&'static str),   // on, but it cannot start now, for this reason
    Ready,                  // it can start a recording
    Running,                // a recording runs
}

/// What it asks of the PC.
pub struct Requirements {
    pub admin: bool,                         // some actions require administrator rights
    pub network: bool,                       // contacts services on the internet
    pub programs: &'static [&'static str],   // external programs (winget, PowerShell)
    pub note: Option<&'static str>,          // a warning shown under the requirements, if any
}
```

`AddonContext` gives access to what is shared: the sampler's samples, the global
settings (units, theme), the common actions (run elevated, open folder,
confirmation dialogs, `notify(title, body)` for a Windows notification, e.g.
an alert that triggers; the test executor records it instead of showing it)
and the install queue (`install_queue`): only one install,
update or uninstall job at a time across the whole app. Whoever waits sees who
holds the queue; turning an add-on off withdraws its waiting requests.

When the app closes, the host first asks about unsaved work (see `unsaved_work`
above), then collects the questions of the add-ons that are on
(`AddonHost::close_questions`, from `close_question`): if there is at least one,
the app cancels the close and asks "Close the app?". Once confirmed,
`AddonHost::close` stops every add-on that is on as turning it off would (an
operation that must not be interrupted finishes its current step) and withdraws
their requests from the queue, but keeps them saved as on; the app closes when
`AddonHost::can_close` is true, that is when none of those operations is still
running.

### Reading sensors

A page or a Summary tile that shows sensor values (clocks, temperatures, power)
reads them through `PageContext::sensors()` and takes nothing by hand. It asks
in every frame it is drawn, and stores nothing: the host creates the shared
sensor service's interest on the first ask, keeps it while the page or tile asks
again in each frame, and drops it at the end of the first frame in which it did
not (and when the add-on is turned off or stopped, including the close of the
app). So there is nothing to drop in `page_hidden`, `summary_hidden` or `stop`.
Refresh reads every sensor at once through every interest the host holds (page,
tile, the core Summary). The core Summary asks the same way, through
`AddonHost::core_sensors`; the app clears its old sensor frame when the host says
Summary stopped asking.

Only an interest that does not depend on what is on screen is taken by hand,
through `AddonContext::sensors()`: a recording (Flight Recorder) or an alert
(Power & Freq). Its add-on owns it and drops it itself.

### The registry

A static list replaces the `Page` enum for everything that is not core:

```rust
pub fn registry() -> Vec<Box<dyn Addon>> {
    vec![
        Box::new(power::PowerAddon::default()),
        Box::new(drivers::DriversAddon::default()),
        // …
    ]
}
```

The state lives in `settings.json`:

```json
"addons": {
  "drivers": { "enabled": true },
  "disk-space": { "enabled": false }
}
```

Each add-on's own settings sit under its `id`, so they do not mix with the global ones.

### The Add-ons page

In Settings, one page lists all the add-ons with a toggle, a description, what they
require (administrator, network) and the license status. The design is the
**Add-ons** sheet on the canvas.

## 3. Paid add-ons

> Today every add-on is **Free** (`TIERS` in `src/addons.rs`); the project relies on
> donations. This section only records how selling could work if that ever changes.

> The table may also hold **feature rows**, named
> `<add-on id>.<feature>`, for parts of an add-on that could be sold apart:
> `power-freq.alerts` (threshold alerts) and `power-freq.pins` (sensors pinned to
> the Summary tile). The add-on asks `tier_of` for its feature row; there is no
> way to buy a feature yet, so a row that is not Free simply turns the feature
> off (it is not offered, and the user's saved alerts or pins are kept).

### Recommendation to start: signed license, verified on the PC

- When sold, a **signed license** is generated (Ed25519, crate `ed25519-dalek`): a
  small text with the licensee, the add-on or bundle purchased, the date and, if
  needed, an expiry. The app contains only the **public key** and verifies the
  signature at startup: no network for normal use, no server to keep running.
- The user pastes the key into the Add-ons page (or opens it from a link). The
  license is saved in `%APPDATA%\UltimatePCManager\license`.
- States of each add-on: **Free**, **Trial** (N days), **Purchased**, **Not
  purchased**. Not purchased: the page shows a locked preview with the buy button;
  no background work.
- **Sales:** a *merchant of record* (Lemon Squeezy, Paddle) collects payment, handles
  European VAT and invoices, and already has a license-key API (activate, verify,
  deactivate).
- **Device limit (optional):** if needed, activation goes once through the seller's
  API, which counts the PCs; after that, verification stays local.

### The limit to know

As long as the code of the paid add-ons is inside the free executable, anyone who
modifies the binary can skip the check. It is the same trade-off as
almost all desktop programs: fine to start with. If it becomes a problem, phase 2
is needed.

## 4. Phase 2, only if needed: add-ons distributed separately

Useful for third-party add-ons, or to avoid handing the code of the paid ones to
everyone. In Rust you cannot reliably load a separately compiled library (Rust does
not guarantee binary compatibility between versions), so the routes are:

| Route | How | Pros | Cons |
|---|---|---|---|
| Separate process | The add-on is a signed executable; it talks to the app over a named pipe and sends it data, and the app draws the page | Isolated: if it hangs it does not bring down the app; downloaded only by whoever bought it | Needs a protocol to describe the page |
| WASM (wasmtime) | The add-on is a WebAssembly module with explicit permissions | Safe for third-party add-ons | Calls to Windows go through the app; more work |

If the phase 1 `Addon` interface is clean, phase 2 adds an adapter without rewriting
the existing add-ons.

## 5. Refactoring steps (phase 1)

1. Introduce `Addon`, `AddonContext`, `Needs`, `Tier` and the registry; no visible change.
2. Move one add-on at a time into the registry, starting with **Benchmarks** (the most
   isolated), then Disk Space, Connections, Power & Freq, Flight Recorder, Installed
   Apps, Drivers. At each step the app compiles and works as before.
3. Remove from `app.rs` the `match` branches and state fields of the add-ons that were moved.
4. Add `addons` to `settings.json` and the Add-ons page with its toggles; the side menu
   and `--page` read the registry.
5. Licenses: signature verification, states, locked preview. The seller link arrives
   when there is really something on sale.

## 6. Open decisions

- Which pages are core and which are add-ons (the table above is a proposal).
- Which paid add-ons, and whether they are sold one by one, as a bundle, or both.
- Lifetime license or subscription; how many PCs per license.
- Whether add-ons written by others will be accepted in the future (this decides whether
  phase 2 is needed and which route to take).
