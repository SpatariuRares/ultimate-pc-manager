# Benchmarks add-on

## Problem Statement

The user wants to know "how fast is my PC?" and to compare it with earlier runs or with a reference: after a driver update, a change of power plan, a new disk, or a slow connection. The app today has a page with three tabs (CPU, Memory, Disk), each with "Run", a progress bar, and results in "points", GB/s and MB/s; the results stay only in memory and are lost on close, the disk test always uses the temporary folder, and there are no GPU or Internet tests, no score, no history, no "Run All", and no cancellation.

The page needs instead a sidebar of tests (CPU, GPU, Disk, Internet and a **Score** entry), a view for the selected test with **two analog gauges** with a digital readout (for CPU: "Single core" in Mops/sec and "Multi core" in Gop/sec), an LED progress bar and a single **Start** button, "Run All Benchmarks" with its phases ("Testing one CPU core", "Testing all CPU cores", "Testing GPU compute", …), cancellation, a test folder chosen by the user, an Internet test against speed.cloudflare.com, a score with base 100, and a history saved to file with "Clear History…". The page must be rebuilt and become an add-on that, when off, does nothing.

## Solution

A `benchmarks` add-on ("Benchmarks") registered in the registry, with:

- a page in the **design A** layout (board `P15-Benchmarks`): a status band with "Run All Benchmarks" (or, while running, "Running all benchmarks: CPU done, now testing GPU graphics" with "Cancel") and the score notes; a sidebar with CPU, GPU, Disk | Internet | Score (see below) and Memory as an addition; a view of the selected test with two gauges, an LED bar and Start; a side panel with the test history ("CPU history", columns Single core / Multi core / score / version), "Clear History…" and "If a test fails";
- a benchmark engine that runs one test at a time on its own thread, with phases, progress and cancellation;
- the missing tests: GPU (compute and Direct3D 11 graphics) and Internet (download and upload);
- a score with base 100 and a persistent history in a JSON file;
- no background work when no test is running; nothing at all when the add-on is off.

Prerequisite: the PRD **"Add-on platform"** (registry, settings, licenses, locked preview). It is the first add-on to move into the registry according to the add-ons proposal (the most self-contained one), so it will serve as the example for the others.

## User Stories

Page and navigation

1. As a user, I want to open "Benchmarks" from the side menu or with `--page "Benchmarks"`, so that I can reach the page directly.
2. As a user, I want a "Benchmarks" menu in the menu bar when I am on this page, with the same actions (Run All, Start, Cancel, Clear History…), so that I can use them from the keyboard.
3. As a user, I want a "Refresh" button in the header, so that I can reload the history and hardware information.
4. As a user, I want a sidebar with CPU, GPU, Disk, a separator, Internet, a separator and Score, so that I can choose which test to look at.
5. As a user, I want the Memory entry too, at the bottom of the sidebar and marked as an addition, so that I don't lose the test the page already has (our addition).
6. As a user, I want each sidebar entry to show its last result and status (never run, running, completed today at 21:30, failed), so that I can see everything without opening each test (our addition, design A).
7. As a user, I want the keyboard (Tab) to get from the sidebar to the Start button, so that the page is usable without a mouse.

Test view

8. As a user, I want the title ("CPU Benchmark") and a line with the hardware being tested (e.g. "AMD Ryzen 5 7600X · 6 cores, 12 threads") and the time it was run for the selected test, so that I know what it measures.
9. As a user, I want two analog gauges with a digital readout, one per figure (CPU: Single core in Mops/sec and Multi core in Gop/sec), so that I can read the result at a glance.
10. As a user, I want an LED progress bar and the name of the current phase, so that I know where it is.
11. As a user, I want a single "Start" button below the gauges, so that I can start the selected test.
12. As a user, I want "Test complete" at the end, so that I know the result is final.
13. As a user, I want the GPU, Disk and Internet views to use the same two-gauge scheme (Compute / Graphics, Write speed / Read speed, Download / Upload), so that the page is consistent.

CPU and memory tests

14. As a user, I want the CPU test in two phases, "Testing one CPU core" and "Testing all CPU cores", with Single core and Multi core results, so that I see both the speed of a single core and of the whole processor (the workload is our own).
15. As a user, I want the memory test (copying a 256 MB block eight times) with a result in GB/s, marked as an addition, so that I can compare RAM too (our addition, already in the app).

GPU tests

16. As a user, I want a Direct3D GPU compute test ("Testing GPU compute"), so that I measure the computing power of the graphics card (cs_5_0 compute shader: eight independent multiply-add chains per thread).
17. As a user, I want a GPU graphics test ("Testing GPU graphics"), so that I measure drawing performance (vertex + pixel shader with a 64-step loop).
18. As a user with more than one GPU (Radeon iGPU + RTX 4080), I want to know which card was tested and be able to choose it, so that the result is not ambiguous (our addition).
19. As a user, I want the GPU error messages ("The Direct3D benchmark shaders could not be compiled.", pipeline or resources not created, "Direct3D compute work failed."), so that I know why it did not work.

