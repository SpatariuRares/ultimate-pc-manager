# Flight Recorder add-on

## Problem Statement

When the PC slows down "every now and then", the user wants to record what the CPU, GPU, NPU, media engines, temperatures and memory are doing while they work, and then review the recording at leisure. The app today has a Flight Recorder with six panels (Processor cores, Disk, Network, Processes, Clock speed, Memory), a recording kept only in memory (up to 14,400 samples), saving to CSV in the Documents folder, "Clear", a "Busiest moments" table, and the record buttons in the status bar. A recording cannot be saved and reopened, there is no playback, and the GPU is not shown for engines.

The page must instead show the machine header ("This machine · 12 performance cores • 0 efficiency cores • 14 GPU engines"), a LIVE · Record · Replay… bar, seven telemetry panels (Performance cores, Efficiency cores, GPU cores, Neural Engine, Media Engine, Thermals, Memory) with a cell per core or engine, save recordings as validated JSON trace files, and play them back with a position bar. It must be rebuilt and become an add-on that, when off, neither records nor samples anything.

## Solution

A `flight-recorder` add-on ("Flight Recorder") registered in the registry, with:

- a page in the **design A** layout (board `P10-FlightRecorder`): a band with the machine name and the hardware line, the text explaining Record, a LIVE · Record · Replay… control bar, a status card that changes with the mode (LIVE, RECORDING with time, samples and a safe-limit gauge, REPLAY with date, a "Replay position" bar and "Close Replay"), the 2×3 panel grid (plus Efficiency cores on hybrid processors), and a "Recordings" side panel with format, saving, errors, opening a trace and "Export as CSV";
- a recorder that samples all the series on the page on its own thread while the user works elsewhere in the app, and that saves to a trace file at the end (header + samples from 00:00);
- a trace reader that validates the file before playing it back;
- no background work when nothing is being recorded and the page is closed; nothing at all when the add-on is off.

Prerequisites: the PRD **"Add-on platform"** (registry, settings, licenses, locked preview) and the shared sensor service described in the Power & Freq PRD (for the Thermals panel).

## User Stories

Page, header and LIVE mode

1. As a user, I want to open "Flight Recorder" from the side menu or with `--page "Flight Recorder"`, so that I can reach the page directly.
2. As a user, I want the band title to be the machine name ("This machine" when live, the name of the machine in the trace during playback, "Unknown machine" if missing), so that I know which PC the data comes from.
3. As a user, I want the line "%1 performance cores • %2 efficiency cores • %1 GPU engines", so that I know how many cells I will see.
4. As a user, I want to read "Record captures every series on this page while you work elsewhere in the app. CPU cores are shown individually; GPU, Neural Engine, and Media Engine grids show the hardware engines Windows reports.", so that I understand what Record does.
5. As a user, I want a "LIVE" indicator with a green dot when the page shows live data, so that I know I am not looking at a recording.
6. As a user, I want a "Refresh" button in the header, so that I can reload the list of engines and sensors.

Panels

7. As a user, I want the Performance cores, GPU cores, Neural Engine, Media Engine, Thermals and Memory panels in a 2×3 grid, so that I see all the panels at once.
8. As a user with a hybrid Intel processor, I want the Efficiency cores panel too; on a processor with no efficiency cores, I want it to disappear, so that I don't see an empty panel.
9. As a user, I want each panel to have a title, the current value at the top right, a chart and a grid of cells, one per core / engine / sensor, so that I can see both the whole and the individual element.
10. As a user, I want these colors: cores green, GPU blue, NPU and Media amber, Thermals red, Memory purple, so that I can recognize the panels at a glance.
11. As a user, I want up to three series per panel (solid line: load; dashed: a second series such as effective clock or GPU clock), so that I can see load and speed together.
12. As a user, I want each logical core to have its own cell in the cores panel, so that I can see which core is working.
13. As a user, I want the GPU cells to be the engines Windows reports (3D, copy, video decode/encode, security, OFA, VR, overlay…), so that I can see which engine is busy.
14. As a user, I want the Media Engine panel to show the video encode and decode engines, so that I can see whether a recording or a stream is using the hardware.
15. As a user, I want the Neural Engine panel to show the NPU activity and the NPU detail, and "No NPU in this PC" when there is none, so that I don't think it is an error.
16. As a user, I want the Thermals panel to show one cell per sensor with its name and the hottest point highlighted, so that I can see where it is getting hot.
17. As a user, I want the Memory panel to show memory in use out of the total, so that I can connect slowdowns with full memory.
18. As a user, I want the temperatures to follow the °C / °F setting, so that they are consistent with the app.

Recording

