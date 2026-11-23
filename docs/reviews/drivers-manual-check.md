# Drivers: manual check (#68, #69)

Check, on a real system, of the elevated actions on the Drivers page. Each step changes the system and asks for UAC, so someone needs to be in front of the PC. Start the app **without** administrator rights.

For each step, record: OK / KO, plus the text the page shows. Results go in the comments on #68 and #69.

## #68: installation from Windows Update

1. Drivers → **Check for updates**.
   - Expected: "Checking Windows Update for driver updates…" appears, then the list of offers with title and version (or no offers).
2. On an offer newer than the installed driver: **Install update** → UAC **Yes**.
   - Expected: download and installation, then "Installed 1 driver update." or "Installed N; failed: <title> (0x…)".
   - Expected: the installed offer disappears and the list is re-read.
   - If Windows asks for a restart: "Restart Windows to finish installing drivers." appears.
3. Repeat step 2, closing UAC with **No**.
   - Expected: "Cancelled at the Windows prompt: nothing was changed.", with no error and nothing changed.
4. If there is an offer **older** than the installed driver: **Install it anyway**.
   - Expected: the confirmation "Install an older driver?"; with Cancel, nothing happens.
   - This step is optional, because it really installs the older driver.
5. Note the result codes encountered (2 = succeeded, 3 = succeeded with errors, 4 = failed, 5 = interrupted).

## #69: INF package, scan, Unload

6. **Scan for hardware changes** → UAC **Yes**.
   - Expected: "Hardware scan finished.". **Output of the last tool** shows the pnputil output.
7. **Install from an INF file…** with a real package, for example the INF of the ASIX AX88179A network driver downloaded from the manufacturer, or an INF already present in `C:\Windows\INF` (oemNN.inf).
   - Expected:
     - code 0: "Driver package installed.";
     - code 259: "The package was added, but no device needed it…";
     - code 3010: "Done. Restart Windows to finish.".
8. Kernel modules → select a manual-start driver that is loaded (on this PC AX88179A / `AxUsbEth.sys`, if the adapter is connected) → **Unload** → confirm "Unload this driver?" → UAC **Yes**.
   - Expected: "The driver was asked to unload." or "The driver refused to unload; it stays loaded until Windows restarts.".
   - Restart or reconnect the adapter after this step.
9. Check the reasons when Unload is not offered:
   - Boot/System start driver: "Not offered: Windows loaded it at boot…";
   - driver not loaded: "Not loaded.".
10. Try a command that fails, for example an invalid INF.
    - Expected: "Finished with code N. See the output below." and the output in **Output of the last tool**.

## #17: protected elevated runner (cannot be auto-tested)

The automatic tests run the bootstrap with a real, non-elevated PowerShell. These steps check the real UAC path.

11. Run any elevated action (for example **Scan for hardware changes**) → UAC **Yes**.
    - Expected: it works as before (result shown, no error).
    - While the UAC prompt is open, look at `%LOCALAPPDATA%\UltimatePCManager\jobs`: only `job-<random>.ps1` is there, no `.json` result.
12. Script swap: while the UAC prompt is open, open that `job-*.ps1` in Notepad and append a line such as `New-Item C:\Users\Public\swapped.txt`, save, then answer **Yes**.
    - Expected: the action fails with "The administrator script was changed before it ran, so it was not run."; `C:\Users\Public\swapped.txt` does not exist.
    - Folder race (optional): while the prompt is open, run in a normal PowerShell a loop that keeps creating `%windir%\Temp\upcm-*` folders. The job must either run normally or fail with "The protected result folder could not be trusted, so the script was not run."; it must never show a result from a folder you created.
13. Result protection: during a long elevated action (for example Install from an INF file), from a normal (non-elevated) PowerShell run `Get-ChildItem $env:windir\Temp\upcm-*` and try `New-Item $env:windir\Temp\upcm-*\envelope.json`.
    - Expected: the folder can be listed or read, but creating, changing, deleting or renaming a file or the folder itself is denied (Access denied). Try `Remove-Item $env:windir\Temp\upcm-* -Recurse` and `Rename-Item` too.