Disk test

20. As a user, I want the disk test to write and then read a test file, with "Writing test file" / "Reading test file" and results Write speed / Read speed in MB/sec, so that I can measure the disk.
21. As a user, I want to choose the test folder ("Test folder", "Choose a folder on the drive to benchmark…") and have it remembered, so that I can test a disk other than C: (setting `diskBenchmarkFolder`).
22. As a user, I want to see the free space under the test folder and the message "Not enough free space to run the test." if there isn't enough, so that I don't fill the disk.
23. As a user, I want the test file (`.…-disk-benchmark-*`) to always be deleted, even if I cancel or the test fails, so that no junk is left behind.
24. As a user, I want the disk error messages (create / write / flush / rewind / read failed, "Windows could not locate the temporary folder."), so that I understand the problem.
25. As a user, I want the test not to read from the Windows cache, so that the read result reflects the disk itself (our addition: today the page reads right after writing, probably from the cache).

Internet test

26. As a user, I want the Internet test with "Testing download speed" and "Testing upload speed" and results in Mbit/s, so that I know how fast my connection is.
27. As a user, I want the note "Uses speed.cloudflare.com", so that I know which service the app connects to.
28. As a user, I want the network error messages (timeout, connection refused, "HTTP %1", TLS or proxy problems, "Unable to connect to the speed-test service."), so that I can tell whether the problem is my network or the service.
29. As a user, I want the Internet test to have no score, only Mbit/s, so that I don't confuse it with the power of the PC.
30. As a user, I want the Internet test to start only when I ask for it, never on its own, so that it doesn't use data without my knowing.

Running all tests and cancellation

31. As a user, I want "Run All Benchmarks" to run CPU, GPU, Disk and Internet in sequence and then update the score, so that one click gives me the full picture.
32. As a user, I want the band during "Running all benchmarks" to say which test has finished and which is running, so that I know how much is left.
33. As a user, I want "Cancel" with the states "Cancelling" and "Cancelled", so that I can stop a long test.
34. As a user, I want the other Start buttons disabled during a test, so that two tests don't interfere with each other.
35. As a user, I want a "Close heavy apps" hint with the current CPU usage, so that I know when the result may be skewed (our addition, design A).
36. As a user, I want a failed test to show "%1 test failed: %2" and leave previous results intact, so that I don't lose data.
37. As a user running a debug build, I want the warning "You are running a debug build, benchmarks are not valid", so that I don't save skewed numbers.

Score

38. As a user, I want a score for CPU, GPU and disk with base 100 relative to a reference machine, and the sentence that says which machine that is, so that I can tell whether my PC is faster or slower than the reference (the reference machine is our own).
39. As a user, I want the "Score" entry in the sidebar with the overall score and the per-test scores, with a bar marking the base's 100, so that I can see it in one place.
40. As a user, I want the note "Score comparisons are valid only per version.", so that I don't compare numbers that cannot be compared.

History

41. As a user, I want every completed run saved to a history file with date, results, score and app version, so that I can find my results after closing the app (file `benchmark-history.json`, ISO date).
42. As a user, I want the history of the selected test ("CPU history") with that test's columns, so that I can see how it changes over time.
43. As a user, I want the text "Benchmark runs will appear here as they complete." when a test has no runs, so that the empty section makes sense.
44. As a user, I want "Clear History…" to ask "Clear CPU benchmark history?" before deleting, so that I don't lose the history by mistake.
45. As a user, I want a corrupted history file not to block the page but to be set aside with a warning, so that I can keep using the tests.

Add-on behavior

46. As a user, I want to turn Benchmarks on or off from the Add-ons page, so that I can keep the menu clean if I don't need it.
47. As a user, I want the off add-on to disappear from the menu and from `--page`, and no test to run, no connection to Cloudflare and no test file to be created, so that there is no hidden work.
48. As a user, I want turning it off during a test to cancel the test and delete the test file, so that nothing is left half-done.
49. As a user, I want the add-on on with no test running to run nothing in the background, so that it costs nothing.
50. As a user, I want turning it back on to find my test folder, chosen GPU, selected test and history intact, so that I don't lose anything.
51. As a user, I want the Add-ons page to say what the add-on requires (network only for the Internet test, free disk space for the disk test, no administrator), so that I can decide with full information.
52. As a user, I want a section in the settings with the test folder, the GPU to test, and a Summary panel yes/no, so that I can adapt the add-on.
53. As a user, I want a Summary panel with the last score and date, so that I can see it on the first page.
54. As a user, if Benchmarks becomes paid and I have not bought it, I want the locked preview with sample data ("Benchmarks are a … feature"), a purchase option and "Enter License Key…", so that I know what I am buying; in this state Start and Run All are disabled.
55. As a user on a trial, I want to see how many days are left and to keep my history even after the trial has expired, so that I don't lose results.

## Implementation Decisions

**Modules**