19. As a user, I want to press "Record" ("Start recording") to begin recording, so that it captures everything the page shows.
20. As a user, I want the recording to continue while I go to other pages of the app, so that I can reproduce a problem while I am recording.
21. As a user, I want to see "RECORDING", the elapsed time in HH:MM:SS and "%1 samples", so that I know how long I have been recording.
22. As a user, I want a bar showing how much of the safe capture limit I have used, and at the limit the message "Safe capture limit reached; stop to save the samples collected.", so that I don't lose data without knowing it.
23. As a user, I want the recording to stop collecting at the limit instead of discarding the oldest samples, so that the file contains the start of the problem (today the page discards the oldest).
24. As a user, I want "Stop recording" to end the recording, so that I decide when it ends.
25. As a user, I want stopping to open the Windows Save dialog with a name including the date and time (e.g. `…_20261115_2104`), so that I can save the trace right away.
26. As a user, I want, if I cancel the dialog, the trace to remain as "Unsaved Flight Recorder trace" to save later or discard, so that I don't lose it through a wrong click.
27. As a user, I want the error messages: "The Flight Recorder trace could not be saved. Close and discard the captured trace?" with "Discard" / "Cancel", "A recording already exists at that path.", "The recording is too large to save safely.", "The recording captured no samples.", "The recording folder could not be created.", and those for writing / publishing the file, so that I know what went wrong.
28. As a user, I want "A trace is already being recorded." if I try to start another one, so that a second one does not start.
29. As a user, I want the Record button to be disabled, with a reason, when the update rate is set to Paused, so that I don't record an empty file (our addition).
30. As a user, I want the recording to sample at the app's update rate, so that the data density is the one I chose.

Playback

31. As a user, I want "Replay…" to open the "Choose a Flight Recorder trace to replay" picker, filtered to trace files, so that I can reopen a recording.
32. As a user, I want "The Replay file picker could not be opened." if the dialog does not open, so that I know the fault is not with the file.
33. As a user, I want the file to be checked before playback and rejected with a clear message if it is for another platform, from a newer version, is not valid JSON, is too large ("That trace contains too many samples."), has no samples, does not start at 00:00, has out-of-order times, has invalid values, or has cells that do not match the header, so that a broken trace does not show wrong data.
34. As a user, I want to see "REPLAY", the trace's machine name and "Recorded %1" with the date, so that I know what I am looking at.
35. As a user, I want the "Replay position" bar with start, current and total time, so that I can move through the recording and go to the moment of the slowdown.
36. As a user, I want the sentence "Replaying a recorded run. Nothing on this page is live until the replay is closed." and "Close Replay", so that I can return to live data when I want.
37. As a user, I want "Close the replay before starting a recording." if I press Record during a playback, so that the two do not get mixed up.
38. As a user, I want a trace from another machine to show its own cells (cores, engines, sensors) even if my PC has different ones, so that I can look at traces sent by others.

Export and status bar

39. As a user, I want "Export as CSV" for the open or just-recorded trace, so that I can open it in Excel (our addition, already in the app).
40. As a user, I want the record buttons in the status bar to no longer control the Flight Recorder, so that the Flight Recorder is controlled from its own page.

Add-on behavior

41. As a user, I want to turn Flight Recorder on or off from the Add-ons page, so that it does not take up memory if I don't use it.
42. As a user, I want the off add-on to disappear from the menu and from `--page`, and to neither sample nor record anything, so that there is no hidden work.
43. As a user, I want turning it off during a recording to make the app ask me to save or discard first, so that I don't lose the trace.
44. As a user, I want, with the add-on on, the page closed and no recording in progress, no sampling of the add-on's own to run, so that it costs nothing when it is not needed.
45. As a user, I want turning it back on to find my settings intact (recordings folder, last Replay folder, Summary panel yes/no), so that I don't have to set them again.
46. As a user, I want the Add-ons page to say what the add-on requires (no administrator, no network, disk space for the traces), so that I can decide with full information.
47. As a user, I want a section in the settings with the default traces folder and a Summary panel yes/no, so that I can adapt the add-on.
48. As a user, I want a Summary panel that appears only while I am recording (red dot, time, samples, Stop), so that I can see from any page that I am recording.
49. As a user, if Flight Recorder becomes paid and I have not bought it, I want the locked preview "Sample Flight Recorder preview" with the banner "Flight Recorder is a … feature", a purchase option and "Enter License Key…", so that I know what I am buying; in this state Record and Replay are disabled and nothing runs.
50. As a user on a trial, I want to see how many days are left, so that I don't lose the feature without knowing it.
51. As a user whose trial has expired, I want Replay to remain available for the traces already saved, so that I don't lose the work I have done (our addition, to be confirmed with the owner).

## Implementation Decisions

**Modules**

