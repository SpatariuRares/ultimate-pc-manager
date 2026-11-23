# Drivers add-on

## Problem Statement

Those who use Ultimate PC Manager want to know whether the drivers on their PC are up to date and to install the newer ones without opening Device Manager, Windows Update and the manufacturers' websites one by one. More expert users also want to see the kernel drivers: which are loaded, how much memory they take up, who wrote them and whether they can be unloaded.

The app already has almost everything: design A "update center" with the status band "are my drivers up to date?", device drivers as in Device Manager, a search on Windows Update matched by hardware ID and compared with the installed version, installation with UAC, INF package, hardware scan, a list of kernel modules with size and load index, Unload, CSV export. But today:

- it is a fixed entry of the page enum, not an add-on that the user can turn on or off;
- its threads (reading the list, the Windows Update search, elevated scripts) cannot be stopped and their results cannot be discarded;
- some Kernel modules features are not complete (four figures as tiles, "%1 shown" and "Updated %1", reading, error and empty-list texts, detail row names, scope from the command line);
- **the installation of driver updates from Windows Update has never been tested end to end on the real system**;
- there is no locked preview in case the add-on becomes paid;
- the add-on contract tests are missing.

## Solution

Turn the Drivers page into an add-on with id `drivers`, registered in the add-on registry (prerequisite: separate PRD "Add-on platform", which provides the `Addon` interface, `AddonContext`, `Needs`, `Tier`, the registry, per-add-on settings, signed licenses and the states Free / Trial / Bought / Not bought).

The add-on keeps the design A layout already built, including the Kernel modules scope. The remaining work is:

1. **Conversion into an add-on**: start/stop of the background work, its own settings, side menu entry and `--page` only when it is on, no PowerShell, Windows Update or pnputil script when it is off.
2. **Closing the gaps** that emerged from the design A canvas.
3. **End-to-end verification** of the installation of Windows Update driver updates, of the INF package, of the scan and of Unload on the real system.
4. **Locked preview** with sample data, if the add-on becomes paid.
5. **Tests** on three levels.

It is the last add-on in the migration order proposed by the add-ons document.

## User Stories

### Status band and Windows Update updates

1. As a user, I want a status band that says "Drivers have not been checked for updates yet" and explains that the check compares each driver with Windows Update, lasts up to a minute and does not ask for administrator rights, so that I know what happens before I press. *(our addition, design A; exists)*
2. As a user, I want a Check for updates button ("Searches Windows Update") that searches the drivers offered for this PC in the Windows Update catalog, so that I do not have to open Settings. *(our addition; exists)*
3. As a user, I want the search not to start on its own when the page opens, so that the network and Windows Update are used only when I ask. *(decision taken; exists)*
4. As a user, I want to see during the search an indicator with "Checking Windows Update for driver updates…" and "You can keep using the app while this runs.", so that I know the app is not blocked. *(our addition; exists)*
5. As a user, I want each offered driver to be matched to the right device by hardware ID or compatible ID (case-insensitive), preferring a connected device, so that the offer really concerns my hardware. *(our addition; exists)*
6. As a user, I want the offered driver to be compared with the installed one (first the version, when formats match; then the date if the version is missing), so that I do not accidentally install an older driver: Windows Update may offer one (seen here: ASIX 4.20.2.0 offered over the installed 4.22.1.0). *(our addition; exists)*
7. As a user, I want to see "N newer drivers are ready for this PC" with up to three lines "name  installed version → offered version", "and N more", and for a single driver "From X, released D. S download from Windows Update.", so that I know what I am about to install. *(our addition; exists)*
8. As a user, I want to click a driver name in the band ("Show this driver") and reach its row in the list, even if it is hidden by a filter or disconnected, so that I can examine it. *(our addition; exists)*
9. As a user, I want an "Install update" / "Install N updates" button with the administrator shield and the note "Windows asks for administrator permission", so that I know the UAC window will appear. *(our addition; exists, to be verified end to end)*
10. As a user, I want the drivers that are offered but no newer to be kept out, with the sentence "Windows Update also offers X V, which is older than the V you have, so it is left out." and an "Install it anyway" link, so that I can go back only if I want to. *(our addition; exists)*
11. As a user, I want the confirmation "Install an older driver?" with "It is not newer than the driver you have. Install it only to go back to that version.", so that I do not downgrade by mistake. *(our addition; exists)*
12. As a user, I want to see "All drivers are up to date on Windows Update" with the note that graphics cards and other devices may have newer drivers on the manufacturer's site, so that I do not believe Windows Update is the last word. *(our addition; exists)*
13. As a user, I want "Checked today at HH:MM" and "Check again", so that I know how recent the answer is. *(our addition; exists)*
14. As a user, I want the installation outcome "Installed N driver updates." or "Installed N; failed: title (0x…)", and the installed drivers to disappear from the list of offers and the list to be read again, so that the state shown is the true one. The elevated script receives the update identity of each clicked offer and looks it up directly; it searches all driver updates again only for an identity that is no longer valid. After a dismissed UAC prompt nothing is installed and the list is not read again. *(our addition; exists, to be verified end to end)*
15. As a user, I want "Restart Windows to finish installing drivers." when Windows requires it, so that I know a restart is needed. *(our addition; exists, to be verified end to end)*
16. As a user, I want a "Could not check Windows Update: …" message if the search fails (service stopped, no network, company policies), so that I know why I have no answer. *(our addition; exists)*
17. As a user, I want a clear message if I closed the UAC window without confirming, distinct from a failed installation, so that I know nothing happened. *(our addition; to be checked, today the error arrives as generic text)*

