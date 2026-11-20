# Connections add-on

## Problem Statement

Anyone who uses Ultimate PC Manager wants to know which ports their PC exposes and which programs talk to the network.
Today the Connections page is a raw table: only IPv4 (all IPv6 connections are missing,
which on this PC are the majority of public listeners, e.g. `[::]:445`), no exposure classification (Internet, All
interfaces, Local network, Loopback), no scope (All / Connected / Listening / Public / UDP / Local IPC), no summary row
with the counts, no side panel with the globe, the owning user, the observation time and the explanation of the exposure,
no loading / unavailable / partial inventory states. Moreover the reading of the tables happens on the UI thread every
time the page is drawn, and it is hard-wired into the `Page` enum and the app's global state: it cannot be switched off
as a separate function.

The user instead wants a page that answers right away "what on my PC is reachable from outside, and who is keeping it
open?", that can be turned on or off as an add-on (PowerToys style) and that, when off, consumes nothing.

## Solution

Turn Connections into an add-on of the registry (see the prerequisite PRD "Add-on platform": registry, per-add-on
settings, licenses), with stable id `connections`, rebuilding the page according to the **design A** of the canvas
(P11-Connections).

From the user's point of view:

- A **status band** at the top with the sentence that answers the main question (e.g. "23 sockets listen on all
  interfaces", with a subtitle on ports, processes and which of these are Windows's own), an action "Show the Public
  scope", and a row of **five tiles**: Connections, Listening, Public, Processes, System traffic ↓/↑.
- An inventory status line: "Full inventory: N of up to 20,000 rows", or the loading, unavailable and partial
  messages.
- **Refresh** button at the top right and automatic refresh at the app's update speed.
- **Scope**: All, Connected, Listening, Public, UDP, Local IPC (disabled, "unavailable on Windows"), each with its count.
- **Find** box (our addition) on application, address, port, state, PID.
- Table with columns **Application, Protocol, State, Local endpoint, Remote / service, Exposure**; PID and executable as
  a sub-row of the application; protocol as TCP4/TCP6/UDP4/UDP6; UDP rows with state "Datagram" and remote "—"; rows
  grouped by exposure (a design A choice, declared on the page).
- **Side panel** with the globe drawing at the top, application name, "TCP6 · Established", PID, User, Local, Remote,
  Exposure, Observed (how long the connection has been seen), the note explaining the exposure, the list "The other
  exposure notes" with all six explanations, and the added actions: End process… (red, confirmation, disabled for System
  with the reason), Go to the process, Copy details, Copy remote address, Search online.
- When the add-on is off, the page disappears from the side menu and from `--page`, and no collection thread runs.

## User Stories

### Page, identity and navigation
1. As a user, I want a "Connections" entry in the side menu when the add-on is on, so that I can open the page in one click.
2. As a user, I want the page title to be "Connections" with the subtitle "Connection Management", so that I know which page I am on.
3. As a developer, I want to open the page with `--page connections` (and the alias `network-connections`), so that I can capture it with `--screenshot`.
4. As a user, I want a Refresh button at the top right, so that I can re-read the connections straight away without waiting for the next update.
5. As a user, I want the list to update by itself at the update speed chosen in Settings (Fast / Normal / Slow / Paused), so that I see the connections change without doing anything.
6. As a user, I want that with updates paused the list stays still until I press Refresh, so that I can examine a situation without it changing.

### Inventory states
7. As a user, I want to see "Collecting connection inventory." while the first read is in progress, so that I know the page is working.
8. As a user, I want to see "Connection inventory is currently unavailable." if Windows refuses to provide the tables, so that I understand the problem is not me.
9. As a user, I want to see "Partial inventory · limited to 20,000 rows" (and the sentence "Connection inventory is partial or limited to 20,000 rows.") when the rows exceed the limit, so that I know the counts are not complete.
10. As a user, I want to see "Full inventory: N of up to 20,000 rows" when the reading is complete, so that I know I see everything. (design A addition)
11. As a user, I want that if only one of the four tables (TCP4, TCP6, UDP4, UDP6) fails the others are still shown and the inventory is marked as partial, so that a small anomaly does not empty the page. (our addition)
12. As a user, I want a clear message when a scope or a search finds no rows (e.g. "No connections match"), so that I don't think the page is broken. (our addition)

