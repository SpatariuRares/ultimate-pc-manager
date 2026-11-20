# Installed Apps add-on

## Problem Statement

Anyone who uses Ultimate PC Manager wants to know, in one place, which programs are on the PC, how much space they take up, when they were last opened, whether a newer version exists, and to be able to update or uninstall them (even more than one at a time) without going through Windows Settings, the Microsoft Store and the individual installers.

The app already has almost the whole page (design A "updates first", updates with winget, last launch from UserAssist, size measured in the folders, uninstall that waits for all child processes, Modify and Repair for MSI, CSV export). But today:

- it is a fixed entry in the page enum, always present, not an add-on that the user can turn on or off;
- its threads (reading the list, measuring folders, winget) have no way to be stopped, nor a way to discard results that arrive late;
- some functions are not yet complete (side panel rows, loading and empty-list texts, size shading);
- there is no locked preview in case the add-on becomes paid;
- the add-on contract tests are missing.

## Solution

Turn the Installed Apps page into an add-on with id `installed-apps`, registered in the add-on registry (prerequisite: separate PRD "Add-on platform", which provides the `Addon`, `AddonContext`, `Needs` and `Tier` interface, the registry, per-add-on settings, signed licenses and the Free / Trial / Bought / Not bought states).

The add-on keeps the design A layout already built (status band with the answer to "are my apps up to date?", groups by use, list with checkboxes, side panel with installed and offered versions side by side). The remaining work is:

1. **Conversion to add-on**: start/stop of background work, its own settings under its id, menu entry and `--page` only when it is on, no winget / PowerShell / folder measuring when it is off.
2. **Closing the gaps** that emerged from the comparison with the design A canvas.
3. **Locked preview** with sample data, in case the add-on becomes paid.
4. **Tests** on three levels.

## User Stories

### List and columns

1. As a user, I want to see all the applications installed from the uninstall registry (64-bit view, 32-bit view and current-user view), from MSI and from MSIX packages, so that the list is complete. *(exists)*
2. As a user, I want to see Store apps with the readable name taken from the manifest and without the packages hidden from the Start list, so that the list looks like the one in Settings. *(our addition; exists)*
3. As a user, I want to see for each app name, publisher, version, last launch and size in the list, so that I understand at a glance what takes up space and what I don't use. *(reduced columns of design A; exists)*
4. As a user, I want the name not to repeat the version ("Audacity 3.7.8" becomes "Audacity"), so that the list is cleaner. *(our addition; exists)*
5. As a user, I want the columns Application, Publisher, Version, Installed, Estimated size, Scope, Source to all remain available (in the list or in the side panel and in the CSV export), so that I lose no information. *(partial: Installed, Scope, Source are not separate rows of the panel today)*
6. As a user, I want the size to be shaded in proportion to its magnitude, so that large apps stand out. *(missing)*
7. As a user, I want to sort the list by clicking a column header (Application, Last opened, Size), with the active column marked, so that I can find the largest or least-used apps. *(exists for three columns)*
8. As a user, I want to filter by name or publisher with the Find field, so that I can find an app in a list of hundreds. *(exists)*
9. As a user, I want to switch between Updates, All apps and Store apps with the count next to each, so that I immediately see how many apps are in each group. *(our addition; exists)*
10. As a user, I want the page to show Updates when there are updates already checked, otherwise All apps, when it opens, so that I get straight to the useful part. *(our addition; exists)*
11. As a user, I want a Refresh list button, disabled while the list is being read, so that I can re-read after an installation done elsewhere. *(exists)*

### Size and last launch

12. As a user, I want to see the size declared by the app (EstimatedSize) right away and then the size measured in its folder, with the wording "in its folder" or "reported by the app", so that I know how reliable the number is. *(our addition; exists)*
13. As a user, I want the folder of another app contained inside a folder (e.g. a game inside GOG Galaxy) not to be counted twice, so that the sizes are correct. *(our addition; exists)*
14. As a user, I want to see "Measuring app folders, N of M…" with an indicator while folders are being measured, so that I know the number may still change. *(our addition; exists)*
15. As a user, I want to see when I last opened each app (Today, Yesterday, N days ago, N months ago), read from the UserAssist counters without administrator rights, so that I know what I don't use. *(no administrator rights needed; exists)*
16. As a user, I want a hint on the Last opened header explaining that Windows only records launches from Start, the taskbar and Explorer, and that the dash means "never seen opened this way", so that I don't misread missing data. *(our addition; exists)*

