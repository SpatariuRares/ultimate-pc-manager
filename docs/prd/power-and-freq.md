# Power & Freq add-on

## Problem Statement

Those who use Ultimate PC Manager want to know how fast the processor really runs, how much it consumes, how hot it is, and whether disks, graphics card and power supply are healthy, without installing HWiNFO or the manufacturers' programs. Today the Power & Freq page is a bar of frequencies per core plus a grid of four values (Average clock, Fastest core, Power plan, Power source), a line about the battery and a list of temperatures taken from `sysinfo`, which on this PC is empty ("This computer doesn't share temperature sensors with apps."). The displayed frequency is the "reported" one (`sysinfo`, the same on all cores: 4.70 GHz), not the effective one, and the power plan is read by launching `powercfg` at every update.

The page must instead be a **tree of hardware sensors**: This PC > device > group > sensor, with columns **Sensor | Value | Min | Max**, groups with a summary value, unavailable readings distinguished from zero, and different sources for each vendor (Windows counters, Energy Meter, NVML/ADLX/IGCL for the GPUs, DXCore for the NPU, AMD Ryzen Master and Intel Dynamic Tuning SDKs, S.M.A.R.T. counters for the disks). It must be rebuilt and turned into an add-on that the user can turn on or off: when off, it must not read any sensor nor load any vendor library.

## Solution

A `power-freq` add-on ("Power & Freq", alias `power-and-freq`) registered in the add-on registry, with:

- a page in the **design A** layout (table `P09-Power` of the canvas): status band at the top that answers "how fast are the cores running?" ("Cores run at 5.11 GHz, 9% above the 4.70 GHz base"), with a sub-line (reported clock, parked cores, plan, power source) and, when needed, a secondary band that explains why CPU power and temperature are missing (AMD SDK absent, administrator needed) with the actions "Get AMD's add-on" and "Restart as administrator";
- a sensor tree with the Sensor / Value / Min / Max columns, device and group headers, summary values on the groups, "Expand all" / "Collapse all", a "Find" search, and a side panel with the selected sensor (last-minute graph, reported clock, Min / Max, "Read from"), the "Sources on this PC" list and the actions "Copy reading" and "Open Windows power options";
- a layer of sensor providers that produces a telemetry frame with stable identities of device and metric, and a statistics accumulator (min / max) independent of the UI;
- background work (reading the sensors) started only when the add-on is on and someone is watching (page open or Summary tile), and stopped completely when the add-on is off;
- a section in the settings and an optional tile in Summary.

Prerequisite: the separate PRD **"Add-on platform"** (`Addon` interface, `AddonContext`, `Needs`, `Tier`, registry, `addons` in `settings.json`, Add-ons page, signed licenses and states Free / Trial / Bought / Not bought with locked preview).

## User Stories

Page and sensor tree

1. As a user, I want to open "Power & Freq" from the side menu or with `--page "Power & Freq"`, so that I can reach the page directly (the id is `power-freq` and `power-and-freq` remains an alias).
2. As a user, I want to see the sensors in a tree This PC > device > group > sensor, so that I immediately find the reading I am interested in.
3. As a user, I want the columns Sensor, Value, Min, Max, so that I see the current value and the extremes of each reading (no Source column).
4. As a user, I want the counters (Data Read, Host Read Commands, Power On Hours…) to have only Value, so that I do not see meaningless min / max.
5. As a user, I want each group row to have an arrow, an icon and a summary value with min / max (Clocks 4.72 GHz / 2.98 / 5.10; S.M.A.R.T. "Healthy"), so that I understand the state of the group without opening it.
6. As a user, I want to expand and collapse each device and group, and to have "Expand all" / "Collapse all" in the header, so that I control how long the list is.
7. As a user, I want a "Refresh" button in the header, so that I can force a new reading.
8. As a user, I want a "Find" field that filters the sensors by name, so that I find "Hotspot" or "Power On Hours" in a long list (our addition, design A).
9. As a user, I want the tree to remember which groups I opened, so that when I return to the page I find it as it was (our addition).
10. As a user, I want an unavailable reading to be grey with a line that says why ("needs administrator", "needs AMD's add-on", "no provider on this PC"), never shown as zero, so that I do not mistake missing data for a real value.
11. As a user, I want a stale reading to carry the mark "(stale)", so that I know the value is old.
12. As a user, I want the message "Waiting for all sensors to report..." until the providers have responded the first time, so that I do not think the page is empty.
13. As a user, I want temperatures to follow the °C / °F setting, so that they read as in the rest of the app.

