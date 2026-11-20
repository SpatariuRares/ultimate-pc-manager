# Installed Apps: manual check (#77)

Repeatable check, on the real system, of update, uninstall, Modify and Repair. The automated tests do not change the system, so these steps are done by hand. Start the app **without** administrator rights: when needed, Windows asks for UAC.

## Test apps

| App | Type | Why |
|---|---|---|
| Notepad++ (`Notepad++.Notepad++`) | registry (EXE installer, machine) | update, uninstall with wizard, "Stop waiting" |
| PuTTY (`PuTTY.PuTTY`) | MSI (machine) | update, Modify, Repair, MSI uninstall |
| Microsoft Sticky Notes (`9NBLGGH4QGHW`) | MSIX (account) | MSIX uninstall, then reinstall |

## Preparation (regular PowerShell)

```powershell
winget install --id Notepad++.Notepad++ --exact --version 8.9.8
winget install --id PuTTY.PuTTY --exact --version 0.83.0.0
```

The two apps stay on an old version. Sticky Notes is already installed; if it is not, use
`winget install --id 9NBLGGH4QGHW --source msstore`.

## Steps and expected results

For each step, record: OK / KO, plus the text the page shows.

### A. Updates

1. Open Installed Apps, then press **Check for updates**.
   - Expected: Notepad++ and PuTTY in the Updates group, with the new version.
2. Select only PuTTY, then press **Update**.
   - Expected: `winget upgrade` starts (UAC if requested) and the summary says one app was updated.
   - Expected: after the re-read, PuTTY shows the new version and is no longer in Updates.
3. Reinstall the old version of PuTTY (preparation command, with `--force`). Then press **Check for updates** and **Update these N**.
   - Expected: batch update and a summary with the count. Any error appears with the winget code translated into words.
4. Repeat step 3, closing UAC with **No**.
   - Expected: that app is listed as not updated in the summary, the page stays usable, and the other apps continue.

### B. Modify and Repair (MSI, PuTTY)

5. Select PuTTY, then press **Modify**.
   - Expected: the MSI configuration opens. When it is closed, "The setup of PuTTY closed." appears.
6. Press **Repair**.
   - Expected: MSI repair (UAC), success result; PuTTY stays in the list.

### C. Uninstall

7. **Registry, single, with Stop waiting.** Select Notepad++, then press **Uninstall** and confirm. When the wizard is open, press **Stop waiting**.
   - Expected: "Stopped waiting…" appears and the wizard stays open.
   - Close the wizard without uninstalling and press **Refresh**: Notepad++ is still present.
8. **Registry, single, to completion.** Repeat Uninstall on Notepad++ and complete the wizard.
   - Expected: the page also waits for child processes, then the app disappears from the list and the summary says it was removed.
9. **UAC closed.** Reinstall Notepad++, then press Uninstall and close UAC with **No**.
   - Expected: unsuccessful result and the app still present, with the page not frozen.
10. **Batch.** Select Notepad++, PuTTY and Sticky Notes with the checkboxes, then press Uninstall and confirm.
    - Expected: removal one after another. Notepad++ goes through the wizard, PuTTY through MsiExec, and Sticky Notes through removal of the package for the account, without UAC.
    - Expected: the summary says "all succeeded" or lists the ones that failed; the three apps disappear after the re-read.

### D. Cleanup

```powershell
winget install --id 9NBLGGH4QGHW --source msstore   # reinstalls Sticky Notes
```

## Results

Report the table of steps (OK / KO + text shown) in a comment on #77. Any fixes that emerge are made within #77.
