# rift

<div style="display:flex; align-items:flex-start; gap:1.5rem; width:100%;">
  <div style="flex:1 1 55%; min-width:0;">
    <p style="margin:0; font-size:1rem; line-height:1.6;">
      rift is a tiling window manager for macos inspired by i3 / sway with a virtual workspace system that takes away the pain points of native macos spaces. it has smooth animations, an extensive featureset, and does <strong>not</strong> require disabling of SIP for <em>any</em> functionality.
    </p>
  </div>
  <div style="flex:0 0 45%; max-width:45%;">
    <img src="assets/demo.gif" alt="Demo" style="width:100%; height:auto; display:block; border-radius:4px;" />
  </div>
</div>

### features
- smooth animations
- virtual workspaces (like [aerospace](https://github.com/nikitabobko/aerospace))
- extensive configurability (documented [here]((https://github.com/acsandmann/rift/wiki/Config))
- ipc layer that allows easily creation of custom integrations (like sketchybar or more dynamic configs)
- multiple layout systems:
	* tiling layout system(like i3/sway)
	* binary space partitioning system(like bspwm)
- menubar integration that shows the state of play ![menubar](assets/menubar.png)
- does **NOT** require disabling of SIP
- allows "displays have separate spaces" to be enabled without any issues (most/all wms ask for it to be disabled). this allows you to have a full screen app on one display whilst using the other display normally.
- switch to next/previous workspace with trackpad gestures (3 fingered swipes, or any amount of fingers)

### motivation
aerospace worked well for me but there were a few things i missed like animations, the ability to have a full screen window on one display whilst working on the other, and more. i also disagreed with the approach of not using the private api's as the os tends to actually rely on these apis far more than the public ones so they *generally* are much more reliable and performant (and in some cases necessary due to holes in the public apis).

### usage
> [config reference](https://github.com/acsandmann/rift/wiki/Config) [quick start](https://github.com/acsandmann/rift/wiki/Quick-Start)

### status
rift is still in active development but is generally *stable*. however, there is no official release as it is still a work in progress...

### credits
rift originally began as a fork(and is licensed as such) of [glide-wm](https://github.com/glide-wm/glide) but has since diverged significantly. it uses numerous private api's which were reverse engineered by yabai and other projects. it is not affiliated with glide-wm or yabai in any way.