### Status band and summary
13. As a user, I want a main sentence that says how many sockets listen on all interfaces (e.g. "23 sockets listen on all interfaces"), so that I see at a glance how exposed the PC is. (design A)
14. As a user, I want a subtitle that says on how many ports and from how many processes, which of these are Windows components and which are not, and that reminds that firewall and router decide reachability anyway, so that I know where to look. (design A)
15. As a user, I want an "Show the Public scope" action in the band, so that I go straight to the list of public listeners. (design A)
16. As a user, I want the Connections tile with the total number of rows, so that I know the size of the inventory.
17. As a user, I want the Listening tile with the number of listening sockets, so that I know how many services accept connections.
18. As a user, I want the Public tile with the number of listeners on all interfaces, so that I see the potential exposure.
19. As a user, I want the Processes tile with the number of distinct processes that have connections, so that I know how many programs use the network.
20. As a user, I want the System traffic ↓/↑ tile with the receive and send speed of the whole system, so that I can relate the connections to the real traffic.
21. As a user with a screen reader, I want the summary to be announced as "Scope %1. %2 connections, %3 listening, %4 public listeners, across %5 processes. Local IPC inventory is unavailable on Windows.", so that I get the same information as others.

### Scope
22. As a user, I want the All scope with all rows, so that I see the complete inventory.
23. As a user, I want the Connected scope with the TCP connections that have a remote end (Established, Close wait, Time wait, etc.), so that I see who I am talking to.
24. As a user, I want the Listening scope with the listening TCP sockets, so that I see the services that accept connections.
25. As a user, I want the Public scope with the listeners on all interfaces (0.0.0.0 or ::), so that I check what could be reachable from outside.
26. As a user, I want the UDP scope with the UDP endpoints, so that I also see connectionless traffic.
27. As a user, I want to see the Local IPC scope disabled with the note "unavailable on Windows", so that I know Windows does not allow it, instead of thinking it is missing.
28. As a user, I want the number of rows next to each scope, so that I know how much it contains before opening it. (design A)
29. As a user with a screen reader, I want the scope control to be announced as "Connection scope: %1", so that I know which is active.
30. As a user, I want the chosen scope to be remembered between launches, so that I find the page as I left it. (our addition, add-on setting)

### Search
31. As a user, I want a Find box that filters by application, address, port, state or PID, so that I immediately find a specific connection. (our addition, marked "Addition")
32. As a user, I want the search to combine with the active scope, so that I can search only among the public listeners. (our addition)

### Rows and columns
33. As a user, I want the Application column with the program's friendly name (e.g. "Service Host", "Microsoft Edge"), so that I recognise the program without knowing the executable.
34. As a user, I want the executable and "PID n" under the name, so that I can tell apart two instances of the same program. (position from design A)
35. As a user, I want for svchost processes to see which service holds the connection (e.g. RpcSs, DoSvc, CryptSvc), so that I know what is behind "Service Host". (our addition)
36. As a user, I want the Protocol column with TCP4, TCP6, UDP4 or UDP6, so that I know the transport and the IP family.
37. As a user, I want to see IPv6 connections too, so that the inventory does not hide half of the listeners. (today missing)
38. As a user, I want the State column with the states as Windows reports them (Established, Listening, SYN sent, SYN received, FIN wait 1, FIN wait 2, Close wait, Closing, Last ACK, Time wait, Closed, Delete TCB), so that I understand the phase of the connection.
39. As a user, I want UDP rows to show the state "Datagram" and the remote "—", so that I don't confuse them with TCP connections.
40. As a user, I want the Local endpoint column with address and port (IPv6 in square brackets), so that I know what the socket listens on.
41. As a user, I want the Remote / service column with "address:port · service name" for connections (e.g. "…:443 · HTTPS") and the port or service for listeners (e.g. "445 · SMB", "IPP"), so that I understand the type of traffic. (well-known service names from our own table)
42. As a user, I want the Exposure column with Internet, All interfaces, Local network, Loopback (and Unspecified when the data is not enough), so that I know how far each socket reaches.
43. As a user, I want the rows grouped by exposure with a heading, count and hint per group, so that I read the most exposed rows first. (a design A choice, declared on the page)
44. As a user, I want very long groups to show the first rows and an "N more" to expand them, so that the page stays readable. (design A)
45. As a user, I want to be able to sort the table by clicking the headers, so that I can sort by port or by application. (our addition)
46. As a user with a screen reader, I want each row to be announced as "%1, PID %2. %3, %4. Local %5. Remote %6. Exposure %7. Owner %8.", so that I hear all the columns.
47. As a user, I want the selected row to stay selected when the list updates, if the connection still exists, so that the side panel does not jump. (our addition)

