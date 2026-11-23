# 0001. Three supported systems and concept parity

Status: accepted

## Context

Ultimate PC Manager was written for Windows only. Code that talks to the operating
system had a Windows version and an empty stand-in for every other system, so on
other systems the app opened but several pages were empty. The owner uses Windows,
Linux and macOS daily and wants one app on all three.

## Decision

The supported systems are:

- Windows 11 (x86_64; ARM64 best effort).
- Linux with glibc 2.17+ and kernel 3.10+, on Wayland and X11 (ARM best effort;
  musl is not supported).
- macOS 13+ on Apple Silicon.

Any other target is a compile error that names the supported systems.

Concept parity means every concern (services, startup entries, processes,
performance data, sensors, and so on) exists as an idea on all three systems, with
the same functions and signatures, and each system supplies its own native
implementation. The UI code never branches per system. Each concern has one module
that holds the shared types and selects a `windows`, `linux` or `macos` submodule at
compile time. There is no generic empty stand-in: a missing implementation is a
compile error on that system, not an empty page found by a user.

An add-on declares the systems it is designed for (all three by default) and is
listed only there. A feature that has nothing to show on a system is hidden, never
greyed out.

## Consequences

- Every concern has three implementations, so adding a concern means adding three
  files (a Linux or macOS one may start as an explicit "not yet available" result
  that the UI already handles).
- Existing Windows code moves into the `windows` submodules unchanged.
- CI must build and test on all three systems.
- Windows 10 is not supported.
