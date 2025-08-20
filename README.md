# rift

rift is a tiling window for macos. it is inspired by i3/sway/hyprland and aerospace's virtual workspace system.

![demo](demo.gif)

### motivation
aerospace worked well for me but there were a few things i missed like animations and the ability to have separate managers per macos space. i also disagreed with the approach of not using the private api's as the os tends to actually rely on these apis far more than the public ones so they *generally* are much more reliable and performant (and in some cases necessary due to holes in the public apis).

### usage
> [config reference](https://github.com/acsandmann/rift/wiki/Config) [quick start](https://github.com/acsandmann/rift/wiki/Quick-Start)

### status
rift is very much a work in progress and not anywhere near completion

### credits
rift originally began as a fork(and is licensed as such) of [glide-wm](https://github.com/glide-wm/glide) but has since diverged significantly. it uses numerous private api's which were reverse engineered by yabai and other projects. it is not affiliated with glide-wm or yabai in any way.