### Side panel
48. As a user, I want to see "Select a row to inspect its route and owner" when no row is selected, so that I know what to do.
49. As a user, I want a globe drawing at the top of the panel, with lines from the centre to the points of the connections, so that I see where the connections go.
50. As a user, I want the globe variants for partial and unavailable inventory, with their captions, so that the drawing does not look complete when it isn't.
51. As a user, I want the panel to show the application name and "protocol · state" (e.g. "TCP6 · Established"), so that I have the summary of the row.
52. As a user, I want the PID, User, Local, Remote, Exposure fields, so that I have all the details in one place.
53. As a user, I want the User field with the account that owns the process (e.g. SYSTEM or my user), so that I know whether it is a service or my own program.
54. As a user, I want the Observed field with how long the app has seen that connection (e.g. "325s", "10 h 52 min"), so that I can tell stable connections from newly born ones.
55. As a user, I want a note with the explanation of the selected row's exposure, using these texts (wildcard, loopback, local network, Internet with "A shared or CDN address may represent more than one service", Local IPC, Unspecified), so that I understand what it means.
56. As a user, I want a list "The other exposure notes" with all the explanations, so that I learn the categories. (design A)
57. As a user, I want to see how many other connections the same process has, so that I understand whether a program opens many ports. (our addition)
58. As a user, I want the well-known service name of the port (e.g. 445 = Windows file sharing), so that I understand what it is for. (our addition)

### Actions (our additions, marked "Addition")
59. As a user, I want "Copy details" to copy all the fields of the row, so that I can paste them into a report. (our addition)
60. As a user, I want "Copy remote address" (or the local address for listeners), so that I can use it elsewhere. (our addition, already present)
61. As a user, I want "Search online" on the process name, so that I can discover a program I don't know. (our addition, already present)
62. As a user, I want "Go to the process" to open Processes with the selected process, so that I can see its CPU and memory. (our addition)
63. As a user, I want "End process…" in red with a confirmation, so that I can close a suspicious program without doing it by mistake. (our addition)
64. As a user, I want "End process…" to be disabled for System with the reason "System is Windows itself and can't be ended.", so that I know why. (our addition)
65. As a user, I want a message if closing the process fails (e.g. access denied without administrator), so that I know it did not happen. (our addition)
66. As a user, I want the same actions in the row's context menu, so that I don't have to open the panel. (our addition)

### Add-on behaviour
67. As a user, I want to turn Connections on or off from the Add-ons page in Settings, so that I keep only the functions I use. (add-on platform)
68. As a user, I want that with the add-on off the entry disappears from the side menu and `--page connections` opens nothing, so that the app stays clean. (add-on platform)
69. As a user, I want that with the add-on off no connection collection thread runs and no table reading happens, so that I don't pay CPU for a function I don't use. (add-on platform)
70. As a user, I want that with the add-on on but the page not open, the collection is stopped (except the Summary tile if I turn it on), so that the work happens only when needed. (our decision)
71. As a user, I want that when I switch the add-on off and back on, the chosen scope, the sorting and the other preferences remain, so that I don't have to reset them. (add-on platform)
72. As a user, I want the Add-ons page to tell me what Connections requires (no administrator, no network, no external program), so that I decide knowingly. (add-on platform)
73. As a user without administrator rights, I still want to see all the connections, with the process name or the user "unknown" where Windows does not grant it, so that the page always works. (our decision)
74. As a user, I want a Connections section in the settings (default scope, grouping by exposure yes/no, show system traffic), so that I can adapt the page. (our addition)
75. As a user, I want an optional tile in Summary with the number of public listeners, so that I see the exposure without opening the page. (our addition)
76. As a user, if Connections becomes paid and I have not bought it, I want to see a locked preview with sample data ("Sample connection preview.", rows such as preview-browser 0.0.0.0:8443, preview-mdns 0.0.0.0:5353, [::1]:7000, 127.0.0.1:631, [::]:5353, "Screen Sharing") and the purchase button, so that I understand what it offers. (license states from the add-on platform)
77. As a user who has not bought it, I want the locked preview not to read anything from my system, so that the preview consumes no resources and shows no real data. (add-on platform)
78. As a user on a trial, I want to see how many days I have left, so that I know when to decide. (add-on platform)