Status band and tiles

14. As a user, I want a sentence at the top such as "Cores run at 5.11 GHz, 9% above the 4.70 GHz base", so that I have the main answer without reading the tree (our addition, design A).
15. As a user, I want under the sentence the reported clock, how many logical processors are parked, the power plan and the power source ("AC power, no battery"), so that I have the context in one line.
16. As a user, I want KPI tiles (Core Average, base frequency, power source, thermal state), so that the key numbers are large and readable (our addition).

CPU

17. As a user, I want the reported frequency of each logical processor and the package average, so that I see what Windows says it has set.
18. As a user, I want the effective frequency "Core Average" calculated from "% Processor Performance" for the base clock, so that I see how fast it really runs, boost included.
19. As a user, I want the base frequency and the base clock in the General group, so that I can compare them.
20. As a user, I want the maximum clock of each core, so that I know how far it can go up.
21. As a user, I want the Bus Clock, so that I see the bus clock; if it cannot be read, I want the grey row with the reason.
22. As a user with an AMD processor, I want the Fabric Clock (FCLK), so that I see the clock of the Infinity Fabric (requires the AMD SDK).
23. As a user, I want to know which logical processors are parked (a "Parked" column/value per core and a count in the band), so that I understand why some cores are idle.
24. As a user, I want a row per logical processor with clock, maximum clock and parked state, so that I see every core.
25. As a user, I want the group "Levels (Energy Perf)" with the minimum and maximum processor state of the active plan and the EPP preference, so that I know how the plan limits the CPU.
26. As a user, I want in the General group the frequency driver, the governor and the power preference (Windows power mode), so that I know who decides the clocks.
27. As a user, I want the name of the Windows power plan in read-only mode, so that I know which plan is active.
28. As a user, I want the total CPU usage and per core in the Utilization group, so that I can connect frequency and load.
29. As a user with an AMD processor, I want package, core and SoC power, PPT limit, package temperature, core and SoC voltages, so that I have what Ryzen Master shows (requires the AMD Ryzen Master Monitoring SDK).
30. As a user with an AMD processor without the SDK, I want the message "Ryzen temperature and power need a free add-on from AMD; see …" with the link and the "Get AMD's add-on" button, so that I know how to unlock those sensors.
31. As a user, I want the status messages of the AMD SDK (administrator needed, driver not started, virtual machine, zero values), so that I understand why it does not read.
32. As a user with an Intel processor, I want the temperatures of Intel Dynamic Tuning and its messages, so that I have the package temperature.
33. As a user, I want the power from the Windows Energy Meter channels (package RAPL, per-core rails, system power), so that I have the watts even without the manufacturers' SDK.
34. As a user, I want under This PC > Powers the system, CPU, GPU, DRAM and SoC powers when available, with the missing rows grey and saying why, so that I see the overall consumption.

Power, battery, system temperatures