### Updates with winget

17. As a user, I want the status band to say "Apps have not been checked for newer versions yet" and explain that the check uses winget, takes a few seconds and does not ask for administrator rights, so that I know what happens before I press. *(our addition; exists)*
18. As a user, I want a Check for updates button that asks winget which apps have a newer version, so that I don't have to check them one by one. *(our addition; exists)*
19. As a user, I want the check not to start on its own when the page opens, so that winget and the network are only used when I ask. *(decision made; exists)*
20. As a user, I want winget's ARP\… and MSIX\… ids to be matched to the right registry entry or package, and the others by name and version, so that the proposed update is the right app's. *(our addition; exists)*
21. As a user, I want to see the updates split into three groups: apps opened in the last 6 months, tools and parts of other programs (never seen opened from Start), apps not opened for more than 6 months ("Consider uninstalling"), so that I can decide whether to update or uninstall. *(our addition, design A; exists)*
22. As a user, I want an "Update these N" button for the group of recently used apps, with a confirmation that lists them, so that I update in one go the apps I really use. *(our addition; exists)*
23. As a user, I want a "Review N" button on the other groups that takes me to the group in the list, so that I can examine them before acting. *(our addition; exists)*
24. As a user, I want to see "Every app winget knows is up to date" when there are no updates, and "Checked today at HH:MM with winget" with "Check again", so that I know how recent the answer is. *(our addition; exists)*
25. As a user, I want the chosen updates to be installed one after another with winget upgrade on the exact id, with the text "Updating X (i of n) — installers may ask for permission…", so that I know where it is. *(our addition; exists)*
26. As a user, I want a final summary "Updated A, B and C. Not updated: X (reason)", with winget's exit codes translated into words (app open, another installation in progress, restart required, cancelled, blocked by policy, version already newer…), so that I understand what went wrong. *(our addition; exists)*
27. As a user, I want a collapsible section "Output of the last winget run" with the last useful lines (without progress bars), so that I can see the detail when something fails. *(our addition; exists)*
28. As a user, I want a clear message if winget is not installed ("Install App Installer from the Microsoft Store…") and if winget does not return a list, so that I know how to fix it. *(our addition; exists)*
29. As a user, I want the list and the winget check to re-read themselves after an update, so that the state shown is the real one. *(our addition; exists)*
30. As a user, I want the side panel to say, for each app, "winget does not know this app…" or "Up to date on winget" or "Not checked for a newer version yet.", so that I don't believe an app is up to date when winget doesn't know it. *(our addition; exists)*

### Ticks and batch actions

31. As a user, I want a checkbox on each row and one in the header to tick all the apps shown, so that I can choose several apps together. *(exists)*
32. As a user, I want a strip of group actions with "Update N with a newer version", "Uninstall N apps…" and "Clear ticks", so that I can act on all the ticked apps. *(batch update: our addition; exists)*
33. As a user, I want the ticks of apps that no longer exist after a re-read to be removed, so that I don't act on vanished apps. *(our addition; exists)*

### Uninstall