## Implementation Decisions

**Prerequisite.** It depends on the PRD "Add-on platform" (`Addon` interface, `AddonContext`, `Needs`, `Tier`, registry,
per-add-on settings under its own id, signed licenses and the locked preview). Connections is the third add-on to be
moved, after Benchmarks and Disk Space, according to the add-on plan's order.

**Modules to build or modify.**

- **Connections data module (new, deep, no UI).** Replaces the connections part of the pro module. Small interface: one
  function that reads the inventory and returns an `Inventory` (rows, status: Full / Partial / Unavailable, number of
  rows, reading instant), and pure functions over the rows: exposure classification, membership of the scopes, summary
  counts, endpoint formatting (IPv6 in square brackets), the well-known service name of the port, the text of the
  exposure explanation, accessibility texts. Each row carries: protocol (TCP/UDP), family (IPv4/IPv6), local address
  and port, optional remote address and port, TCP state (enum of the 12 states) or Datagram, PID, creation instant if
  available.
- **Observed tracking.** A map from the connection key (protocol, local, remote, PID) to the first instant the add-on
  saw it, updated on every reading and pruned for connections that have gone; if Windows provides the socket's creation
  instant (the "owner module" tables include it), that is used. The choice between the two sources is to be verified
  (see Further Notes).
- **Background worker (new).** A thread owned by the add-on, started by `start` and stopped by `stop` (with join), which
  reads the inventory at the global update speed and when the page asks for Refresh, and publishes the last `Inventory`
  to a shared state. It reads nothing when the add-on is off, when the license is "Not bought", and (decision) when the
  page is not open and the Summary tile is not active. Today the reading happens on the drawing thread: it must be
  removed from there.
- **Connections page (new, replaces the page function in the pages_pro module).** Draws design A: status band with the
  sentence and five tiles, inventory status line, Refresh, scope, Find, grouped table, side panel with globe, fields,
  notes and actions. The actions use the common ones of the `AddonContext` (copy, search online, end process with
  confirmation, go to process).
- **App shell.** Remove `Page::Connections`, the state field `conns`, the row selection and the related `match` arms; the
  side menu and `--page` read from the registry. The aliases `connections` and `network-connections` are registered by the
  add-on.

**Windows data sources.**

- Rows: IP Helper, extended TCP and UDP tables with the owner's PID, for **AF_INET and AF_INET6** (four tables). Today
  only IPv4. Does TCP also include rows in the Bound state if Windows returns them? Decision: exclude them (on this PC: TCP
  excluding Bound 182 + UDP 72 = 254).
- 20,000-row limit: beyond it, the list is truncated and the state becomes Partial.
- Process name, executable and user: from the sampler's samples shared through `AddonContext` (which already has name, exe
  and user per PID); PID 4 = System, PID 0 = System Idle Process.
- Friendly application name: from the executable's file description (FileDescription), with the executable name as
  fallback; cache per path.
- Services inside svchost: list of services with PID (Service Control Manager), already available in the winsys module.
- System traffic ↓/↑: sum of the network rates per interface from the sampler.
- Exposure classification (pure): Loopback for 127.0.0.0/8 and ::1; All interfaces for 0.0.0.0 and :: on listeners and UDP
  sockets; Local network for private addresses (10/8, 172.16/12, 192.168/16), link-local (169.254/16, fe80::/10) and local
  IPv6 (fc00::/7); Internet for the rest; Unspecified when data is missing. For connections the classification looks at the
  remote address; for listeners at the local one. Local IPC is never produced on Windows.
- Scopes (pure): Connected = TCP with a remote end (all states except Listening, Closed, Delete TCB); Listening = TCP
  Listening; Public = Listening with All interfaces exposure (decision: TCP only; wildcard UDP
  stays in the exposure); UDP = all UDP rows.
- Well-known service names: our own static table of common ports (80 HTTP, 443 HTTPS, 445 SMB, 135 RPC, 993 IMAPS, 631 IPP,
  5353 mDNS, 5355 LLMNR, 2179 Hyper-V, 7680 Delivery Optimization, …).