### Device drivers (Hardware drivers / Windows built-in)
18. As a user, I want to see a driver per device, as in Device Manager, with name, maker, version, date and a Windows Update column ("New: V" or "Up to date"), so that I understand the state of each peripheral. *(our addition; exists)*
19. As a user, I want by default to see only third-party drivers (the ones that get updated) and the Windows ones to be one click away in "Windows built-in", with the count on each tab, so that the list is not swamped. *(our addition, design A; exists)*
20. As a user, I want the drivers grouped by type (Sound, Network, Bluetooth, Mice, keyboards and controllers, Chipset and system, Software components…) with the number beside it, so that I find a driver by family. *(our addition; exists)*
21. As a user, I want identical entries (same name, version and maker) to be folded into one with "N entries folded into one", so that multiple devices do not repeat the row. *(our addition; exists)*
22. As a user, I want "Show disconnected" to see also the disconnected devices, hidden by default as in Device Manager, so that I can find the driver of something I unplugged. *(our addition; exists)*
23. As a user, I want the state under the name when it matters: "Not connected", "Disabled", "Problem (code N)", "Windows Update offers the older V", "N years old: check the maker" (third-party driver 3 years old or more), so that I notice problems. *(our addition; exists)*
24. As a user, I want Windows drivers dated 2006 or earlier to show "With Windows" instead of the placeholder date, so that they do not look old. *(our addition; exists)*
25. As a user, I want to filter by name, maker, manufacturer or class with the Find field ("Driver or maker"), and to see "No driver or maker matches what you typed." if nothing matches, so that I find a driver quickly. *(exists)*
26. As a user, I want the context menu with "Open the driver's folder", "Copy details" and "Search online", so that I can act on a row without selecting it. *(our addition; exists)*
27. As a user, I want a side panel with the selected driver, its state and, if Windows Update offers something, installed and offered versions side by side with the changed part highlighted and "Install update" / "Install V anyway…", so that I decide in one place. *(our addition, design A; exists)*
28. As a user, I want the panel to say "Not compared with Windows Update yet.", "Up to date", "Nothing newer on Windows Update" or "This driver is from X. Windows Update has nothing newer; the maker's site may." with "Search for a newer driver", so that I know what to do next. *(our addition; exists)*
29. As a user with a graphics card, I want "Graphics drivers usually reach X's site before Windows Update." and "Open X driver downloads" pointing to the manufacturer's page, so that I find the most recent driver. *(our addition; exists)*
30. As a user, I want in the panel "Open the driver's folder", "Open in Device Manager" and "Copy details" (with version, date, INF, service, state, instance ID, hardware ID and the Windows Update offer), so that I have all the information for technical support. *(exists)*
31. As a user, I want "Select a driver to see its details and what Windows Update offers for it." when nothing is selected, so that I know how to use the panel. *(our addition; exists)*

