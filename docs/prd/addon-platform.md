# Add-on platform (prerequisite)

## Problem Statement

Ultimate PC Manager is a PC manager written in Rust (see the
[README](../../README.md)); in the code the app is still called "My Task Manager". Today it has 17 pages: the 15 in the
side menu plus Settings and Colors (the colors menu that opens from the button at the bottom of the sidebar).

All pages always appear in the menu and the user cannot exclude any of them, even those they don't use.
Heavy work (winget, Windows Update, PowerShell scripts, scans) starts when the user opens the page or presses a
button, but once started it keeps running outside the page and there is no way to stop it. Every function is
hard-wired into the **platform code** (the page enumeration, the menu, the sample loop, the status bar): adding a new
one means modifying those central points.

Product vision, as context and not as a measurable goal: the app must become the tool for managing a PC, with a core
that is always present and additional features that the user chooses.

## Solution

A fixed **core** (Summary, Performance, Processes, System Info, App history, Startup apps, Users, Services, Settings,
Colors) plus a set of **add-ons** compiled into the app and switchable one by one from a **Settings › Add-ons** page,
modelled on PowerToys.

A switched-off add-on disappears from the side menu, starts no new work and keeps its settings; opening it with
`--page` leads to Settings › Add-ons with that add-on selected. Each add-on declares what it asks of the system
(administrator rights, internet, external programs) and its tier: free or paid. How licenses, trials and sales work
will be decided in a separate PRD ([draft](licenses-draft.md)).

The first add-ons are 7 existing pages, each with its own PRD:
[Power & Freq](power-and-freq.md), [Flight Recorder](flight-recorder.md), [Connections](connections.md),
[Installed Apps](installed-apps.md), [Drivers](drivers.md), [Disk Space](disk-space.md), [Benchmarks](benchmarks.md).
The proposal from which this PRD originates is [`docs/addons.md`](../addons.md); in case of conflict, this PRD prevails.

## Goals and non-goals

**Goals**

- Each add-on is switched on and off from Settings › Add-ons without restarting the app.
- A switched-off add-on does not appear in the menu or in Summary and does not start new work.
- An add-on's settings survive switching it off, an update, and a return to a previous version.
- Adding an add-on touches only its own module and one registry line.
- Flight Recorder and Power & Freq leave the platform code like the others.

**Non-goals**

- Licenses, trial, sales and locked preview: separate PRD.
- Add-ons distributed separately, external or third-party.
- Renaming the app.

## Alternatives considered

| Alternative | Why it was discarded |
|---|---|
| A settings flag to hide pages, without a common interface | Hides the menu but leaves each function scattered through the platform code; it does not give a single place for each page, its work and its settings. |
| Separate builds or Cargo features (free and full version) | Two executables to distribute and update; the user cannot turn an add-on on without reinstalling. |
| Libraries loaded at runtime (DLLs) | Rust does not guarantee binary compatibility between versions: every app update would break the add-ons. |
| Separate process or WASM right now | Requires a protocol to describe the pages; high cost without third-party add-ons to host. |
| Licenses in this PRD | The decisions they depend on are open and the stories contradicted each other; moved to a separate PRD. |

## User Stories

### Managing add-ons