14. Result forgery: a normal process cannot create `envelope.json` there (denied); a result with a wrong or missing token is shown as "The elevated result was rejected: it does not carry this job's token." (covered by an automatic test).
15. Close UAC with **No**.
    - Expected: still "Cancelled at the Windows prompt: nothing was changed.". After every step above, no `job-*.ps1` is left in the jobs folder. A finished job leaves its `upcm-*` folder in `%windir%\Temp` (only the caller-readable `envelope.json`, because the caller cannot delete it); the next elevated job removes folders older than 30 minutes.

## #27: restore point (needs UAC and System Restore)

1. Page Drivers, "More actions > Install from an INF file...", pick a package. The confirm dialog shows "Create a restore point first (same administrator prompt)", checked.
   - Expected with System Restore on and no point in the last 24 hours: one UAC prompt, then "Restore point created. Driver package installed." (check `rstrui.exe`: a "Ultimate PC Manager: driver change" point exists).
2. Repeat at once (Windows allows one point per 24 hours by default).
   - Expected: one UAC prompt, then "Restore point not created" with "Windows already created a restore point in the last 24 hours." Cancel: "Cancelled: nothing was changed.", nothing installed. Repeat and Continue: a second UAC prompt, then "Driver package installed." with no restore point.
3. Turn System Restore off for C: (System Properties > System Protection) and repeat.
   - Expected: "System Restore is off on drive C:." with the same Continue/Cancel choice.
4. Uncheck the option, confirm, and restart the app.
   - Expected: no restore point step, the same single prompt as before, and the option stays unchecked next time.
5. Same for "Install update" (Windows Update) and "Install ... anyway".

## #30: disable, enable and restart a device (needs UAC)

Use a harmless device, for example a USB device you can unplug or a Bluetooth adapter. Never start with a display, disk or system device.

1. Drivers, Device drivers, right click a harmless device.
   - Expected: a "Device" group with "Disable device" (or "Enable device" if it is already disabled) and "Restart device", each with the shield. The side panel shows the same two buttons under "Device".
2. Choose "Disable device".
   - Expected: a dialog "Disable <name>?" saying "The device stops working until you enable it again.", with "Create a restore point first (same administrator prompt)" checked. Cancel changes nothing and shows no UAC prompt.
3. Confirm with the restore point on, answer UAC **Yes**.
   - Expected: one UAC prompt, then "Restore point created. Done." (or the honest restore point reason and the Continue/Cancel choice, as in #27). The list is read again: the device now says "Disabled" and the panel offers "Enable device".
4. Choose "Enable device", confirm, UAC **Yes**.
   - Expected: "Done." and the device says "Working" again.
5. Choose "Restart device", confirm, UAC **Yes**.
   - Expected: "Done." and the device works again. Close UAC with **No** on another try: "Cancelled at the Windows prompt: nothing was changed.", and the list is not read again.
6. Try the action on a device Windows will not stop (some system devices): choose Restart on a device in use.
   - Expected: "Windows refused: <what pnputil printed last>" or "Restart Windows to finish." when Windows needs a restart.
7. Protected classes: select a Display adapter (and, on a spare PC, a disk drive or storage controller), right click, "Restart device".
   - Expected: the dialog shows in red "This is a display adapter: you may lose your screen or your boot disk until Windows restarts." and the confirm button is off until "I understand, continue anyway" is ticked. The panel says the device asks for a stronger confirmation. "Enable device" on such a device asks no stronger confirmation. Only confirm this on a PC where you can afford a black screen for a moment.
8. Start an Install update (or any other job) and, while the Windows prompt is open, right click a device.
   - Expected: both device actions are greyed out until the job ends.
