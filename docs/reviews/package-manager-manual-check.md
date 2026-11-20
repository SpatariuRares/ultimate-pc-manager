# Package Manager — manual check (#120)

Checks on a real system that the automated tests do not cover. Run the app
**not** as administrator (the app never starts elevated). Write the outcome of
each step on issue #120.

Already verified on the development PC (2026-11-18, winget 1.29.380):
search, single install, final `winget list` check, `winget export` listing,
and uninstalling a machine-wide MSI (junegunn.fzf) through the page path,
which falls back on the MSI's own UAC prompt
(`real_page_uninstall_of_a_machine_wide_msi_falls_back_on_its_prompt`).

Logs: `%LOCALAPPDATA%\UltimatePCManager\package-manager\logs`; journals in
`%APPDATA%\UltimatePCManager\package-manager\journal`. winget's own logs:
`%LOCALAPPDATA%\Packages\Microsoft.DesktopAppInstaller_8wekyb3d8bbwe\LocalState\DiagOutputDir`.

## A. Closing in the middle of an install

Use a package that takes a while to install (for example
`Microsoft.VisualStudioCode` or `Mozilla.Firefox`) plus two small ones.

| # | Step | Expected |
|---|------|----------|
| A1 | Tick 3 packages, **Install selected (3)**. While the first one installs, close the window with X. | "Close the app?" asks first: "A series of 3 is running (0 of 3 done). Closing lets the current package finish and starts no other; …". |
| A2 | Choose to keep running. | The series carries on; nothing was stopped. |
| A3 | Close again during the next package, and confirm. | The app stays until that package ends, then closes. The third package does not start. |
| A4 | Check with `winget list --id <id> --exact`. | The package that was running is fully installed; the next one is not. |
| A5 | Open the package's log (log folder above). | The log is complete: it ends with winget's final line for that package, not cut off. |
| A6 | Start the app again. | The status bar shows "Package Manager: the last run was interrupted (N of 3 done)" on every page; the Package Manager page shows "The last run was interrupted: N of 3 done." with **Continue** and **Ignore**. |
| A7 | **Continue**. | Only the packages not done run again; the summary covers them. |
| A8 | Repeat A1 and A3 with File › Exit and with Alt+F4. | Same as A1–A3. |

## B. A crash in the middle of an install

| # | Step | Expected |
|---|------|----------|
| B1 | Start a series of 3 as in A1. During the first package, end the app from Task Manager (End task on Ultimate PC Manager). | winget and the installer keep running on their own and finish (watch them in Task Manager). |
| B2 | Check with `winget list --id <id> --exact` once the installer has gone. | The package is installed. |
| B3 | Open its log. | The log is complete up to winget's last line, even though the app was gone. |
| B4 | Start the app again. | The interrupted-run offer appears (as A6). The package that was running when the app died counts as interrupted. |
| B5 | **Continue**. | The interrupted one is checked or reinstalled (winget reports "already installed" and it reads as skipped); the others install. |
| B6 | Do B1 again and then **Ignore** at the next start. | No install runs; the offer does not come back at the following start. |

## C. A real series from a bundle on a clean machine

On a clean Windows VM or PC (fresh install, signed in with a local account).

| # | Step | Expected |
|---|------|----------|
| C1 | Turn Package Manager on in Settings › Add-ons and open its page. | Readiness check: winget is ready, or the page says which of "not installed", "not registered", "too old" applies and offers its fix. If a fix runs, the page checks again and then shows ready. |
| C2 | On the development PC, **Create a bundle from the installed apps**, keep 10–20 packages, add one entry by hand with a link (for example a program winget does not have), set a version on one entry and scope *machine* on two, then **Save the bundle…**. Copy the `.upmbundle` file to the clean machine. | The file saves; reopening it there shows the same entries, versions, scopes and the manual entry. |
| C3 | On the clean machine, **Open a bundle…** and pick the file. | Entries of a catalog this PC does not have read "catalog not available"; every other entry can be ticked. |
| C4 | **Install selected**. | Entries that need administrator rights (scope *machine*) run first, so their UAC prompts come at the start; then the rest run without asking. |
| C5 | Close one UAC prompt with **No**. | That entry reads as cancelled; the series continues with the next. |
| C6 | Wait for the end. | The summary groups the outcomes (installed, restart needed, skipped, cancelled, failed, not started) with winget's words and raw codes; the manual entry reads "To install by hand: … (link)" with **Open the site**. |
| C7 | **Retry the failed ones** (if any failed). | Only the failed ones run again. |
| C8 | **Open the log folder**. | One log per package. |
| C9 | **Export for winget…**, then on another clean machine `winget import <file>`. | winget installs the same packages; the manual entry is not in the file. |

## Outcome

Note on #120, per step: passed, or what happened instead (with the log and the
exit code shown on the page).