### Kernel modules
32. As an expert user, I want a "Kernel modules" tab with the number of registered driver services, so that I reach the full list of kernel drivers with one click. *(exists)*
33. As an expert user, I want four figures as tiles: Drivers ("Registered with Windows"), Loaded ("Linked into the kernel now"), Third-party ("Not shipped by Microsoft"), Kernel memory ("Held by loaded driver images"), so that the main numbers are visible at a glance. *(partial: today a single sentence with the same numbers)*
34. As an expert user, I want the five scopes All, Loaded, Not loaded, Third-party, File system, each with its own count, so that I can narrow the list. *(exists without the counts on the buttons)*
35. As an expert user, I want "%1 shown" and "Updated %1" (time of the last reading) next to the scopes, so that I know how many rows I see and how recent they are. *(missing)*
36. As an expert user, I want the columns Driver, Name, State, Start type, Type, Version, Vendor, so that I see the main properties of each driver. *(exists)*
37. As an expert user, I want to filter by driver name, service name or vendor with the message "No driver name, display name or vendor matches the filter." when nothing matches, so that I know the filter found nothing. *(exists with slightly different text)*
38. As an expert user, I want the details with the labels: Driver, Service name, State, Kind, Start type, Version, Vendor, Load index (with the load order group), Kernel memory, Location, Unload, so that I find every field. *(partial: today the labels are Service, Memory, Group, File, and State/Kind are in the subtitle)*
39. As an expert user, I want to see real kernel memory and load index of the loaded module, read without administrator rights, so that I know how much each driver weighs. *(exists — in the canvas it was a placeholder, now the data is real)*
40. As an expert user, I want Unload offered only when it is possible, with the reason: "Not loaded.", "Not offered: Windows loaded it at boot, and the kernel refuses to stop a driver it started itself.", "Needs administrator rights (Windows will ask); most drivers refuse the request while the system runs.", "Not offered: its configuration could not be read." (start type "Unknown" when Windows does not return the service configuration), so that I do not try an impossible action. *(exists)*
41. As an expert user, I want the confirmation "Unload this driver?" with "Devices that use it stop working until it loads again or Windows restarts.", so that I do not stop a driver by mistake. *(our addition; exists, to be verified end to end)*
42. As an expert user, I want "Open file location", "Open in Device Manager", "Copy details" in the panel and "Copy service name" in the context menu and in the panel, so that I have all the actions at hand. *("Copy service name" today only in the context menu; in the canvas also in the panel)*
43. As an expert user, I want "Select a kernel module to see where it lives, how much memory it holds and whether it can be unloaded." when nothing is selected, so that I know what I will find in the panel. *(our addition; exists)*
44. As an expert user, I want to open the page directly on a scope from the command line (`--drivers-scope all|loaded|not-loaded|third-party|file-system`), so that I can capture or show a specific scope. An invalid value is reported in the status line as "<value> (valid scopes: all, loaded, not-loaded, third-party, file-system)" and the page opens on its saved scope; `--page drivers`, `--page kernel-modules` and `--page modules` (any case) open the page. *(exists)*

### Other actions (More actions)
45. As a user, I want "Install from an INF file…" to add a driver package and install it on compatible devices (with UAC), with the outcomes "Driver package installed.", "The package was added, but no device needed it…", "Done. Restart Windows to finish.", so that I can install a driver downloaded from the manufacturer. *(our addition; exists, to be verified end to end)*
46. As a user, I want "Scan for hardware changes" (with UAC) and the outcome "Hardware scan finished.", so that Windows detects a device I just connected. *(our addition; exists, to be verified end to end)*
47. As a user, I want "Export the list as CSV…" with device drivers, kernel modules and Windows Update offers, and the outcome "Saved …" or "Could not save: …", so that I can share the inventory. *(our addition; exists)*
48. As a user, I want a collapsible section "Output of the last tool" with the output of pnputil / sc, and "Finished with code N. See the output below." when something goes wrong, so that I see the error detail. *(our addition; exists)*
49. As a user, I want "Refresh list", disabled while reading, so that I can read it again after a hardware change. *(exists)*
50. As a user, I want only one job at a time (search, installation, INF, scan, Unload) to be possible, with the buttons disabled while another is running, so that elevated operations do not overlap. *(our addition; exists)*