**How it becomes an `Addon`.**

- id `connections`, name "Connections", description "Find out what ports your computer is exposing to the
  Internet".
- `needs`: no administrator (IP Helper tables are readable by a normal user), no network, no external program.
- `tier`: to be decided in the platform; the design must work both Free and paid, with the locked preview using the sample
  data of story 76 and no system reading.
- `start` / `stop`: start and stop of the worker; `stop` empties the in-memory inventory.
- `settings`: default scope, grouping by exposure, sorting, show System traffic; kept under the id even when off.
- `summary_tile` (optional): number of public listeners and connections, with a link to the page.

**Decisions already made.** Design A layout from canvas P11 (band + five tiles, scope, grouped table, panel with globe);
no Owner or PID column, first column Application, protocol
TCP4/TCP6/UDP4/UDP6, UDP "Datagram", exposure "All interfaces" in place of "Public", Public as a scope and tile, Refresh
present, globe in the panel, User and Observed in the panel. Our additions (Find, actions, service name, grouping) remain
marked "Addition" on the page.

## Testing Decisions

Good tests check external behaviour (what a function returns for a given input, what the add-on does when switched on or
off), not internal details such as the structure of the maps or the names of private fields.

1. **Unit tests on the pure data logic of the connections module**, without UI or system access (prior art: the unit
   tests in the drivers and apps modules):
   - exposure classification for IPv4 and IPv6: loopback, wildcard, private, link-local, fc00::/7, public, edge cases
     (172.15 vs 172.16, 169.254);
   - scope membership and summary counts (connections, listening, public, distinct processes, UDP) on a hand-built set of
     rows, including the real numbers listed in Further Notes;
   - mapping of the 12 TCP states and of Datagram to the texts;
   - formatting of endpoints (IPv6 in square brackets, port) and of Remote / service with the well-known service name;
   - Full / Partial (over 20,000 rows, or a failed table) / Unavailable inventory state;
   - Observed: first sighting kept between readings, pruning of connections that have gone, duration formatting;
   - accessibility text of the row and of the summary with the placeholders filled in;
   - Find filter combined with the scope.
2. **Shared add-on contract tests** (defined by the platform), which Connections must pass: it is in the registry with id
   `connections`; when off it does not appear in the menu nor in `--page`; `start` starts and `stop` stops the worker (no
   reading after `stop`); with license "Not bought" no work starts and the page shows the preview; settings survive
   switching off and on.
3. **`#[ignore]` tests on this PC's real data**, like the existing `lists_this_pc`: reads the real inventory and checks that
   there are TCP4, TCP6, UDP4 and UDP6 rows, at least one listener (e.g. 445 of System), that every row has an exposure and
   that the state is Full; plus a capture with `--page connections --screenshot` to compare by eye with design A.

Modules tested: connections data module (levels 1 and 3), Connections add-on via the contract tests (level 2).

## Out of Scope

- Closing or resetting a single TCP connection (only "End process" remains, as an addition).
- Firewall rules.
- The IPv6 address and the connection type of the network adapter: they belong to Performance › Network.
- Local IPC inventory (named pipes, Unix-domain sockets): Windows does not expose it; the scope stays disabled.
- Geolocation of remote addresses or reverse DNS resolution (the globe is a drawing, not a geographic map).
- Connection history beyond the session; packet capture; traffic per individual connection.
- Separate distribution of the add-on (phase 2 of the plan).

## Further Notes

Open questions:

- **Exact look of the globe**: a grid globe with lines from the centre to the points above the selected
  app; how the points are positioned (by address? random?) is to be decided.
- **User and Observed values** in the mock are placeholders; the real source of Observed (first sighting by the app or
  Windows's socket creation instant) is to be decided by trying the "owner module" tables.
- **Public scope and wildcard UDP**: the Public scope counts only TCP (23) and shows wildcard UDP as All interfaces
  exposure (32).
- **Service names** (SMB, RPC, HTTP) are from the mock.
- The grouping by exposure is a design A choice.
- Real numbers used in the design (15 Nov 2026, this PC): 254 rows, 128 connected, 54 listening, 23 public on 15 ports, 72 UDP, 58 processes; exposure Loopback 111, Local network 39, Internet 49, All interfaces 55.