1. As a user, I want to see in Settings › Add-ons the list of all available add-ons, so that I know what the app can do beyond the core.
2. As a user, I want to turn each add-on on or off with a toggle, so that the app contains only what I use.
3. As a user, I want a switched-off add-on to disappear from the side menu, so that the menu shows only what I use.
4. As a user, I want a switched-off add-on not to start new work and the work that can be stopped to be stopped, so that the app does not consume CPU, network or disk for functions I don't use.
5. As a user, during an operation that cannot be interrupted (installation, uninstall, update) I want the toggle to show "will turn off when done", so that switching off an add-on does not leave the system half-done.
6. As a user, I want an add-on's settings to remain saved when I switch it off, so that when I turn it back on I find it as I left it.
7. As a user, I want a return to a previous version of the app, or an error in an add-on's settings, not to make me lose the other settings.
8. As a user, I want a switched-on add-on to be usable straight away, without restarting the app.
9. As a user, I want to read for each add-on a one-line description, so that I understand what it is for before switching it on.
10. As a user, I want to see whether an add-on needs administrator rights for some actions, so that the Windows request does not surprise me.
11. As a user, I want to see whether an add-on uses the internet, so that I know which functions contact external services.
12. As a user, I want to see whether an add-on uses external programs (winget, PowerShell), so that I know what must be installed.
13. As a user, I want to see in the side panel the resource consumption declared by the selected add-on (CPU, network, disk, and when it works), so that I understand what keeping it on involves.
14. As a user, I want the top band to say how many add-ons are on, so that I have the answer without counting them.
15. As a user, I want the core to be labelled "always active", so that I understand why it cannot be switched off.
16. As a user, I want opening a switched-off add-on with `--page` to take me to Settings › Add-ons with that add-on selected, so that I can turn it on instead of seeing an error.
17. As a user, I want an invalid start page (unknown or a switched-off add-on) to open Summary, so that the app always opens on a valid page. Today an unknown page opens Processes: the behaviour changes.
18. As a user, I want the side menu to separate the core from the add-ons with an "Add-ons" heading like the ones already used in the menu, so that I can tell the two parts apart.
19. As a user, I want "Refresh" to update the open add-on, as it today updates the open page.
20. As a user, I want the controls an add-on puts in the status bar (for example the recording buttons of Flight Recorder) to disappear when the add-on is off.
21. As a user, I want an add-on to be able to have its own section in the settings and a tile in Summary, which disappear when it is off.

### Developer

22. As a developer, I want a common interface that every add-on implements, so that adding an add-on touches only its module and one registry line.
23. As a developer, I want a single registry of add-ons, so that the menu, settings, `--page`, start page and Summary all read the same source.
24. As a developer, I want every add-on to have a stable identifier, so that settings remain valid across versions.
25. As a developer, I want each add-on's settings to be separated by identifier and read one by one, so that a broken section falls back to defaults only for itself.
26. As a developer, I want a hook called on every performance sample even when the page is closed, reserved for short work, so that Flight Recorder and Power & Freq collect data without living in the platform code.
27. As a developer, I want a status-bar area provided by the platform, so that an add-on can show its controls without modifying the bar.
28. As a developer, I want the start of an add-on not to do costly work and the heavy work to start at the first opening of the page or on the user's action, as today.
29. As a developer, I want switching off to stop the work that can be stopped and discard the results of the work that cannot.
30. As a developer, I want a shared context (performance samples already collected by the app every second, global settings, the already existing queue of common actions: running with administrator rights, opening folders and URLs, asking for confirmations), so that add-ons do not duplicate code.
31. As a developer, I want the code used by several add-ons (PowerShell execution, administrator check, CSV, shared drawing helpers) to live in the platform, so that switching off one add-on does not break another.
32. As a developer, I want to move add-ons one at a time, leaving the app working at each step, so that each step can be tried and reverted on its own.
33. As a developer, I want common contract tests on every add-on, so that an add-on that does not stop its work when off or loses its settings makes the tests fail.
34. As a developer, I want every add-on to declare its tier (free or paid), so that the future license PRD already has the data.

## Implementation Decisions

- **Core and add-ons.** Core: Summary, Performance, Processes, System Info, App history, Startup apps, Users, Services,
  Settings, Colors. Initial add-ons: Power & Freq, Flight Recorder, Connections, Installed Apps, Drivers, Disk Space,
  Benchmarks. The "Power" entry of Performance (power of the CPU package) stays in the core and must not depend on the
  code of Power & Freq.
- **Phase 1, modules included (PowerToys model).** All add-ons are compiled into the executable; there is no dynamic
  loading.
- **Common `Addon` interface.** Each add-on exposes: stable identifier, name, one-line description, requirements
  (administrator, network, external programs), declared consumption, tier (free / paid); start and stop; page drawing;
  optional: hook on every sample, status-bar area, manual refresh, settings section, Summary tile.
