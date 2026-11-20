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