35. As a user, I want to know whether the PC is powered from the mains ("AC Power") or from the battery, so that I interpret the clocks correctly.
36. As a laptop user, I want the battery charge, whether it is charging yes/no, remaining time, number of cycles, condition and battery saver state, so that I know how the battery is doing. Next to the remaining time, the charge or discharge rate in watts with words ("charging at 25 W", "draining at 12 W"), from `BATTERY_STATUS.Rate` (mW, positive charging, negative discharging); an unknown rate (`BATTERY_UNKNOWN_RATE`) or one in relative units greys with its reason (#190).
37. As a desktop user, I want to read "No battery in this PC" instead of empty rows, so that I do not think it is an error.
38. As a user, I want the thermal state (nominal / elevated / serious / critical), so that I know whether the system is slowing down due to heat.
39. As a user, I want the ACPI thermal zones with their messages, and "firmware publishes no ACPI thermal zones" when there are none, so that I know whether the firmware exposes temperatures.

GPU, NPU, motherboard, memory

40. As a user with an NVIDIA, AMD or Intel GPU, I want graphics clock and maximum, memory clock and maximum, card power and imposed limit, temperature and hotspot, fan, usage, video encoder and decoder, so that I have the picture of the graphics card (NVML / ADLX / IGCL vendors).
41. As a user, I want on the header of each GPU the line "Clock %1   Power %2   Memory %3", so that I have the summary without opening the groups.
42. As a user with an NPU, I want usage, power, interval energy, temperature and engine and memory clock, and "No NPU in this PC" when DXCore does not report one, so that I know what the NPU does.
43. As a user, I want voltages, temperatures and fans of the motherboard and clock / temperatures of the memory when a vendor exposes them, and grey rows with the reason when there is no vendor, so that the page is honest about what it cannot read (not available on this PC).

Disks

44. As a user, I want for each disk temperature (Composite, min, max), usage (activity), read and write speed, so that I see how the disk is working ("Speed" group).
45. As a user, I want the S.M.A.R.T. group with Available Spare, Available Spare Threshold, Percentage Used, Data Read, Data Written, Host Read / Write Commands, Controller Busy Time, Power Cycles, Power On Hours, Unsafe Shutdowns, Media and Data Integrity Errors, Error Log Entries, and the "Healthy" summary, so that I know how worn the disk is.
46. As a user with SATA disks, I want the equivalent ATA counters, so that non-NVMe disks also have health.
47. As a user without administrator rights, I want the S.M.A.R.T. rows to say "needs administrator", so that I know how to get them.
48. As a user, I want an optional "Manual SMART Probe" with a warning text, so that the direct disk reading starts only if I ask for it.

Side panel and actions

49. As a user, I want to select a sensor and see in the side panel its name, device and group, the large value, Min / Max and "Read from" (the source), so that I know where the number comes from.
50. As a user, I want a small graph of the last minute of the selected sensor, with the base frequency line for clocks, so that I see the trend (our addition).
51. As a user, I want the "Sources on this PC" section that lists the providers and whether they work (Windows counters, Energy Meter, NVML, ADLX, IGCL, DXCore, AMD SDK, Intel DTT, S.M.A.R.T.), so that I understand what is missing and why.
52. As a user, I want "Copy reading" to copy the name, value, min, max and source of the sensor, so that I can paste it into a message (our addition).
53. As a user, I want "Open Windows power options" to open the Windows panel, so that I can change plan there (our addition; the app does not change the plan).
54. As a user, I want a note that points to Processes > Efficiency mode for slowing down a single program (EcoQoS), so that I do not look for it here.
55. As a user without administrator rights, I want "Restart as administrator" in the band when there are sensors that require it, so that I can unlock them with one click and a UAC prompt.

Add-on behavior

56. As a user, I want to turn Power & Freq on or off from the Add-ons page, so that I keep the app light if I do not need it.
57. As a user, I want, with the add-on off, the page to disappear from the side menu and from `--page`, and no sensor to be read nor any vendor DLL (nvml, ADLX, IGCL, AMD SDK) to be loaded, so that the app does no hidden work.
58. As a user, I want, with the add-on on but the page closed (and no Summary tile), the sensors not to be read, so that I do not pay CPU for data nobody is looking at.
59. As a user, I want, when I turn the add-on on again, to find my settings again (open groups, selected sensor, Manual SMART Probe consent, Summary tile yes/no), so that I do not have to redo them.
60. As a user, I want the Add-ons page to tell me before turning it on what it requires (optional administrator for S.M.A.R.T. and the AMD SDK; no network; vendor DLLs if present), so that I decide with full information.
61. As a user, I want a section in the add-on settings with: Summary tile yes/no, show unavailable rows yes/no, consent to the Manual SMART Probe, so that I can adapt the page.
62. As a user, I want an optional tile in Summary (off by default) with Core Average and, when this PC reads them, the CPU package power and temperature, each GPU's Board Power and temperature, and the S.M.A.R.T. status ("Healthy" or the first problem), so that I have the main figures of the whole PC on the first page; a value this PC does not read has no line (#192).
63. As a user, I want the reading frequency to follow the app's update speed (Fast / High / Normal / Slow / Paused), and in Paused the page to stay still, so that it behaves like the other pages.
64. As a user, if Power & Freq becomes paid in the future and I have not bought it, I want a locked preview with sample data, the banner "Power & Freq is a … feature" and the buy and "Enter License Key…" buttons, so that I know what I am buying; in this state no background work runs.
65. As a user on a trial, I want to see how many days remain, so that I do not lose the feature without knowing it.
66. As a user, I want a vendor error (a DLL that crashes, a driver that does not respond) to mark only its sensors as unavailable, without bringing down the page or the app, so that the rest keeps working.

## Implementation Decisions

**New modules and modules to modify**

- **Telemetry model (pure).** A frame with stable identities: devices (This PC, CPU package, each GPU, NPU, each disk, motherboard, memory, battery, thermal zones), groups in this order (Temperatures, Utilization, Clocks, Powers for the CPU; Temperatures, Utilization, Speed, S.M.A.R.T. for the disks; plus General, Voltages, Levels (Energy Perf), Fans where they are needed) and readings. Each reading has a metric id (e.g. `cpu.package.frequency.average_effective_hz`), name, unit, optional value, reason for unavailability, source, type (measurement with statistics or counter without min/max) and the instant of the last update. Interface: build the frame, merge readings from different providers, compute the summary value of a group, mark as stale a reading older than N intervals.
- **Statistics accumulator (pure).** Keeps min / max per metric id since the start of observation (the "epoch"), ignores unavailable values and non-finite ones; resets when the add-on stops. A possible manual "reset" remains to be decided (see Further Notes).
- **Formatting (pure).** GHz / MHz, W, °C / °F, %, GB/s, TB, hours, counts with thousands separator, "Healthy" for the S.M.A.R.T. summary; band sentence ("x% above / below the base"); effective clock = base clock × % Processor Performance / 100.
- **Providers (Windows).** Each with a common interface: availability and reason, reading that returns partial readings, maximum time per call. Sources:
  - PDH counters: `\Processor Information(*)\% Processor Performance`, `Processor Frequency`, `Parking Status`, `% Processor Utility`; `\Energy Meter(*)\Power` (already used in perfdata for the package power); `\Thermal Zone Information(*)\Temperature`; utilization and speed of the physical disks;
  - CPU limits (#186, source "CPU limits (Windows counters)", same PDH query): `\Processor Information(*)\% Performance Limit` and `Performance Limit Flags` fill the CPU's **Limits** group (after Clocks): one Performance Limit and one Limit Reasons row per logical processor, plus a package row with the worst (lowest) percent and the reasons of every core. The flags decode into words by the documented `LimitReasons` bits of `PEP_PPM_PERF_CONSTRAINTS`: 0x1 thermal, 0x2 power, 0x4 domain dependency; other bits read "unknown reason (bit N)", no bit reads "none". Below the base clock with a known reason the band adds the cause ("… below the 4.70 GHz base: power limit"). "Sources on this PC" lists CPU limits after Windows counters with its own state; a missing counter greys only its own rows;
  - CPU idle states (#187, source "Windows counters", same PDH query): `\Processor Information(*)\% C1 Time`, `% C2 Time` and `% C3 Time` fill the CPU's **Idle states** group (after Limits): Package C1 / C2 / C3 (the average of every logical processor) and one CPU N C1 / C2 / C3 row per logical processor, as percentages with statistics (ids `cpu.idle.c1_pct`, `cpu.coreN.idle.c1_pct`, ...). A state whose counter is missing greys only its own rows ("no counter on this PC");
  - `CallNtPowerInformation(ProcessorInformation)` for CurrentMhz and MaximumMhz;
  - power plan APIs (`PowerGetActiveScheme`, `PowerReadACValueIndex`/`DCValueIndex`, `PowerReadFriendlyName`) for the plan name, minimum/maximum state, EPP; `PowerGetEffectiveOverlayScheme` for the mode (already in perfdata). Replaces the `powercfg` launched today;
  - `GetSystemPowerStatus` and battery IOCTLs for charge, cycles, condition, remaining time, charge / discharge rate (`IOCTL_BATTERY_QUERY_STATUS`); battery saver state;
  - GPU: NVML (`nvml.dll`) for NVIDIA, ADLX for AMD, IGCL for Intel, loaded dynamically only if present; DXCore to enumerate adapters and NPUs;
  - disks: storage property queries for the NVMe health log and ATA counters (requires administrator), usage from the counters;
  - AMD Ryzen Master Monitoring SDK (detected from the registry key, requires administrator and the AMD driver) and Intel Dynamic Tuning, loaded dynamically.
- **Shared sensor service.** Providers, model and statistics live in a shared module that does not belong to a page: Flight Recorder (Thermals panel) and, in the future, Performance (Thermals tile not built yet) also use them. The service starts only when at least one add-on that is on asks for it and stops when nobody asks.
- **Page (design A).** Status band + SDK/administrator band, KPI tiles, tree with Sensor / Value / Min / Avg / Max (Avg since #185) and rows per logical processor, side panel (selected sensor with last-minute graph, Sources on this PC, EcoQoS note, Copy reading, Open Windows power options). Replaces the current `page_power` and the `PowerInfo` block of the pro module; the `freq_hist` history kept today by the app moves to the service.
- **Statistics (#185, supersedes Min / Max only).** The sensor service keeps one statistics accumulator per run: Min, Avg and Max per metric since an epoch, counting every published frame once; unavailable and not finite values are ignored, counters and text states show Value only. "Reset statistics" (next to Expand all / Collapse all) starts a new epoch now, and the page says since when ("Statistics since 14:32"). The statistics survive leaving the page while anyone (the Summary tile, Flight Recorder) holds interest in the service, and clear when the last interest drops. "Copy reading" gives "Name (Device › Group): value, Min x, Avg y, Max z, since HH:MM, read from S".
- **Summary tile** (off by default) drawn with the host's Summary tile helper: one line per value this PC reads — Core Average, package power, package temperature, one line per GPU with its Board Power and temperature, then S.M.A.R.T. ("Healthy" when every read disk says so, else the first disk's problem). No grey placeholder lines (#192).
- **Pinned sensors (#193).** "Pin to Summary" / "Unpin from Summary" in the side panel pins the selected sensor; the Summary tile shows the pinned sensors after its built-in lines, in the order pinned, each "Name: value" (an unread value gives its reason) with an "Unpin" button. A pinned id missing from the current frame (a removed disk) shows "<id>: not on this PC" (pure `power_freq::pinned_lines`). Saved under `pinned` (ordered list of metric ids), written only while a pin exists. The feature has its own line in the tier table (`power-freq.pins`, Free).

**How it becomes an Addon**

- id `power-freq` (alias `power-and-freq`), name "Power & Freq", tier Free: the tier of each add-on is a row of the `TIERS` table in `src/addons.rs`, so making it paid is a word to change.
- `needs`: **optional** administrator (S.M.A.R.T., AMD SDK), no network, external programs: vendor DLLs if installed.
- `start`: registers interest in the sensor service but starts no readings until the page or tile is visible; `stop`: cancels the interest, waits for the service thread to end if no one else uses it, unloads the vendor DLLs, resets the statistics.
- Reading at the pace of the app's update speed; slow readings (S.M.A.R.T., NVML at startup) on their own slower pace; each provider with a timeout so that a blocked one does not stop the others.
- Settings under `addons.power-freq`: open groups, selected sensor, Summary tile, show unavailable, Manual SMART Probe consent, and `history_window_s` (the side-panel graph's time window, default 60 s, written only when changed).
- **Graph history (#184).** The service keeps timestamped points per metric over a time window and drops points older than the window, not a fixed count, so the graph covers the same time at any update speed. Its caption states the time actually shown: "Last minute" or "Last 2 minutes" once the window is full, the real span ("Last 40 seconds") while it fills. This supersedes the example of story 27 of #182 ("Last 2 minutes" at 2 s): the window is fixed (Settings, default 60 s), so the caption no longer follows the update speed.
- "Restart as administrator" uses the common "run elevated" action of the `AddonContext`, reopening the same page.
- **Alerts (#194).** Settings › Power & Freq lists the alerts: sensor, above / below, value, duration, Delete, and "Add alert" (a new alert watches the CPU package temperature, above 90 °C for 10 s). They are saved under `alerts` (`metric`, `comparison`, `threshold` in the metric's base unit, °C for temperatures, `duration_s`); the key is written only while an alert exists. Thresholds are typed and shown in the app's temperature unit (clocks in GHz). A pure evaluator (`power_freq::alerts::evaluate`: rules + frame + instant → statuses and transitions) triggers once the value stays past the threshold for the duration and clears once it stays back on the safe side for the same duration (hysteresis): one transition per crossing. An unavailable or missing sensor neither triggers nor clears; Settings shows why. While the add-on is on and at least one alert exists, one watcher thread holds a sensor-service interest (page closed, window minimized), evaluates every new frame and raises one Windows notification per transition through `AddonContext::notify` (a notification-area balloon, shown as a toast, by `platform::toast`); with no alert nothing runs. Triggered sensors are highlighted in the tree and named in the status bar. The feature has its own line in the tier table (`power-freq.alerts`, Free).
- **Record with Flight Recorder (#195).** A row under the status band: "Record with Flight Recorder", off with the reason when Flight Recorder is off ("Flight Recorder is off: turn it on in Settings › Add-ons to record.") or locked (Flight Recorder's own reason: replay open, updates paused, last trace unsaved); "Recording…" with Stop while it records. The state comes from the host before each draw (`Addon::recording_shown`, pure `record_action`); pressing it pushes `Action::RecordSensors`, which the app hands to `AddonHost::record_sensors`, running Flight Recorder's own Record (with sensors) or Stop. Power & Freq never calls Flight Recorder directly.

**Decisions already taken**

- Design A layout of table P09-Power: four columns, This PC root, group rows with summary, order of the groups, "Speed" instead of "Throughput", name "S.M.A.R.T.", summary "Healthy".
- No Source column: the source sits in the side panel.
- No power plan change nor deletion of duplicate plans: only "Open Windows power options".
- EcoQoS is not a sensor: it stays in Processes.

## Testing Decisions

A good test checks the behavior visible from outside (which value, which text, which state comes out of an input), not how it is built inside.

1. **Unit tests on the pure logic**, without UI or system access, like the existing tests of the drivers and apps modules. Modules tested: telemetry model, statistics accumulator, formatting, raw data parser. Cases: min / max that ignore unavailable readings and NaN; counters without min / max; reading that becomes stale after N intervals; group summary value; group order; effective clock from % Processor Performance and base; sentence "9% above the 4.70 GHz base" and the below-base case; °C / °F conversion; units and separators; decoding of the NVMe health log from a sample buffer (spare, percentage used, data units in TB, power on hours, critical warning → "Healthy" or not); thermal zone temperature from tenths of Kelvin; battery state from a sample structure ("No battery in this PC"); reasons for unavailability (SDK absent, administrator needed, no provider).
2. **Add-on shared contract tests** (from the "Add-on platform" PRD): the add-on appears in the registry with the correct id and `needs`; when off it does not appear in the menu nor in `--page`; after `stop` no sensor-service thread stays alive and no vendor DLL stays loaded; the settings under its id survive turning off and on again; in the Not bought states it shows the locked preview and starts no work.
3. **`#[ignore]` tests on this PC's real data**, like the existing `lists_this_pc`: one that reads a complete frame and prints the tree (expected here: 12 logical processors, base 4.70 GHz, Core Average above zero, Ultimate Performance plan, no battery, no thermal zone, AMD SDK absent, RTX 4080 via NVML, S.M.A.R.T. "needs administrator" without elevation); plus a `--page "Power & Freq" --screenshot` capture to compare with the design A table.

## Out of Scope

- Changing the power plan, deleting duplicate plans, changing power limits or voltages (the page only reads).
- Overclocking, fan curves, LED control.
- Export or logging of the sensors (belongs to Flight Recorder).
- Permanent graphs. Pinning sensors to the Summary tile is now in scope (#193, see "Pinned sensors" above).
- The macOS "Apple Silicon" version of the page (ANE, SoC-only power): the app is Windows only.
- Redistributing the AMD Ryzen Master SDK: the user installs it from AMD.

## Further Notes

Open points:
- Control to reset the statistics: decided in #185, "Reset statistics".
- Whether sensors can be pinned, charted or exported from this page; the last-minute graph in the panel is our addition.
- "Expand all" / "Collapse all" are placed in the header.
- Still to be confirmed: General and Levels (Energy Perf) groups, rows per logical processor with clock and parked, layout of the GPU devices, power and battery rows of This PC, the "Clock / Power / Memory" line on the GPU header, the equivalence Core Average = average effective clock.
- Sample values of the canvas (Min/Max, iGPU Radeon, per-core usage) are placeholders.
- To decide: how to update `--page` with the page name, and whether the shared sensor service should be described in the "Add-on platform" PRD or here.

Decisions after the code review:
- **Known limit: `nvml.dll` stays loaded after stop.** `nvmlInit` locks the DLL in the process: when nobody is watching, the readings stop, the provider calls `nvmlShutdown` and `FreeLibrary`, but the module stays mapped (idle) until the app closes. The owner accepts this limit; the contract test verifies that the providers are released, not the list of the process's modules. ADLX and IGCL, on the other hand, are unloaded.
- **Id `power-freq`.** The id stays `power-freq` (also the settings key, `addons.power-freq`); `power-and-freq` remains an alias accepted by `--page` and by saved pages.
- **Free tier, easy to change.** Power & Freq and Flight Recorder are Free. The tier of each add-on is in a single table, `TIERS` in `src/addons.rs` (one row per id); `Addon::tier()` reads it. To make it paid, change `Tier::Free` to `Tier::Paid` on its row. The other five add-ons are Paid; a test checks that the table covers every registered add-on.
- **The thermal state is an inference.** Windows does not publish a thermal state for desktop apps: we derive it from the lowest `% Passive Limit` of the ACPI zones with our own thresholds: 100 nominal, ≥ 80 elevated, ≥ 50 serious, below that critical. The panel says so in the "Read from" of the reading ("Windows power status, inferred from the ACPI zones' lowest % Passive Limit: ...").
- **This PC › Powers › GPU** is the sum of the Board Power of all the cards that have one (NVML, ADLX, IGCL), integrated ones included; "Read from": "sum of the cards' Board Power". Without any value it stays grey with the Energy Meter reason.
- **AMD cards' optional fields (ADLX).** Graphics Max Clock, Memory Max Clock, Power Limit, Fan (%) and Fan Speed (RPM, AMD cards only) come from what the driver says it supports (`IADLXGPUMetricsSupport`, asked once when ADLX starts): each maximum is the top of the field's range, the Power Limit is the top of the Total Board Power range (of the GPU power range on APUs), Fan % is the current RPM over the top of the fan range. A field the installed driver does not support stays grey with "not supported by the installed AMD driver"; a failed ADLX call says "ADLX did not answer (error N)". NVIDIA and Intel cards are unchanged (no Fan Speed row).
- **NVIDIA states (#188).** NVML cards also show Clocks › Clock Limit Reasons (the clock event reasons bitmask in words: Idle, Application setting, Power cap, Hardware slowdown, Sync boost, Thermal, Hardware thermal, Power brake, Display clock setting; "None" when nothing limits), General › Performance State (P0–P15) and General › PCIe Link ("Gen 4 x16 of Gen 4 x16", live of maximum). Clock reasons use `nvmlDeviceGetCurrentClocksEventReasons`, falling back to the deprecated `nvmlDeviceGetCurrentClocksThrottleReasons` on older drivers. A function the driver lacks greys only its reading ("this NVIDIA driver has no ..."); NVML_ERROR_NOT_SUPPORTED reads "not supported on this card". NVML has no "reliability voltage" bit (that limit is NVAPI's), so it is not shown.
- **S.M.A.R.T.** has its own source ("S.M.A.R.T.", after "Disk counters" in "Sources on this PC", with its real state, for example "needs administrator"). The NVMe health log is read by itself when the app is elevated; only the ATA pass-through of SATA disks waits for consent to the Manual SMART Probe.
- **Intel DTT not read in this build.** Its rows are grey ("reading Intel DTT is not supported yet"); Intel DTT no longer copies the ACPI zones and no renamed zone feeds the package temperature in the Summary tile.
- **AMD Ryzen Master Monitoring SDK (#191).** Read from SDK 3.1 or later: `Platform.dll` (in `bin` under the registry value `HKLM\SOFTWARE\AMD\RyzenMasterMonitoringSDK\InstallationPath`) is loaded only when that key exists, the processor is AMD, the SDK's file version is at least 3.1, the PC is not a virtual machine, the app is elevated and AMD's driver runs; it is uninitialised and unloaded when the add-on stops. `ICPUEx::GetCPUParameters` (at most once a second, as AMD asks) fills CPU Package temperature, package / Cores / SoC power, PPT Limit, Core and SoC voltages and Fabric Clock. The package temperature, when known, joins the line under the band ("Package 71.5 °C"). The reasons on the grey rows and in "Sources on this PC": "needs AMD's add-on" and "AMD's add-on is too old; install the latest" (both with "Get AMD's add-on"), "not available in a virtual machine", "needs administrator" (with "Restart as administrator", which reopens Power & Freq elevated), "AMD's driver is not running", "AMD's SDK did not start", "AMD's SDK found no processor", "AMD's SDK does not support this processor", "AMD's SDK could not read the processor", and "AMD's SDK reports zero" for a zero value. The SDK's C++ interface is called through its vtable as documented in `src/sensors/ryzen.rs`; a device that does not answer as a CPU is reported, not read.
- **Only the vendors this PC has.** A source for hardware the PC lacks is not listed in "Sources on this PC" and shows no rows: an Intel PC sees no AMD Ryzen Master SDK or ADLX line, an AMD PC no Intel DTT or IGCL line, a PC without an NVIDIA card no NVML line (`Provider::for_this_pc`: the CPU vendor for the SDK and DTT, DXCore's cards for the GPU libraries; when DXCore cannot list the cards, NVML stays listed with its reason). A source for hardware the PC has stays listed with its reason (a Ryzen without the SDK still says "needs AMD's add-on").