- **Trace model (pure).** Header: format version, platform, machine name, recording date, performance and efficiency core names, GPU, NPU and Media engine names, thermal sensor names and keys, total memory, interval. Sample: time since the start, series per panel (primary / secondary / tertiary), one value per cell, thermal hotspot, memory in use. Interface: serialize and read JSON; validate (platform, version, size, number of samples, starting at zero, increasing times, finite values within limits, cells consistent with the header), returning an error with its message (stories 27 and 33); default name with date `YYYYMMDD_HHMM`; CSV export; search for the sample nearest to a position.
- **Recorder.** Its own thread that samples at the update rate, reads the app sampler's data (per core, memory) plus the GPU engine counters already read in perfdata (`GPU Engine` per engine type, NPU via DXCore), and temperatures from the shared sensor service. Safe capture limit: a maximum number of samples and an estimated size; at the limit it stops collecting and shows the message. Interface: start, stop (returns the trace), status (time, samples, fraction of the limit).
- **Reader / playback.** Loads and validates a file, exposes the position, and provides the page with the panel values at the chosen time.
- **Page (design A).** Band, LIVE · Record · Replay… bar, mode cards (LIVE, RECORDING, REPLAY), the panel grid with cells, "Recordings" side panel. Replaces `page_recorder` and the `Recorder` / `RecSample` of the pro module, the sampling currently done in the app loop (`on_sample`), and the record buttons in the status bar.
- **File dialogs.** Save and Open with the common Windows dialogs (already present among the Windows functions used by the project), through the common action of the `AddonContext`.

**Format and names**

- JSON trace with header + samples from 00:00. Extension: specific to our app; the final name follows the app's name, which is still open.
- Default name with date and time, in the folder chosen in the settings (default: Documents).

**How it becomes an Addon**

- id `flight-recorder`, name "Flight Recorder", Free tier to start.
- `needs`: no administrator, no network, no external programs.
- `start`: no thread; the LIVE view uses the data the app's sampler already collects, plus the GPU/NPU counters read only while the page is open. The recorder thread starts only with Record.
- `stop`: if it is recording, asks save/discard (through the common dialog), then stops the thread, closes the playback, and frees the trace memory.
- Settings under `addons.flight-recorder`: traces folder, last Replay folder, Summary panel.
- Summary panel visible only while recording.
- Records for other pages (#195): it answers `Addon::sensor_recording` (Running while recording; Locked with the reason while a replay is open, updates are paused or a trace waits unsaved; Ready otherwise) and `Addon::set_sensor_recording`, which runs its own Record (with the sensor interest) or Stop (Save dialog). Other add-ons never call it: their page pushes `Action::RecordSensors`, the app hands it to `AddonHost::record_sensors`, and the host tells the page the state before each draw (`Addon::recording_shown`; Off while Flight Recorder is off). A license lock ("Not bought") does not exist in code yet; when it does, it reports Locked the same way.

**Decisions already taken**

- Design A layout of board P10: a single LIVE · Record · Replay… bar, six 2×3 panels in this order, the colors of story 10.
- "Busiest moments", "Clear", and the Disk / Network / Processes / Clock speed panels removed. CSV kept as an addition, but as an export of a trace, not as the only format.
- The status bar no longer controls the Flight Recorder.

## Testing Decisions

What is checked is the external behavior (what is saved, what is accepted or rejected, what the page shows), not internal details.

1. **Unit tests on pure logic**, without UI or system, like the existing tests of the drivers and apps modules. Modules tested: the trace model and the recorder with a fake source. Cases: JSON round trip of an identical trace; each validation rule returns its message (different platform, newer version, broken JSON, too many samples, no samples, first time not zero, out-of-order times, NaN value, cells beyond the header); default name from a fixed date; CSV with the expected headers and rows; the capture limit stops collection and keeps the start; the position search returns the right sample; Efficiency cores panel absent with zero efficiency cores; HH:MM:SS and "%1 samples" formatting.
2. **Contract tests common to the add-ons**: registry, id and `needs`; off does not appear and has no thread; `stop` during a recording leaves no live thread; settings preserved; Not bought → locked preview, Record and Replay disabled.
3. **`#[ignore]` tests on this PC's real data**: one that lists this machine's cores and engines (expected here: 12 performance, 0 efficiency, 14 GPU engines, no NPU) and records for a few seconds into a temporary file, reads it back and validates it; plus a capture `--page "Flight Recorder" --screenshot` to compare with the design A board.

## Out of Scope

- Video recording of the window.
- Background recording with the app closed or as a service.
- Automatic analysis of peaks ("busiest moments") and annotations on the trace.
- Remote viewing from an iPhone.

## Further Notes

Open questions:
- Sampling interval during recording (decided: the update rate) and the value of the safe capture limit (today 14,400 samples).
- Whether stopping opens the Save dialog right away or the trace stays "Unsaved" for later (decided: dialog right away; the trace stays "Unsaved" if cancelled).
- What the three series of each panel are (proposal: load, effective clock / GPU clock, temperature or power).
- How GPU engines are counted (all engines or excluding the video ones): the canvas shows 14 on the RTX 4080.
- To be confirmed with the owner: Replay of saved traces after the trial expires; the file extension tied to the final app name.