34. As a user, I want "Uninstall…" on the single app with the confirmation "The app's own uninstaller will start…", so that I know the program itself decides what to remove. *(exists)*
35. As a user, I want the confirmation "Uninstall N applications?" with "They are removed one after another…", so that I know the uninstallers start in sequence. *(exists)*
36. As a user, I want Store apps to be removed for my account ("Windows will remove this app for your account"), so that no administrator is needed. *(exists)*
37. As a user, I want Windows Installer products to use the standard MSI removal, so that the uninstall is reliable even if the registry entry is odd. *(our addition; exists)*
38. As a user, I want the wait to follow every process started by the uninstaller (which often copies itself into a temporary folder and exits), so that the result arrives only when all the work is finished. *(our addition; exists)*
39. As a user, I want a "Stop waiting" button while an uninstaller is open, so that I can stop waiting without closing it. *(our addition; exists)*
40. As a user, I want the messages for each outcome: "%1 was uninstalled.", "%1 of %2 applications were uninstalled.", "%1 is still installed after its uninstaller closed.", "The uninstaller for %1 was still running after an hour.", "%1 was not uninstalled: the administrator prompt was dismissed.", "Windows could not start the uninstaller for %1", so that I know what happened. *(exists)*
41. As a user, I want Uninstall… to be disabled, with the reason, when Windows marks the app as not uninstallable ("Windows marks %1 as an application that cannot be uninstalled.") or when there is no uninstaller ("%1 did not register an uninstaller with Windows."), so that I don't try an impossible action. *(exists)*
42. As a user, I want "The uninstaller %1 registered is missing." when the uninstaller file no longer exists, so that I understand the entry is orphaned. *(exists)*
43. As a user, I want that after the uninstall the app's disappearance is verified and the list is re-read, so that the result is checked and not assumed. *(exists)*

### Modify, Repair and side panel

