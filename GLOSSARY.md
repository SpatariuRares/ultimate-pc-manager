# Ultimate PC Manager

A manager that shows and controls what a PC is doing, on every supported system.

## Platform

**Supported system**:
An operating system on which the app must reach concept parity.
_Avoid_: platform, target, OS (when the support promise is meant)

**Concept parity**:
Every page and every add-on except system-specific ones exists on every supported system and answers the same question there, through that system's own mechanisms, looking as alike as those mechanisms allow.
_Avoid_: feature parity, port, "works on Linux"

**Native equivalent**:
The mechanism a supported system itself uses for a concept, such as its own service manager or package manager for the Services page or the Package Manager add-on.
_Avoid_: fallback, emulation

**Source**:
One of several mechanisms of the same kind on one system whose items a page shows together, each item marked with the source it came from; apt, Flatpak and winget are sources of packages.
_Avoid_: backend, provider, manager (for one of several)

## Pages

**Page**:
One entry of the sidebar, either a core page or an add-on's page; a page is never hidden on a supported system.
_Avoid_: view, tab, screen

**Add-on**:
An optional part of the app the user turns on or off, with its own page.
_Avoid_: plugin, extension, module, Pro page

**System-specific add-on**:
An add-on designed for some supported systems only, and shown only on those.
_Avoid_: Windows-only add-on (for the existing ones, which must reach concept parity)

**Feature**:
One capability inside a page, such as a column, an action or a panel; unlike a page, a feature is hidden on a system that has nothing it could show.
_Avoid_: function, capability (for a whole page)
