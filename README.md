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
  </p>
</div>

## Features
- Multiple layout styles
  - Tiling (i3/sway-like)
  - Binary Space Partitioning (bspwm-like)
  - Master-stack (dwm-like)
  - Scrolling columns (niri-style)
- Menubar icon that shows all of the workspaces and the layouts within <details> <summary><sup>click to see the menu bar icon</sup></summary><img src="assets/menubar.png" alt="Rift menu bar icon" /></details>
- MacOS-style mission control that allows you to visually navigate between workspaces <details><summary><sup>click to see mission control</sup></summary><img src="assets/mission_control.png" alt="Rift Mission Control view" /></details>
- Focus follows the mouse with auto raise
- Drag windows over one another to swap positions
- Performant animations <sup>(as seen in the [demo](#rift))</sup>
- Switch to next/previous workspace with trackpad gestures <sup>(just like native macOS)</sup>
- Hot reloadable configuration
- Interop with third-party programs (ie Sketchybar)
  - Requests can be made to rift via the cli or the mach port exposed [(lua client here)](https://github.com/acsandmann/rift.lua)
  - Signals can be sent on startup, workspace switches, and when the windows within a workspace change. These signals can be sent via a command(cli) or through a mach connection
- Does **not** require disabling SIP
- Works with “Displays have separate Spaces” enabled (unlike all other major WMs)

## Quick Start
Get up and running via the wiki:
<br>

[<kbd><br>config<br></kbd>][config_link]

[<kbd><br>quick start<br></kbd>][quick_start]
<br>

## Status
Rift is in active development but is still generally stable. There is no official release yet; expect ongoing changes.

> Issues and PRs are very welcome.

## Motivation
Aerospace worked well for me, but I missed animations and the ability to use fullscreen on one display while working on the other. I also prefer leveraging private/undocumented APIs as they tend to be more reliable (due to the OS being built on them and all the public APIs) and performant.
<sup><sup>for more on why rift exists and what rift strives to do, see the [manifesto](manifesto.md)</sup></sup>


## Credits
Rift began as a fork (and is licensed as such) of <a href="https://github.com/glide-wm/glide">glide-wm</a> but has since diverged significantly. It uses private APIs reverse engineered by yabai and other projects. It is not affiliated with glide-wm or yabai.


<!---------------------------------------------------------------------------->

[config_link]: https://github.com/acsandmann/rift/wiki/Config
[quick_start]: https://github.com/acsandmann/rift/wiki/Quick-Start