### Reading, empty and error states
51. As a user, I want the reading state "Asking the service control manager for every driver registered on this machine." in the Kernel modules scope and "Reading the drivers…" in the others, so that I know the list is on its way. *(partial: the first text is missing)*
52. As a user, I want "Drivers could not be enumerated" with the reason when reading fails, instead of a silent empty list, so that I understand it is an error. *(missing: today a reading error gives an empty list)*
53. As a user, I want "No drivers were reported" / "Windows published no driver registrations." when Windows returns nothing, distinct from "No drivers here." and from the filter states, so that I can tell a problem from a filter. *(missing)*

### Security band (spec #11, #32)
A compact read-only "Security" strip under the status band shows Memory integrity (`HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity\Enabled`) and the vulnerable driver blocklist (`HKLM\SYSTEM\CurrentControlSet\Control\CI\Config\VulnerableDriverBlocklistEnable`).
- Each value shows as "On" (1), "Off" (0), "Not set" (no key or value: the page does not guess) or "could not be read" (access denied, another type, any other number), each with its own explaining line; an unreadable value is never shown as off.
- The values are read with normal rights, in process, in the same read as the driver listing; nothing is written and no elevated job is added. When the add-on is turned off the read is dropped and nothing new starts.
- "Open Windows Security" opens `windowsdefender://coreisolation`; the strip says "Read-only: change them in Windows Security."

### Behavior as an add-on
54. As a user, I want to turn Drivers on or off from the Add-ons page in Settings, so that I keep the app light if I do not need it. *(our addition)*
55. As a user, I want the add-on, when off, not to appear in the side menu nor be reachable with `--page "Drivers"`, so that it is truly absent. *(our addition)*
56. As a user, I want, when off, no thread, PowerShell script, Windows Update search, pnputil or sc to start, so that it consumes nothing. *(our addition)*
57. As a user, I want turning it off during a Windows Update search to interrupt the search and discard the result, so that nothing continues after it is turned off. *(our addition)*
58. As a user, I want that, during an installation, a scan or an Unload (elevated scripts), the switch does not turn the add-on off until the work is finished, with a message that explains it, so that a driver installation does not end without an outcome. *(our addition)*
59. As a user, I want that, when I turn it on again, I find my settings again (chosen tab, kernel scope, "Show disconnected", add-on options), so that turning it off does not make me lose anything. *(our addition)*
60. As a user, I want the Add-ons page to show, before turning it on, what it asks of the system: network and Windows Update service for the search, PowerShell, administrator rights via UAC to install, scan and download drivers, so that I know what I accept. *(our addition)*
61. As a user, I want the app not to need to be started as administrator: the list and the search work with normal rights, and only the actions that change the system ask for UAC, so that the rest of the app runs with normal rights. *(decision taken; exists)*
62. As a user, I want optionally a tile in Summary with "N newer drivers" and the time of the last check, shown only after a check I made, so that I see the state without opening the page and without automatic searches. *(our addition, optional)*
63. As a user, I want in the add-on settings section the choice to search Windows Update when the page opens (off by default) and the "check the maker" threshold (3 years by default), so that I adapt the add-on to my use. *(our addition, optional)*

### If it becomes paid
64. As a user without a license, I want a locked preview with sample data (status band, list, panel) and a buy button, so that I understand what I get before buying. *(to be built only if it becomes paid)*
65. As a user without a license, I want the locked preview not to read devices or the kernel and not to contact Windows Update, so that nothing runs for a feature I do not have. *(our addition)*
66. As a user on a trial, I want to see how many days remain, so that I know when to decide. *(our addition, from the platform)*
67. As a user who has bought it, I want the license to be verified on the PC without network, so that the list and the details work even offline. *(our addition, from the platform)*

## Implementation Decisions