44. As a user, I want Modify (ModifyPath or the MSI configuration) and Repair (MSI repair) when the app allows them, with the wait and the outcomes "The setup of X closed.", "Stopped waiting…", "…was still running after an hour.", so that I can repair a program without looking for its installer. *(our addition; exists)*
45. As a user, I want a side panel with name, publisher, installation date, installed and offered versions side by side with the changed part highlighted, and a usage note (in amber if I haven't opened it for more than 6 months, with the space I would free), so that I decide in one place. *(design A; exists)*
46. As a user, I want the panel to show the rows Size, Scope ("Machine: everyone on this PC" / "Current user", plus 32-bit), Installed, Source ("Registry", "MSI", "MSIX"), Registry key, Folder, winget id, as in the design A canvas, so that I find all the fields. *(partial: today there are "For" and "Kind" in place of Scope and Source, and Registry key is missing)*
47. As a user, I want the links Open folder, Website and Copy details (with the confirmation "Details copied"), so that I reach the app's files or site. *(our addition; exists)*
48. As a user, I want "Select an app to see its details. Tick several to update or uninstall them together." when nothing is selected, so that I know how to use the panel. *(our addition; exists)*

### Other actions

49. As a user, I want "More actions" with "Export the list as CSV…" (all the columns of story 5 plus ours) and "Open Windows Settings › Installed apps", so that I can save the list or switch to Windows. *(our addition; exists)*
50. As a user, I want a "Could not save: …" message if the export fails, so that I know the file is not there. *(our addition; exists)*

### Loading, empty states and errors

51. As a user, I want to see a reading state with the text ("Scanning" / "Reading the machine and current-user application inventory."), so that I know the list is arriving. *(partial: today it is "Reading the installed apps…")*
52. As a user, I want the empty state ("No installed applications were reported" / "Windows did not publish any visible uninstall entries.") when Windows returns nothing, distinct from "No app or publisher matches what you typed." and from "No app has a newer version.", so that I understand whether it is a filter or a problem. *(partial: today it is "No apps here.")*
53. As a user, I want to see "You can keep using the app while this runs." during every long operation, so that I know the app is not blocked. *(our addition; exists)*
54. As a user, I want only one operation at a time (check, update, uninstall, modify) to be possible, with the buttons disabled while another is running, so that operations do not overlap. *(our addition; exists)*

### Behaviour as an add-on

55. As a user, I want to turn Installed Apps on or off from the Add-ons page in Settings, so that I keep the app light if I don't need it. *(our addition)*
56. As a user, I want, when it is off, the add-on not to appear in the side menu nor to be reachable with `--page "Installed Apps"`, so that it is truly absent. *(our addition)*
57. As a user, I want, when it is off, no threads, winget, PowerShell (Store list, MSIX removal) or folder measuring to start, so that it consumes nothing. *(our addition)*
58. As a user, I want, when it is turned off, a folder measuring in progress to stop and incoming results to be discarded, so that nothing continues after switching it off. *(our addition)*
59. As a user, I want that if an update or uninstall is in progress the toggle does not turn the add-on off until the operation has finished (or I stop waiting), with a message explaining it, so that an installer is not left half-done without an outcome. *(our addition)*
60. As a user, I want that when I turn it back on I find my settings again (chosen group, sorting, add-on options), so that switching off does not make me lose anything. *(our addition)*
61. As a user, I want the Add-ons page to show, before switching on, what the add-on asks of the system: network and winget (App Installer) for updates, PowerShell for Store apps, administrator rights only when an installer or uninstaller asks for them through UAC, so that I know what I accept. *(our addition)*
62. As a user, I want the app not to have to be started as administrator: when needed, Windows asks with the UAC window, so that the rest of the app runs with normal rights. *(decision made; exists)*
63. As a user, optionally a tile in Summary with "N apps have a newer version" and the date of the last check, shown only after a check I made myself, so that I see the status without opening the page and without automatic checks. *(our addition, optional)*
64. As a user, I want in the add-on's settings section the choice to check winget when the page opens (off by default), so that I decide myself whether to use the network automatically. *(our addition, optional)*

### If it becomes paid

65. As a user without a license, I want to see a locked preview with sample data (full table, status band) and a purchase button, so that I understand what I get before buying. *(to build only if it becomes paid)*
66. As a user without a license, I want the locked preview not to read my registry, not to call winget and not to measure folders, so that nothing runs for a feature I don't have. *(our addition)*
67. As a user on a trial, I want to see how many days remain, so that I know when to decide. *(our addition, from the platform)*
68. As a user who has bought it, I want the license to be verified on the PC without network, so that the page works even offline. *(our addition, from the platform)*

## Implementation Decisions

- **Prerequisite**: PRD "Add-on platform" (`Addon`, `AddonContext`, `Needs`, `Tier` interface, registry, per-add-on settings in `settings.json` under `addons.installed-apps`, signed licenses, locked preview). In the proposed migration order, Installed Apps is the second-to-last, before Drivers.
- **Existing modules**:
  - *app data module* (today apps): types `App`, `Source`, `WingetRow`, `State`; the `Hub` that owns the shared state and starts the threads (list re-read, folder measuring, winget check, update, uninstall, modify/repair, stop waiting); pure functions for parsing and comparison (winget table, winget-app matching, UserAssist-shortcuts-packages matching, rot13, path cleaning, nested folders, command-line splitting, dates); a separate system part (registry, Get-AppxPackage, UserAssist, winget, process start and wait, check that the app is gone).
  - *page module* (today page_apps): `AppsUi` (scope, sorting, search, ticks, selection, confirmations, open groups), status band, list, side panel, dialogs, CSV export; pure presentation functions (age, name without version, group by use, short version, list of names).
- **Dependencies to resolve**: today the add-on uses from the drivers module the PowerShell execution and the CSV field, and from the Drivers page the date, version-splitting, tick and separator helpers. They must be moved into `AddonContext` or into the shared controls, so that turning off Drivers takes nothing away from Installed Apps.
- **New: `InstalledAppsAddon`**, which implements `Addon` and owns `Hub` and `AppsUi` (today fields of `TaskApp`, to be removed from there together with the `Page::Installed` branch and the case in `refresh_now`).
  - `id` "installed-apps", `name` "Installed Apps", `needs`: network, external programs (winget, PowerShell), administrator "on request through UAC".
  - `start`: does not start anything by itself; marks the add-on as active. The first re-read starts at the first opening of the page, as today.
  - `stop`: increments the generation counter (folder measuring already checks it and stops); every thread, before writing its state, checks that the generation is still its own and otherwise discards the result; empties the state and the "started" flag. If an installer/uninstaller operation is in progress, the platform asks the add-on whether it can stop: it answers no while `busy` is not empty, and the Add-ons page shows the reason.
  - `page`: the current design A page.
  - `settings`: "Check for updates when the page opens" (off by default); the 6-month threshold stays fixed.
  - `summary_tile` (optional): number of apps with an update and time of the last check, only if a check happened in this session; no automatic checks.
  - Interface state to keep (scope, sorting) is saved under the add-on's id.
- **Data sources (already used)**: the three uninstall registry views (64-bit, 32-bit, current user) with the Windows Installer fields; Get-AppxPackage in a child PowerShell with the names from the manifest; the UserAssist counters from the registry and the Start menu shortcuts (target and AppUserModelID); `winget list` and `winget upgrade --id … --exact`; ShellExecuteEx with waiting on the process tree; removal of MSIX packages for the account; MsiExec for standard removal, Modify and Repair; folder size read from the file system.
- **Gaps to close** (from the design A canvas):
  - side panel: Scope rows with the values ("Machine: everyone on this PC" / "Current user"), Installed, Source ("Registry", "MSI", "MSIX"), Registry key (the real subkey, e.g. Audacity_is1), in place of the "For" and "Kind" rows;
  - size shading in the list in proportion to magnitude;
  - reading and empty-list texts ("Scanning", "Reading the machine and current-user application inventory.", "No installed applications were reported", "Windows did not publish any visible uninstall entries."), distinct from the filter states;
  - sorting: decide whether Version should also be sortable.
- **Decisions already made**: design A "updates first" layout; last launch from UserAssist instead of SRUM (no administrator); no automatic winget check; one operation at a time; where disk space goes remains on the Disk Space page.
- **Locked preview** (only if `Tier` becomes paid): fixed sample data in the page module, drawn with the same code as the real page but with actions disabled and the platform's purchase banner; no system access.

## Testing Decisions

A good test verifies external behaviour (what goes in, what comes out, what the user sees), not internal details. Three levels, agreed with the owner.

1. **Unit tests on the pure logic**, without UI and without the system. Prior art: the existing tests of the app data module (winget table, winget matching, command line, nested folders, rot13, icon paths, use through shortcuts and packages) and of the page module (age, name without version, groups by use, short versions, lists of names). To add:
   - `uninstall_blocker`, `can_modify`, `can_repair`, `has_update`, `scope_label`, `folder` on hand-built apps;
   - translation of winget codes and filtering of progress lines;
   - update and uninstall summary texts (one, many, all succeeded, some failed);
   - panel values for Scope / Source / Registry key;
   - choice of the empty state (reading, no apps, filter with no results, no updates);
   - intensity of the size shading;
   - CSV export rows (commas and quotes).
2. **Tests of the shared add-on contract** (from the platform PRD), which `InstalledAppsAddon` must pass: it is in the registry with a stable id; when off it does not appear in the menu nor in `--page`; `stop` stops the background work and late results are discarded (with the system part replaced by a fake that is slow); turning it back on keeps the settings; every license state gives the right page, and "Not bought" does not touch the system.
3. **`#[ignore]` tests on this PC's real data**: the existing `lists_this_pc` of the app module (list, last launch, comparison with winget; with `APPS_DUMP` it writes the JSON) and a capture `--page "Installed Apps" --screenshot` after the conversion, to compare with the design A canvas. Real update and uninstall remain a manual check on a test app (not automated: they modify the system).

Modules tested: app data module, app page module, `InstalledAppsAddon`.

## Out of Scope

- The add-on platform itself (registry, Add-ons page, licenses, sales): separate PRD.
- Installing new apps (searching the winget catalogue): the add-on updates and uninstalls what is there.
- Where disk space goes: Disk Space page.
- Last launch from SRUM (requires administrator) and counting launches from outside Start / taskbar / Explorer.
- Cleaning up leftovers after uninstall (remaining folders and keys).
- Phase 2 of the platform (add-ons in a separate process or WASM).
- Renaming the app ("My Task Manager").

## Further Notes

- On this PC 51 apps out of 175 have a last-launch date: this is a UserAssist limit, to be explained (already done in the column hint).
- After the conversion a repeatable manual check of update and uninstall is needed (a test app with an old version), because the automatic tests do not modify the system.
- Open decisions that affect this add-on (from the add-ons document): whether it is free or paid, sold alone or as a bundle.
