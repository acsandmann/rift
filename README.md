<div align="center">

# Rift
  <p>Rift is a tiling window manager for macOS that focuses on performance and usability. </p>
  <img src="assets/demo.gif" alt="Rift demo" />

  <p>
    <a href="https://github.com/acsandmann/rift/actions/workflows/rust.yml">
      <img src="https://img.shields.io/github/actions/workflow/status/acsandmann/rift/rust.yml?style=flat-square" alt="Rust CI Status" />
    </a>
    <a href="https://github.com/acsandmann/rift/commits/main">
      <img src="https://img.shields.io/github/last-commit/acsandmann/rift?style=flat-square" alt="Last Commit" />
    </a>
    <a href="https://github.com/acsandmann/rift/issues">
      <img src="https://img.shields.io/github/issues/acsandmann/rift?style=flat-square" alt="Open Issues" />
    </a>
    <a href="https://github.com/acsandmann/rift/stargazers">
      <img src="https://img.shields.io/github/stars/acsandmann/rift?style=flat-square" alt="GitHub stars" />
    </a>
    <a href="https://matrix.to/#/%23rift:matrix.org">
      <img src="https://img.shields.io/matrix/rift%3Amatrix.org?style=flat-square" alt="Matrix" />
    </a>
  </p>
</div>

## Features
- Multiple layout styles
  - Tiling (i3/sway-like)
  - Binary Space Partitioning (bspwm-like)
  - Floating (independent window frames with optional stacks)
  - Master-stack (dwm-like)
  - Scrolling columns (niri-style)
  - Stack (accordion)
- Menubar icon that opens a menu for switching workspaces, changing layouts, and accessing quick Rift controls <details> <summary><sup>click to see the menu bar icon</sup></summary><img src="assets/menu_menu.png" alt="Rift menu bar icon" /></details>
- Save and restore layouts from the menu bar or CLI, with reusable layouts listed from a configurable folder <details> <summary><sup>click to see the menu</sup></summary><img src="assets/menu_layouts.png" alt="Rift menu for restoring layouts" /></details>
<!-- - MacOS-style mission control that allows you to visually navigate between workspaces <details><summary><sup>click to see mission control</sup></summary><img src="assets/mission_control.png" alt="Rift Mission Control view" /></details> -->
- Focus follows the mouse with auto raise
<!-- - Drag windows over one another to swap positions -->
- Does **not** require disabling SIP
- Performant animations <sup>(as seen in the [demo](#rift))</sup>
- Switch to next/previous workspace with trackpad gestures <sup>(just like native macOS)</sup>
- Hot reloadable configuration
- Mach port based IPC for communicating with rift from <a href="https://acsandmann.github.io/rift-docs/ecosystem/plugins/">third-party programs</a> (sketchybar, etc)
- Works with “Displays have separate Spaces” enabled (unlike all other major WMs)

## Quick Start
Get up and running via the docs:
<br>

[<kbd><br>config<br></kbd>][config_link]

[<kbd><br>quick start<br></kbd>][quick_start]
<br>

## Community

Join [#rift:matrix.org](https://matrix.to/#/#rift:matrix.org) for discussion, support, and development.

## Support

If rift is part of your daily workflow, consider [sponsoring its development](https://github.com/sponsors/acsandmann).

## Motivation
Aerospace worked well for me, but I missed animations and the ability to use fullscreen on one display while working on the other. I also prefer leveraging private/undocumented APIs as they tend to be more reliable (due to the OS being built on them and all the public APIs) and performant.
<sup><sup>for more on why rift exists and what rift strives to do, see the [manifesto](manifesto.md)</sup></sup>


## Credits
Rift began as a fork (and is licensed as such) of <a href="https://github.com/glide-wm/glide">glide-wm</a> but has since diverged significantly. It uses private APIs reverse engineered by yabai and other projects. It is not affiliated with glide-wm or yabai.


<!---------------------------------------------------------------------------->

[config_link]: https://acsandmann.github.io/rift-docs/reference/configuration/
[quick_start]: https://acsandmann.github.io/rift-docs/quick-start/