- **Prerequisite**: PRD "Add-on platform" (`Addon` interface, `AddonContext`, `Needs`, `Tier`, registry, per-add-on settings in `settings.json` under `addons.drivers`, signed licenses, locked preview). Drivers is the last in the migration order.
- **Existing modules**:
  - *driver data module* (today drivers): types `DeviceDriver`, `KernelDriver`, `DriverUpdate`, `State`; the `Hub` that owns the state and launches the jobs (re-reading, Windows Update search, installation by id, INF package, scan, Unload) with only one "busy" job at a time; pure functions (version from the title, matching offers to devices, version/date comparison, `worth_installing`, `unload_note`, `third_party`, `status`, CSV field); a separate system part (device properties from the configuration manager, driver services from the service control manager with version and vendor of the file, loaded modules from NtQuerySystemInformation, execution of normal or elevated PowerShell scripts that write the result as JSON, file dialogs). The module also exposes PowerShell execution and `is_admin`, used by the Installed Apps add-on: with the conversion these move to `AddonContext` (common action "run elevated") or to a shared module, so that turning Drivers off does not remove anything from Installed Apps.
  - *page module* (today page_drivers): `DriversUi` (tab, kernel filter, search, "Show disconnected", selections, confirmations), status band, grouped and folded device list, device panel, kernel list and panel, dialogs, CSV export; pure presentation functions (type from the class, order of the types, folding, version splitting, dates, "With Windows", years of age, graphics card maker pages). Some of these (dates, version splitting, check mark, separator) are also used by the Installed Apps page: they must be moved into the shared controls.
- **New: `DriversAddon`**, which implements `Addon` and owns `Hub` and `DriversUi` (today fields of `TaskApp`, to be removed from there together with the `Page::Drivers` branch and the case in `refresh_now`).
  - `id` "drivers", `name` "Drivers", `needs`: network (Windows Update), external programs (PowerShell, pnputil, sc), administrator "on request via UAC".
  - `start`: starts nothing by itself; the first reading starts when the page is first opened, as today.
  - `stop`: a generation counter in the `Hub` (today absent, to be added as in Installed Apps); each job checks the generation before writing the state and otherwise discards the result. The Windows Update search (non-elevated PowerShell) is interrupted by terminating the child process. Installation, INF, scan and Unload (elevated, not interruptible by us) block the shutdown until `busy` is empty; the Add-ons page shows the reason. The state is emptied.
  - `page`: the current design A page.
  - `settings`: "Check Windows Update when the page opens" (off by default), age threshold for "check the maker".
  - `summary_tile` (optional): number of newer drivers and time of the last check, only after a check; no automatic search.
  - The interface state to keep (tab, kernel scope, "Show disconnected") is saved under the add-on id.
- **Data sources (already used)**: device properties of the configuration manager (version, FILETIME date, provider, INF, service, hardware and compatible IDs, presence, problem code); driver services from the service control manager and version resources of the files; list of the kernel's loaded modules (size and load index, without administrator on this PC); the COM API of the Windows Update Agent in a child PowerShell to search (non-elevated, 10–60 s) and to download and install (elevated); pnputil to add and install a package and to scan; sc to stop a driver service. Temporary scripts live in the app's jobs folder and are deleted at the end of the job.
- **Gaps to close** (from the design A canvas):
  - four tiles Drivers / Loaded / Third-party / Kernel memory in the Kernel modules scope instead of the sentence;
  - count on each kernel scope, "%1 shown" and "Updated %1";
  - detail labels (Driver, Service name, State, Kind, Start type, Version, Vendor, Load index with load order group, Kernel memory, Location, Unload) and "Copy service name" also in the panel;
  - two further detail rows, shown only when the service has the value: "Load order group" (after Load index) and "Depends on" (services, then groups written "Name (group)"); the State row shows the pending states (Start pending, Stop pending, Continue pending, Pause pending, Paused) besides Running and Stopped; the kinds are Kernel, File system, Recognizer and Adapter;
  - the filter text of story 37 in the kernel scope;
  - the reading, error and empty states of stories 51–53; the system part must be able to return a reading error instead of an empty list;
  - a command-line option for the kernel scope, with the values of story 44;
  - a specific message when the UAC window is closed without confirmation.
