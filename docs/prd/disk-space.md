# Disk Space add-on

## Problem Statement

The user wants to know "what are your largest files, and where are they?" and to keep a history of finished scans. Today the Disk Space page shows only a card per drive with a used/free bar: it does not measure folders or files, and has no scan, groups, views, largest-items lists, file types, history, or actions on items. In addition, the redesign of Installed Apps explicitly decided that "where disk space goes" is no longer the job of Installed Apps (which only measures each app's folder to show its size) but of Disk Space: without this add-on that question remains unanswered in the app.

The page is also hard-wired into the `Page` enum and the global state; a full scan of C: on this PC counts over 4 million files and takes more than two minutes, so it must be able to run in the background, be stopped, and above all never start if the user is not using the feature.

## Solution

Turn Disk Space into an add-on in the registry (see the prerequisite PRD "Add-on platform": registry, per-add-on settings, licenses), with stable id `disk-space`, rebuilding the page according to **design A** of the canvas (P14-DiskSpace).

From the user's point of view:

- A **toolbar** with three drop-down menus: **target** (C:\, D:\, …, All disks "N local drives", Choose Folder…), **Group** (File Size / File Type / Folder Size), **View** (Balls / Block), and on the right the chosen target and the green **Start** button. A **Disk Space** menu in the header with the same commands.
- A **status band** that, when a scan is finished, says in one sentence where the space goes (e.g. "DockerSandboxes holds 713 GB, almost half of what the scan counted on C:\"), with largest folder and file, duration, total and number of files, the note if the total exceeds the used space (sparse images counted in full), and the warning note "the total is a floor" if some folders could not be read, with "See up to 8 examples" and Dismiss.
- A dark **canvas**: **Balls** view (packed balls colored by file type, area proportional to size) or **Block** view, with navigation inside folders: breadcrumbs, "Parent", a "this folder" block for loose files.
- Below the canvas, the **Largest 8 files** list (contents of the File Size group).
- A **side panel**, in this order: the drive card (size, bar, used / free), the **File Type** legend (Media, Documents, Code, Archives, Applications, System, Other with color, file count and size), **Largest Folders**, actions on the selected item (Show in File Explorer, Open the folder, Copy path), and **History** of finished scans with "Clear History…".
- A **status strip** at the bottom: a segmented bar of green LEDs for progress, the states "Idle" / "Measuring…" / "Paused" / "Partial" (stopped) / "Finished", "Not scanned" / "Done today at 14:03" / "Partial today at 14:03", and while a drive or All Disks scan runs "about %1 left" (after the first 5 seconds; never for a folder).
- During a scan: progress, Pause / Resume, Stop Scanning with a confirmation and "Don't ask again"; results fill in live and the scan keeps going when you change page or hide the window.
- With the add-on off, the page disappears and no scan can start; a scan in progress is stopped.

## User Stories

### Page and navigation
1. As a user, I want a "Disk Space" entry in the side menu when the add-on is on, so that it opens the page in one click.
2. As a developer, I want to open the page with `--page disk-space` (alias `diskspace`) and capture it with `--screenshot`, so that I can verify it.
3. As a developer, I want to start a scan from the command line with `--scan` / `--scan=<target>`, so that I can test the scan without clicking.
4. As a user, I want a "Disk Space" menu in the header with Start, Pause/Resume, Stop, target, group, view and Clear History…, so that I find all the commands in one place.

### Target
5. As a user, I want to choose any local drive as the target from the drop-down menu, written as "C:\", with its used and free space, so that I know what I am about to measure.
6. As a user, I want the list of drives to update when I connect or disconnect a disk, so that the target is always valid.
7. As a user, I want the "All disks" target with the wording "N local drives", so that I measure the whole PC at once.
8. As a user, I want "Choose Folder…" to open a "Choose a folder to measure" picker with the "Choose" button, so that I measure only one folder.
9. As a user, I want the chosen target to be remembered at the next launch, so that I do not have to select it again.
10. As a user, I want the used/free bar of each drive in the panel card and in the target menu, so that I keep what the page already shows today. (addition kept from the current page)

### Before the scan
11. As a user, I want to see "Nothing measured yet." and "Not scanned" when the target has never been measured, so that I understand I need to start a scan.
12. As a user, I want a veil on the canvas with the invitation "Measure %1" before the first scan, so that the button to press is obvious.
13. As a user, I want the Start button to say what it measures ("Start: measure C:\" or "measure C:\ again"), so that I have no doubts about the target. (design A)
14. As a user, I want the note "Saved to History when it finishes" next to Start, so that I know the result will be kept. (design A)
15. As a user, I want opening the page to show the last finished scan of the target from the history instead of the empty veil, so that I see the results right away without measuring again. (our addition)
16. As a user, I want the page never to start a scan on its own, so that the app does not read millions of files without my asking. (our decision)

### During the scan
17. As a user, I want to press Start to measure the target, so that the scan starts.
18. As a user, I want a progress bar with percentage, files counted and bytes so far (e.g. "64% · 2,581,000 files · 958 GB so far"), so that I know how much is left. (form from design A)
19. As a user, I want the segmented LED bar in the status strip at the bottom, so that I see the progress even when looking elsewhere.
20. As a user, I want Pause and, while paused, Resume with the "Paused" state (and "Pause pending" while the pause is on its way), so that I can free the disk for a moment without losing the work.
21. As a user, I want results to appear live during the scan (the balls fill in as it goes), so that I soon see the large folders.
22. As a user, I want the note "The scan keeps running if you switch tabs or hide the window.", and I want it to be true, so that I can do other things.
23. As a user, I want "Stop Scanning" to ask "Stop scanning %1?" with "The scan is %1 through. Stopping keeps what has been measured so far on screen, but does not save it to history." and the buttons "Stop Scanning" / "Keep Scanning", so that I do not stop a long scan by mistake.
24. As a user, I want the "Don't ask again" checkbox in the confirmation, remembered in the setting `diskSpaceSuppressStopConfirm`, so that I can stop right away if I prefer.
25. As a user, I want to be able to turn the confirmation back on from the Disk Space section of the settings, so that it does not stay hidden forever. (our addition)
26. As a user, I want a stopped scan to stay visible but not go into the history, so that the history contains only complete results.
27. As a user, I want to be asked, when I change target during a scan, whether to stop the one in progress, so that I do not lose one by distraction. (our addition)
28. As a user, I want the scan to run at low priority for CPU and disk, so that the PC stays usable. (our addition)

### Results: groups, views, navigation
29. As a user, I want the "Group:" menu with File Size, File Type and Folder Size, so that I look at the space in the way I need.
30. As a user, I want Group: Folder Size to draw the folders of the current level by size, so that I find the heaviest folders.
31. As a user, I want Group: File Type to draw the file types (Media, Documents, Code, Archives, Applications, System, Other), so that I know what kind of data takes up the space.
32. As a user, I want Group: File Size to draw the largest files, so that I find the individual files to remove.
33. As a user, I want the "View:" menu with Balls and Block, so that I can choose the drawing I read best.
34. As a user, I want the Balls view: packed balls, colored by file type, area proportional to size, the largest in the center, so that I see the proportions at a glance.
35. As a user, I want the Block view: rectangles proportional to size (treemap), so that I have a tidier alternative. (look from design A)
36. As a user, I want to hover over a ball or a block and see name, size and dominant type (e.g. "713 GB · mostly Other"), so that I read the details without clicking. (design A)
37. As a user, I want to double-click a folder on the canvas to enter it (a single click shows its chip, #223), with breadcrumbs (e.g. "C:\ › Users › spata"), so that I can drill down to where the space goes. (our addition)
38. As a user, I want the "Parent" button to go up one level, so that I can go back up.
39. As a user, I want a "this folder" item that collects the loose files of the level, and the "This Folder Only" option, so that the space of files does not disappear among the subfolders.
40. As a user, I want the chosen group and view to be remembered, so that I find the page as I left it. (saved under the add-on id)
41. As a user with the keyboard, I want the canvas to receive focus and to be navigable with the arrows (the selected item shows its chip), Enter (Show in File Explorer), Shift+F10 (the chip's menu, whose "Open the folder" enters it) and Backspace (Parent), so that the mouse is not needed.

### Lists and summary
42. As a user, I want the status band with a sentence on where the space goes (the largest folder and its share of the total), so that I get the answer without reading the drawing. (design A)
43. As a user, I want "Largest folder" and "Largest file" with path and size in the band, so that I immediately know the two main culprits.
44. As a user, I want "Finished today at 21:12 after 2 min 22 s: 1,517 GB in 4,032,654 files.", so that I know when and how much was measured. (design A with real data)
45. As a user, I want a note when the measured total exceeds the used space of the disk ("sparse disk images count at their full size"), so that I do not think it is an error. (our addition)
46. As a user, I want the "Largest Folders" list in the panel (e.g. "Largest %1 folders"), so that I have a list in addition to the drawing.
47. As a user, I want the "Largest %1 files" list below the canvas, with name, path and size, so that I find the largest files.
48. As a user, I want the "File Type" legend with color, number of files ("1 file" / "%1 files") and size for each category, so that I can read the colors of the drawing.
49. As a user, I want the counts "1 file type" / "%1 file types" where needed, so that the sentences are grammatically correct.
50. As a user, I want the lists to update to the level I entered, so that they reflect the current folder.

### Unreadable items
51. As a user, I want the note "%1 folders could not be read, so the total is a floor." (or "1 folder…", or "Some items could not be read, so the total is a floor."), so that I know the total is a minimum.
52. As a user, I want at the end of the scan "Scan finished with skipped items" / "%1 items could not be measured" with "The scan has finished. These items could not be counted, so the total is incomplete.", so that I understand the result.
53. As a user, I want "See up to 8 examples" to show up to eight skipped paths with "Up to eight examples are shown. Some system-protected or unavailable items may still be skipped.", so that I know what is missing.
54. As a user, I want a "Dismiss" to hide the note, so that it does not distract me after I have read it.
55. As a user without administrator rights, I want the scan to work anyway and count protected folders as unreadable, so that I do not have to elevate the app. (our decision)

### Item actions
56. As a user, I want "Show in File Explorer" on the chosen item, which opens File Explorer with the item selected, so that I decide what to do with it.
57. As a user, I want "Open the folder" to open the folder, so that I see its contents. (our addition)
58. As a user, I want "Copy path", so that I can paste the path elsewhere. (our addition, marked)
59. As a user, I want the messages "File Explorer could not reveal this item.", "This item may have moved or been deleted since the scan." and "Item unavailable" when the action fails, so that I know why nothing happened.
60. As a user, I want the same actions in the context menu of the canvas items and of the lists, so that I do not have to go through the panel. (our addition)
61. As a user, I want a link from an item that belongs to an installed app to the Installed Apps page (if that add-on is on), so that I can uninstall it instead of deleting files. (our addition, consistent with the division of tasks)

### History
62. As a user, I want every finished scan to be saved in History (root, date, total, number of files, unreadable), so that I can review it without measuring again.
63. As a user, I want the History section in the panel with the count and, if empty, "Finished scans are saved here.", so that I know what it is for.
64. As a user, I want to click a scan in the history to load it into the canvas and the lists, so that I can compare the disk of today with that of the past.
65. As a user, I want "Clear History…" in red with "Clear the scan history?" and "%1 saved scans will be removed. This cannot be undone." (or "One saved scan…"), so that I do not delete it by mistake.
66. As a user, I want the messages "%1 saved scans could not be read and were left out of History." / "One saved scan…", "The scan history folder could not be read.", and the errors "Couldn't save scan history…" and "Couldn't locate the scan history folder.", so that I know when the history is not reliable.
67. As a user, I want a single saved scan not to exceed 16 MiB (keeping the largest levels and lists), so that the history does not in turn fill the disk.

### Add-on behavior
68. As a user, I want to turn Disk Space on or off from the Add-ons page in Settings, so that I keep only the features I use. (add-on platform)
69. As a user, I want, with the add-on off, the entry to disappear from the menu and from `--page`, and `--scan` to do nothing, so that the app stays clean. (add-on platform)
70. As a user, I want, with the add-on off, no scan or thread to run, and turning it off during a scan to stop that scan (without saving it), so that it does not use the disk for a feature I do not use. (add-on platform)
71. As a user, I want, when I turn the add-on off and on again, the target, group, view, "Don't ask again" and the history to remain, so that I lose nothing. (add-on platform)
72. As a user, I want the Add-ons page to say what Disk Space requires (no administrator, no network, reading the whole disk, space for the history), so that I decide with awareness. (add-on platform)
73. As a user, I want a Disk Space section in the settings (Stop confirmation, number of items in the lists, history folder and how many scans to keep), so that I can adapt the feature. (our addition, except `diskSpaceSuppressStopConfirm`, see story 24)
74. As a user, I want an optional tile in Summary with the free space of the drives and the date of the last scan, so that I see the status without opening the page. (our addition)
75. As a user, I want a discreet notification when a long scan finishes while I am on another page, so that I know the results are ready. (our addition)
76. As a user, if Disk Space becomes paid and I have not bought it, I want a locked preview with sample data (ball canvas, legend, empty History) and the buy button, so that I understand what it offers. (license states from the add-on platform)
77. As a user who has not bought it, I want the locked preview not to read any file and Start to be disabled, so that the preview does not consume resources. (add-on platform)
78. As a user on a trial, I want to see how many days are left and keep the history even after the trial ends, so that I do not lose the data if I buy later. (add-on platform; keeping it is our decision)

## Implementation Decisions

**Prerequisite.** It depends on the PRD "Add-on platform" (`Addon` interface, `AddonContext`, `Needs`, `Tier`, registry, per-add-on settings, licenses, locked preview). In the Disk Space plan it is the second add-on to move into the registry, right after Benchmarks.

**What exists today.** Only the page function that draws a card per drive (name, file system, type, removable, used/free bar with red above 90%, "free of … (% used)") from the drive data of the sampler. The apps module already has a folder measurement (recursive sum of sizes, nested folders of other apps excluded) used only for apps: the traversal logic can be extracted or inspire the new scanner, but Installed Apps remains responsible only for the size of each app. Everything else is missing.

**Modules to build or modify.**

- **Space tree module (new, deep, pure, no UI or system).** A `SpaceTree` built from events "file found (path, size)" and "folder not readable (path)". Interface: incremental insertion; totals per node; children sorted by size; the first N files and folders per node and globally; bytes and counts per category per node; a "this folder" node for loose files; a list of unreadable examples (up to eight) and their count; a snapshot that can be queried while it grows.
- **File type classification (pure).** Mapping extension → category Media, Documents, Code, Archives, Applications, System, Other, from the extension lists in Further Notes; .exe/.msi in Applications and .dll/.sys/.etl in System. Legend colors fixed in this order.
- **Drawing layouts (pure).** Circle packing (Balls, area ∝ size, the largest in the center) and treemap (Block), from a list of (id, size) to rectangles or circles in a given area; deterministic so that it is testable and does not jump between frames during the scan.
- **History format (pure).** Serialization and reading of a saved scan with the fields (id, rootPath, started, finished, totalBytes, fileCount, categoryBytes, categoryCounts, unreadableCount, folders, files), with pruning to stay under 16 MiB; tolerant reading that counts unreadable files instead of failing.
- **Scanner (new, with system).** Background thread owned by the add-on: folder traversal with the Windows file enumeration APIs (FindFirstFileEx with the fast-search options and large buffers), without following junctions and symbolic links (so as not to count twice; cloud-file, deduplication and container-isolation folders are measured, since what they hold locally is on the disk), allocated size per file with each hard-linked file counted once (see Further Notes); sending events to the `SpaceTree` in batches; states Idle / Running / Pause pending / Paused / Stopping / Finished / Stopped; progress estimated from the used space of the volume (or from the total of the chosen folder, not known in advance: in that case only files and bytes so far are shown); low priority for thread and I/O. "All disks" = a virtual root with the fixed local drives as children.
- **History archive (new, with system).** Folder of the scans next to the app's settings (`DiskScans` under the app folder), one file per scan, listing, loading, total deletion.
- **Disk space page (new, replaces the current feature).** Design A: toolbar with three drop-down menus and Start, Disk Space menu in the header, status band, Balls/Block canvas with breadcrumbs and Parent, Largest files below, side panel (drive card with the already existing bar, File Type legend, Largest Folders, actions, History), status strip with LEDs, Stop dialog, skipped-items note.
- **Item actions.** "Show in File Explorer" with the item selected (shell API to open the folder and select), "Open the folder", copy of the path; existence check first, to show "This item may have moved or been deleted since the scan.". They use the common actions of the `AddonContext`. "Show in File Explorer" hands the shell the plain path, never the `\\?\` long-path form (the shell rejects it); for a path the shell cannot resolve, its 8.3 short form is tried.
- **App shell.** Remove `Page::DiskSpace` and its `match` branch; register the aliases `disk-space` and `diskspace` and the `--scan` flag through the add-on.

**How it becomes an `Addon`.**

- id `disk-space`, name "Disk Space", description "What are your largest files, and where are they?".
- `needs`: no administrator (protected folders become "unreadable"), no network, no external program; a warning that it reads the whole disk and saves the history.
- `tier`: to be decided in the platform; with "Not bought" the page shows the preview with sample data, Start disabled, no reading.
- `start`: loads only the history list (small); **it does not start scans**. `stop`: stops a scan in progress without saving it, closes the thread, frees the tree in memory.
- `settings`: target, group, view, `diskSpaceSuppressStopConfirm`, size of the lists, how many scans to keep; kept under the id even when off.
- `summary_tile` (optional): free space per drive and last scan.

**Decisions already taken.** Canvas layout design A of P14: three drop-down menus instead of segmented controls, "Start" button, Balls canvas colored by type, panel in the order drive card / File Type / Largest Folders / History, status strip with LEDs, "Idle", "Not scanned", paths written as "C:\", category "Code". No file deletion on Windows (the page only shows the item in File Explorer); on Linux the only removal is Move to trash, with a confirmation (see the Linux section). No link to Windows Storage settings and no installed-app sizes on this page (they were inventions of the previous mock; apps remain in Installed Apps). Used/free bar and Copy path are our kept additions.

### Linux (spec #41, decided in #75)

- Drives are the mounted real filesystems (no proc, sysfs or other pseudo filesystems), one entry per device: when the same filesystem is mounted more than once (bind mounts, several btrfs subvolumes of one filesystem), only the mount closest to `/` is listed, so the same space is never shown twice.
- A scan always stays on the filesystem it started on; there is no option to cross into other mounts. The boundary is a **mount point** in the mount table, not a different device number: a folder that is a mount point (another filesystem, a bind mount of the same one, or a pseudo filesystem such as `/proc`) is not entered, while a nested btrfs subvolume that is not mounted has a device number of its own but is part of the same filesystem and is scanned. A mount that is not entered can be scanned by choosing it as its own drive. If the mount table cannot be read, a folder on another device than its parent is not entered instead.
- The action that shows an item in the file manager is worded **Open location** (it opens the folder holding the item with `xdg-open`); on Windows it stays "Show in File Explorer".
- **Move to trash** is offered on Linux only (through the shared availability rule, so it is hidden where it cannot work). It asks first, naming the item and its size, then runs `gio trash` (the FreeDesktop trash of the item's own drive, so it can be restored). It is never offered for the scanned folder or a drive's root. It does not delete anything for good. The scan's results are not changed afterwards: the message says "Rescan to update the sizes."
- Sizes are allocated blocks, hard links are counted once (device and inode), and folders the user cannot read are counted as skipped.

## Testing Decisions

Good tests check external behavior (what a function returns given an input, what the add-on does when it is turned on or off), not internal details.

1. **Unit tests on the pure data logic**, without UI or system access (prior art: the unit tests in the drivers and apps modules):
   - `SpaceTree`: node and root totals after insertions in any order; sorted children; first N files and folders; the "this folder" node; counts and up to eight unreadable examples; "the total is a floor" when there is at least one;
   - extension classification into the seven categories, uppercase and lowercase, files without extension = Other;
   - formatting: sizes, "1 file" / "%1 files", "1 file type" / "%1 file types", "1 folder…" / "%1 folders…", scan duration, Stop confirmation text with percentage, Clear History text with "One saved scan…";
   - Balls and Block layouts: no overlap, area proportional to size within a tolerance, identical result for identical input;
   - history: round trip of a scan, pruning under 16 MiB, corrupt file counted as unreadable;
   - scanner state machine (Start, Pause pending, Paused, Resume, Stop, Finished) driven with fake events: only Finished produces a history entry.
2. **Shared add-on contract tests** (defined by the platform), which Disk Space must pass: it is in the registry with id `disk-space`; when off it does not appear in the menu nor in `--page`; `start` starts no scans; `stop` during a scan stops it without saving and leaves no live threads; with license "Not bought" there is no reading and Start is disabled; settings survive turning off and on again.
3. **`#[ignore]` tests on this PC's real data**, like the existing `lists_this_pc`: scan of a real, not small folder (e.g. the user profile or the project folder) verifying total > 0, a largest file present, no junction followed twice, at least one non-empty category; a second test that saves and reloads the scan from the history folder; plus a capture with `--page disk-space --screenshot` (and one with `--scan` on the test folder) to compare by eye with design A.

Modules tested: space tree, type classification, drawing layouts, history format, scanner state machine (level 1); Disk Space add-on via contract tests (level 2); scanner and history archive on this PC (level 3).

## Out of Scope

- Deleting, moving or compressing files from the app (only "Show in File Explorer"; on Linux also Move to trash, which asks first and never deletes for good).
- Installed app sizes and uninstalling: they remain in Installed Apps (at most a link).
- Link to Windows Storage settings / Storage Sense.
- Disk activity lights: they belong to the notification area icon and to Performance › Disk.
- Search for duplicate files, cleanup of temporary files: candidates for future add-ons (cleanup).
- Network drives and removable disks in "All disks" (only fixed local drives; removable ones remain selectable one by one).
- Direct reading of the NTFS MFT for faster scans (requires administrator; to be evaluated in the future).
- Separate distribution of the add-on (phase 2 of the plan).

## Further Notes

Decisions and open points:

- **Selection chip**: the chip on the canvas. Clicking a ball or block shows a rounded box at the click point with the item's name, a second line (files / type), its size and its path; clicking the chip reveals the item in File Explorer after an existence check; right-clicking it offers "Show in File Explorer" and "Copy path"; it is cleared when a scan starts, by clicking empty canvas and by changing level (spec #223). In addition, the right-click menu is the lists' item menu (it also has "Open the folder"), and with the keyboard the selected item shows its chip, Enter reveals it and Shift+F10 opens its menu (egui has no context-menu key).
- **This Folder Only**: "This Folder Only" is the last entry of the target menu (re-targets the last chosen folder); "this folder" is also the fallback name in "Stop scanning %1?". Navigation inside folders uses the breadcrumb (story 37).
- **Physical or logical size** (decided, #80): the allocated size of each file is counted (the space actually occupied on the disk), read per file with the logical size as fallback, not from the folder listing. Each hard-linked file is counted once per volume (volume serial + file ID); after 262,144 tracked links further links count normally and "Hard-link tracking limit reached. Scan a smaller folder." is shown. Junctions and symbolic links are not followed; folders that are cloud files (OneDrive and other sync providers), deduplication or container-isolation reparse points are measured (their local content is on the disk). One worker thread; the view is updated at most every 500 ms.
- **Categories** (decided, #80): .exe/.msi in Applications; .dll/.sys/.etl in System; **.pf falls in Other**. The extension lists: Media (png jpg jpeg gif heic heif tiff tif bmp webp svg raw cr2 nef dng psd mp3 m4a aac wav aiff flac ogg opus mp4 m4v mov avi mkv webm wmv flv), Documents (pdf doc docx pages txt rtf md odt xls xlsx numbers csv tsv ods ppt pptx key odp epub mobi), Code (swift c h cpp cc hpp m mm rs go py rb js ts jsx tsx java kt cs php sh zsh bash pl lua sql html css json xml yml yaml toml plist cmake), Archives (zip tar gz bz2 xz 7z rar dmg iso pkg jar war aar cab zst), Applications (app framework bundle xcappdata exe msi), System (dylib so a o kext log cache db sqlite sqlite3 shm wal swp tmp dll sys etl), Other (the rest).
- **"Open the folder"** (decided, #80): navigates inside Disk Space (enters the folder, or the folder that contains the file); File Explorer opens only with "Show in File Explorer".
- **Stop confirmation** (decided, #80): for the scan of a folder the text says "partway" instead of the percentage; "Don't ask again" also applies to the confirmation when changing target during a scan. The texts "Stopped while measuring %1…" and "Stopped scans are not saved to History." remain. Changing target during a scan is kept as an open product decision.
- **Strings added by #223**: "about %1 left" (time estimate while a drive scan runs), "It is readable only by the system." / "They are readable only by the system." (skipped-items explanation), "Hard-link tracking limit reached. Scan a smaller folder.".
- **Choices from #92**: the warning in the status band only for scans of at least 10 s that finish while the page is hidden; "Keep only the latest" proposes 50 scans; the length of the lists goes from 3 to 25 (8 as the starting value).
- **History folder** (story 73): "History folder" setting with Choose… and Reset to default (`%APPDATA%\UltimatePCManager\DiskScans`).
- **Locked preview / trial (stories 76–78)**: postponed to the future spec on the platform licenses.
- **Content of the "Disk Space" menu** in the header (#223): &Start, &Pause / &Resume, S&top, &Balls View, Bloc&k View, Clear History...
- Placeholder numbers in the design: 37 unreadable, 64% progress, history entries different from today's.
- Real data used (15 Nov 2026, this PC): C: 948.5 GB, 121.9 GB free; D: 5 GB, 2.9 GB free; scan of C: 4,032,654 files in 142 s; largest folder AppData\Local\DockerSandboxes 713.4 GB.
- Relationship with Installed Apps: the redesign of Installed Apps moved the analysis of "where the space goes" here; Installed Apps continues to measure only the folder of each app.