- **Benchmark engine.** One thread at a time; each test is a unit with a name, phases, progress from 0 to 1, figures and units, and cooperative cancellation. Interface: start a single test or all of them in sequence, cancel, read the state (current test and phase, progress, partial results, error). Replaces `start_benchmark` / `BenchState` of the pro module, which today uses text keys and cannot be cancelled.
- **CPU test.** The current workload (a mix of integer and floating-point work, 2 s on one core and 2 s on all cores) stays, with results expressed in Mops/sec and Gop/sec.
- **GPU test.** wgpu on the native backend of each system (Direct3D 12, Vulkan, Metal; hardware cards only): our WGSL compute shader (eight independent multiply-add chains per thread, 2 FLOP per multiply-add) and our vertex/fragment shader pair (a 64-step loop), timed with GPU timestamps or, when the device has none, with the CPU clock around each waited batch. The score note is "Score comparisons are valid only per version."
- **Disk test.** Test file in the chosen folder, write and read without the Windows cache (write-through writes and unbuffered reads), a free-space check before starting, guaranteed deletion.
- **Internet test.** HTTPS to speed.cloudflare.com (downloads in increasing blocks, upload), with a timeout and the network error messages of story 28; the HTTP client respects the system proxy. An HTTP client is needed: the choice of library is to be made during implementation (e.g. Windows WinHTTP, so we don't write our own TLS).
- **Memory test.** The current one, marked as an addition.
- **Score (pure).** For CPU, GPU and disk: the ratio to the reference machine's values × 100; overall as the geometric mean of the available scores. The reference machine is ours (proposal: the owner's PC, Ryzen 5 7600X + RTX 4080 + Samsung 980 PRO = 100), and the page sentence names it.
- **History (pure + file).** A JSON file in the app's data folder with one entry per run: test, ISO date, figures, score, app version, platform, hardware tested. Interface: add, read per test, delete per test; an unreadable file is renamed and replaced.
- **Page (design A).** Band, sidebar, a view with two analog gauges drawn with egui's painter, LED bar, Start, and a side panel with history and errors. Replaces `page_benchmarks`. "Benchmarks" menu in the menu bar only on this page.

**How it becomes an Addon**

- id `benchmarks`, name "Benchmarks", Free tier to start. First add-on to move into the registry (step 2 of the add-ons proposal).
- `needs`: network (Internet test only), no administrator, no external programs.
- `start`: reads history and settings, no thread. `stop`: cancels the running test, waits for the thread to finish, deletes the test file.
- Settings under `addons.benchmarks`: test folder, GPU, selected test, Summary panel.
- Summary panel: last overall score and date.

**Decisions already taken**

- Design A layout of board P15: bar with CPU, GPU, Disk | Internet | Score, two gauges per test, a single Start, units Mops/sec and Gop/sec for CPU.
- The score is not in the CPU view but in its own sidebar entry.
- The sidebar entry is called "Score" for now: the score takes the app's name once that is decided.
- Memory stays as an addition at the bottom of the sidebar.

## Testing Decisions

What is checked is the external behavior (results, messages, history contents), not how the engine is built internally.

1. **Unit tests on pure logic**, without UI or system, like the existing tests of the drivers and apps modules. Modules tested: score, history, formatting, engine with fake tests. Cases: score 100 with the base values, 200 with double, overall as geometric mean and without Internet; formatting of Mops/sec, Gop/sec, MB/sec, Mbit/s; gauge scale (a "rounded" full scale above the value); adding and reading history per test, deleting a single test, a corrupted file set aside; "Run All" runs the tests in order and stops on Cancel with state "Cancelled"; a failed test produces "%1 test failed: %2" and does not touch previous results; the free-space check rejects with "Not enough free space to run the test."; the test file is deleted after cancellation (using a temporary test folder); network messages from error codes and HTTP status.
2. **Contract tests common to the add-ons**: registry, id and `needs` (network); off does not appear and has no thread; `stop` during a test cancels it and leaves no thread or file; settings and history preserved; Not bought → locked preview, Start disabled.
3. **`#[ignore]` tests on this PC's real data**: short CPU, memory and disk tests that print the numbers; a GPU test that lists the adapters and tries the chosen one (expected here: the Radeon iGPU and the RTX 4080); an Internet test against Cloudflare (needs network, like `checks_windows_update`); plus a capture `--page "Benchmarks" --screenshot` to compare with the design A board.

## Out of Scope

- Online comparison with other users or leaderboards.
- Gaming, battery, memory latency or random-access disk benchmarks.
- Long stress tests and stability checks.
- Speed-test servers other than Cloudflare.

## Further Notes

Open questions:
- The unit for the GPU test is to be decided.
- The location of the history and its columns: the Version column follows from "Score comparisons are valid only per version".
- Whether the history is per test or global (the stories assume per test).
- All scores on the canvas are placeholders.
- To be decided with the owner: the reference machine for 100, the name of the score, the HTTP library for the Internet test.