- **End-to-end verification to do** (never tried on the real system): installation of an update offered by Windows Update (download, installation, outcome codes 2/3/4/5, restart request, re-reading), installation of an INF package (codes 0, 259, 3010), hardware scan, Unload of a manual-start driver. Any fixes to the installation script or to the interpretation of the outcomes are part of this work.
- **Decisions already taken**: design A "update center" layout; third-party as the default view; Windows built-in and Kernel modules one click away; only the newer drivers go into "Install newer drivers"; no automatic search; one job at a time; driver dates are the device's DriverDate, dates in the INF text are month-day-year.
- **Locked preview** (only if `Tier` becomes paid): fixed sample data (taken from the canvas: Voice Clarity, ASIX), drawn with the same code as the real page with the actions disabled and the platform's buy band; no system access.

## Testing Decisions

A good test verifies external behavior (what goes in, what comes out, what the user sees), not internal details. Three levels, agreed with the owner.

1. **Unit tests on the pure logic**, without UI and without the system. Prior art: the existing tests of the driver data module (version from the title, hardware ID case-insensitivity, version then date comparison, CSV field) and of the page module (version splitting, folding of identical entries, audio types together, readable dates). To add:
   - matching offers to devices (preference for the connected device, offer without device) and `worth_installing` in the three cases;
   - reading of the search and installation JSON (succeeded, succeeded with errors, failed, interrupted, restart required, script error) extracted into pure functions;
   - translation of pnputil codes into texts (0, 259, 3010, others) for INF, scan and Unload;
   - `unload_note` for not loaded, boot/system start, without and with administrator;
   - `status` of devices (not connected, disabled, problem, working, not started) and `third_party`;
   - kernel scope filters and the counts of the four figures;
   - "With Windows" and "check the maker" by age and maker;
   - reading of the command-line scope option (valid and invalid values);
   - CSV export rows.
2. **Shared add-on contract tests** (from the platform PRD), which `DriversAddon` must pass: it is in the registry with a stable id; when off it does not appear in the menu nor in `--page`; `stop` interrupts the search and discards late results (with the system part replaced by a slow fake); turning off is refused during an elevated job; turning on again keeps the settings; each license state gives the right page and "Not bought" does not touch the system; turning off Drivers does not break Installed Apps (which uses the shared PowerShell execution).
3. **`#[ignore]` tests on this PC's real data**: the existing `lists_this_pc` (device and kernel drivers) and `checks_windows_update` (real search, 10–60 s, needs the network), plus `--page "Drivers" --screenshot` captures for each tab and scope after the conversion, to compare with the design A canvas. The real installation remains a guided manual check (it changes the system and asks for UAC): it must be done at least once with a real update, an INF package, a scan and an Unload, and the outcome noted.

Modules tested: driver data module, driver page module, `DriversAddon`.

## Out of Scope

- The add-on platform itself (registry, Add-ons page, licenses, sales): separate PRD.
- Downloading drivers from manufacturers' websites (NVIDIA, AMD, Intel…): the add-on only opens their page.
- Uninstalling a driver package from the driver store, going back to the previous driver (Device Manager rollback), disabling a device.
- Restore points before installation.
- Driver values read through manufacturers' SDKs.
- Phase 2 of the platform (add-ons in a separate process or WASM).
- Renaming the app ("My Task Manager").

## Further Notes

- **Open points**: Open file location, Open in Device Manager and Copy details remain in the details panel. Kernel memory and load index are read from the kernel module list, to be confirmed on a PC without administrator rights different from this one.
- This PC's numbers: 483 drivers, 219 running, 42 file systems, 108 with non-Microsoft vendor; ASIX = service AX88179A, running, manual start, kernel driver, AxUsbEth.sys. Useful as a reference for the `#[ignore]` tests.
- The installation from Windows Update has never been tested from start to finish: it is the main risk of this add-on and must be verified before declaring it finished.
- The driver module today also provides PowerShell, `is_admin` and file dialogs to Installed Apps: the conversion must move them into a shared place.
- Open decisions that affect this add-on (from the add-ons document): whether it is free or paid, sold alone or as a bundle.