- **Shared context `AddonContext`.** Gives access to the samples already collected by the app's sampling loop, to the
  global settings (theme, units, speed) and to the already existing action queue (`Action`, executed after drawing).
  The code today shared between Drivers and Installed Apps (PowerShell execution, administrator check, CSV field,
  drawing helpers for dates, versions and ticks) moves into the platform or into common widgets.
- **Registry and page identity.** A single list describes all pages, core included, with fixed entries that cannot be
  switched off; menu, icons, `--page`, start page, last page and Summary read from there. Pages are saved by
  identifier, not by position.
- **First visible step.** The function that today says whether a page is included always returns "yes": it must be
  tied to the on/off state, so the toggles work even before the add-on code is moved.
- **Settings.** The settings file gains an `addons` section: for each identifier, the on/off state and an opaque value
  with its own settings. Default: all on. Sections are read one by one; unknown fields are kept when saving, so that a
  return to a previous version does not delete them. An unreadable section falls back to defaults only for that add-on.
- **Lifecycle.** At startup the platform calls the start of the add-ons that are on, which must not do costly work.
  Switching on/off calls start/stop without restarting the app. Stop stops the work that can be stopped; for work that
  cannot be stopped the toggle shows "will turn off when done", and a per-add-on generation counter discards old results.
- **User interface.** Settings › Add-ons follows the style of the "Add-ons (proposal)" board of the project's design
  canvas: band with the count, list with toggle, tier and requirement icons, side panel with details and actions,
  status bar. Search and filters will be added when the add-ons grow. `--page` accepts "Add-ons" as the name of the
  Settings subsection.
- **Refactoring order.** 1) registry, interface and context, with the core as fixed entries; tie the toggles to the
  state; 2) move the code shared between add-ons into the platform; 3) move one add-on at a time (Benchmarks, Disk
  Space, Connections, Power & Freq, Flight Recorder, Installed Apps, Drivers); 4) remove from the platform code the
  branches of the page enumeration and the app-state fields of the moved add-ons; 5) complete Settings › Add-ons page.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Switching off an add-on during an installation, uninstall or update leaves the system half-done | "Will turn off when done" toggle; old results discarded with the generation counter. |
| An add-on that saves invalid data loses all the settings | Sections read one by one, unknown fields kept, defaults only for the broken section; dedicated test. |
| Flight Recorder and Power & Freq remain tied to the platform code | Hook on every sample and status-bar area in the interface; test that moves them last. |
| The hook on every sample slows the window | "Short work only" rule and a test that measures the duration of the hook. |
| Regressions during the move | One add-on at a time, app working at each step, contract tests at each step. |
| Switching off an add-on breaks another that uses its code | Shared code moved into the platform before the add-ons; test that switches each add-on on alone. |

## Testing Decisions

- A good test verifies external behaviour (what the user sees, what stays saved, what work runs), not internal details.
- **Common contract tests**, run on every registered add-on: when off it starts no work; starting does not launch external
  processes; off and back on keeps the settings; the identifier is stable and unique; each add-on works on its own when on;
  the hook on every sample stays under the permitted duration.
- **Registry and settings**: menu and `--page` reflect the state; invalid start page opens Summary; unknown fields kept
  on save; a broken section falls back to defaults only for itself; default all on.
- **Capture**: `--page "Add-ons" --screenshot` to check the page.
- Prior art: the Rust unit tests already present in the driver and installed-apps modules (version comparison, parsing),
  which do not touch the system; the `#[ignore]` tests `lists_this_pc` that read the PC's real data.

## Out of Scope

- Licenses, trial, sales, locked preview and license states: separate PRD ([draft](licenses-draft.md)).
- Add-ons distributed separately (separate process, WASM) and third-party add-ons. The phase 1 interface draws the
  pages directly with egui: phase 2 will require a different drawing interface.
- The content of each add-on: it has its own PRD.
- Renaming the app (title, executable, settings folder still "My Task Manager").

## Further Notes

- Open decisions: final core/add-on split; all the licensing decisions (see the draft).
- Prerequisite of: the 7 add-on PRDs.
