
use builtin;
use str;

set edit:completion:arg-completer[rift-cli] = {|@words|
    fn spaces {|n|
        builtin:repeat $n ' ' | str:join ''
    }
    fn cand {|text desc|
        edit:complex-candidate $text &display=$text' '(spaces (- 14 (wcswidth $text)))$desc
    }
    var command = 'rift-cli'
    for word $words[1..-1] {
        if (str:has-prefix $word '-') {
            break
        }
        set command = $command';'$word
    }
    var completions = [
        &'rift-cli'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand query 'Query information from rift'
            cand execute 'Execute commands in rift'
            cand subscribe 'Event subscription commands'
            cand service 'Manage the launchd service for rift'
            cand verify 'verify'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;query'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand workspaces 'List virtual workspaces (optionally for a specific MacOS space)'
            cand windows 'List windows (optionally filtered by space)'
            cand displays 'List connected displays'
            cand window 'Get information about a specific window'
            cand applications 'List running applications'
            cand layout 'Get layout state for a space'
            cand workspace-layout 'Get workspace layout-engine mode(s)'
            cand metrics 'Get performance metrics'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;query;workspaces'= {
            cand --space-id 'space-id'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;windows'= {
            cand --space-id 'space-id'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;displays'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;window'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;applications'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;layout'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;workspace-layout'= {
            cand --space-id 'space-id'
            cand --workspace-id 'workspace-id'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;metrics'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;query;help'= {
            cand workspaces 'List virtual workspaces (optionally for a specific MacOS space)'
            cand windows 'List windows (optionally filtered by space)'
            cand displays 'List connected displays'
            cand window 'Get information about a specific window'
            cand applications 'List running applications'
            cand layout 'Get layout state for a space'
            cand workspace-layout 'Get workspace layout-engine mode(s)'
            cand metrics 'Get performance metrics'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;query;help;workspaces'= {
        }
        &'rift-cli;query;help;windows'= {
        }
        &'rift-cli;query;help;displays'= {
        }
        &'rift-cli;query;help;window'= {
        }
        &'rift-cli;query;help;applications'= {
        }
        &'rift-cli;query;help;layout'= {
        }
        &'rift-cli;query;help;workspace-layout'= {
        }
        &'rift-cli;query;help;metrics'= {
        }
        &'rift-cli;query;help;help'= {
        }
        &'rift-cli;execute'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand window 'Window management commands'
            cand workspace 'Virtual workspace commands'
            cand layout 'Layout commands'
            cand config 'Configuration management commands'
            cand mission-control 'Mission control commands'
            cand display 'Display/mouse commands'
            cand save-and-exit 'Save current state and exit rift'
            cand debug 'Print layout tree debugging output in the running rift instance'
            cand serialize 'Serialize and print runtime state'
            cand toggle-space-activated 'Toggle whether the current space is managed by rift'
            cand show-timing 'Show timing metrics'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;window'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand next 'Focus the next window'
            cand prev 'Focus the previous window'
            cand focus 'Move focus in a direction'
            cand toggle-float 'Toggle window floating state'
            cand toggle-fullscreen 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)'
            cand toggle-fullscreen-within-gaps 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)'
            cand resize-grow 'Grow the current window size (increments by ~5%)'
            cand resize-shrink 'Shrink the current window size (decrements by ~5%)'
            cand resize-by 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%'
            cand close 'Close a window by window server identifier'
            cand add-scratchpad 'add-scratchpad'
            cand toggle-scratchpad 'toggle-scratchpad'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;window;next'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;prev'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;focus'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;toggle-float'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;toggle-fullscreen'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;toggle-fullscreen-within-gaps'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;resize-grow'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;resize-shrink'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;resize-by'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;close'= {
            cand --window-id 'Window Id (window server id or idx from window id)'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;add-scratchpad'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;toggle-scratchpad'= {
            cand --name 'Name of the scratchpad (optional, defaults to "default")'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;window;help'= {
            cand next 'Focus the next window'
            cand prev 'Focus the previous window'
            cand focus 'Move focus in a direction'
            cand toggle-float 'Toggle window floating state'
            cand toggle-fullscreen 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)'
            cand toggle-fullscreen-within-gaps 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)'
            cand resize-grow 'Grow the current window size (increments by ~5%)'
            cand resize-shrink 'Shrink the current window size (decrements by ~5%)'
            cand resize-by 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%'
            cand close 'Close a window by window server identifier'
            cand add-scratchpad 'add-scratchpad'
            cand toggle-scratchpad 'toggle-scratchpad'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;window;help;next'= {
        }
        &'rift-cli;execute;window;help;prev'= {
        }
        &'rift-cli;execute;window;help;focus'= {
        }
        &'rift-cli;execute;window;help;toggle-float'= {
        }
        &'rift-cli;execute;window;help;toggle-fullscreen'= {
        }
        &'rift-cli;execute;window;help;toggle-fullscreen-within-gaps'= {
        }
        &'rift-cli;execute;window;help;resize-grow'= {
        }
        &'rift-cli;execute;window;help;resize-shrink'= {
        }
        &'rift-cli;execute;window;help;resize-by'= {
        }
        &'rift-cli;execute;window;help;close'= {
        }
        &'rift-cli;execute;window;help;add-scratchpad'= {
        }
        &'rift-cli;execute;window;help;toggle-scratchpad'= {
        }
        &'rift-cli;execute;window;help;help'= {
        }
        &'rift-cli;execute;workspace'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand next 'Switch to next workspace'
            cand prev 'Switch to previous workspace'
            cand switch 'Switch to specific workspace'
            cand move-window 'Move current window to workspace'
            cand create 'Create a new workspace'
            cand last 'Switch to the last workspace'
            cand set-layout 'Set layout mode for a workspace (or active workspace when omitted)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;workspace;next'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;prev'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;switch'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;move-window'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;create'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;last'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;set-layout'= {
            cand --workspace-id 'Workspace index (0-based). Defaults to active workspace if omitted'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;workspace;help'= {
            cand next 'Switch to next workspace'
            cand prev 'Switch to previous workspace'
            cand switch 'Switch to specific workspace'
            cand move-window 'Move current window to workspace'
            cand create 'Create a new workspace'
            cand last 'Switch to the last workspace'
            cand set-layout 'Set layout mode for a workspace (or active workspace when omitted)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;workspace;help;next'= {
        }
        &'rift-cli;execute;workspace;help;prev'= {
        }
        &'rift-cli;execute;workspace;help;switch'= {
        }
        &'rift-cli;execute;workspace;help;move-window'= {
        }
        &'rift-cli;execute;workspace;help;create'= {
        }
        &'rift-cli;execute;workspace;help;last'= {
        }
        &'rift-cli;execute;workspace;help;set-layout'= {
        }
        &'rift-cli;execute;workspace;help;help'= {
        }
        &'rift-cli;execute;layout'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand ascend 'Move selection up the tree'
            cand descend 'Move selection down the tree'
            cand move-node 'Move the selected node in a direction'
            cand join-window 'Join the selected window with neighbor in a direction'
            cand toggle-stack 'Toggle stacked state for the selected container'
            cand toggle-orientation 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)'
            cand unjoin 'Unjoin previously joined windows'
            cand toggle-focus-float 'Toggle floating on the focused selection (tree focus)'
            cand adjust-master-ratio 'Adjust master ratio by a delta (master/stack layout only)'
            cand adjust-master-count 'Adjust master count by a delta (master/stack layout only)'
            cand promote-to-master 'Promote the selected window into the master area (master/stack layout only)'
            cand swap-master-stack 'Swap the first master with the first stack window (master/stack layout only)'
            cand swap-windows 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)'
            cand scroll-strip 'Scroll the strip by a normalized delta (scrolling layout only)'
            cand snap-strip 'Snap the strip to the nearest column boundary (scrolling layout only)'
            cand center-selection 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;layout;ascend'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;descend'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;move-node'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;join-window'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;toggle-stack'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;toggle-orientation'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;unjoin'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;toggle-focus-float'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;adjust-master-ratio'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;adjust-master-count'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;promote-to-master'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;swap-master-stack'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;swap-windows'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;scroll-strip'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;snap-strip'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;center-selection'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;layout;help'= {
            cand ascend 'Move selection up the tree'
            cand descend 'Move selection down the tree'
            cand move-node 'Move the selected node in a direction'
            cand join-window 'Join the selected window with neighbor in a direction'
            cand toggle-stack 'Toggle stacked state for the selected container'
            cand toggle-orientation 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)'
            cand unjoin 'Unjoin previously joined windows'
            cand toggle-focus-float 'Toggle floating on the focused selection (tree focus)'
            cand adjust-master-ratio 'Adjust master ratio by a delta (master/stack layout only)'
            cand adjust-master-count 'Adjust master count by a delta (master/stack layout only)'
            cand promote-to-master 'Promote the selected window into the master area (master/stack layout only)'
            cand swap-master-stack 'Swap the first master with the first stack window (master/stack layout only)'
            cand swap-windows 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)'
            cand scroll-strip 'Scroll the strip by a normalized delta (scrolling layout only)'
            cand snap-strip 'Snap the strip to the nearest column boundary (scrolling layout only)'
            cand center-selection 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;layout;help;ascend'= {
        }
        &'rift-cli;execute;layout;help;descend'= {
        }
        &'rift-cli;execute;layout;help;move-node'= {
        }
        &'rift-cli;execute;layout;help;join-window'= {
        }
        &'rift-cli;execute;layout;help;toggle-stack'= {
        }
        &'rift-cli;execute;layout;help;toggle-orientation'= {
        }
        &'rift-cli;execute;layout;help;unjoin'= {
        }
        &'rift-cli;execute;layout;help;toggle-focus-float'= {
        }
        &'rift-cli;execute;layout;help;adjust-master-ratio'= {
        }
        &'rift-cli;execute;layout;help;adjust-master-count'= {
        }
        &'rift-cli;execute;layout;help;promote-to-master'= {
        }
        &'rift-cli;execute;layout;help;swap-master-stack'= {
        }
        &'rift-cli;execute;layout;help;swap-windows'= {
        }
        &'rift-cli;execute;layout;help;scroll-strip'= {
        }
        &'rift-cli;execute;layout;help;snap-strip'= {
        }
        &'rift-cli;execute;layout;help;center-selection'= {
        }
        &'rift-cli;execute;layout;help;help'= {
        }
        &'rift-cli;execute;config'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand set-animate 'Update animation settings'
            cand set-animation-duration 'set-animation-duration'
            cand set-animation-fps 'set-animation-fps'
            cand set-animation-easing 'set-animation-easing'
            cand set-mouse-follows-focus 'Update mouse settings'
            cand set-mouse-hides-on-focus 'set-mouse-hides-on-focus'
            cand set-focus-follows-mouse 'set-focus-follows-mouse'
            cand set-stack-offset 'Update layout settings'
            cand set-stack-default-orientation 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"'
            cand set-outer-gaps 'set-outer-gaps'
            cand set-inner-gaps 'set-inner-gaps'
            cand set-workspace-names 'Update workspace settings'
            cand set 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true'
            cand get 'Get current config'
            cand save 'Save current config to file'
            cand reload 'Reload config from file'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;config;set-animate'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-animation-duration'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-animation-fps'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-animation-easing'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-mouse-follows-focus'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-mouse-hides-on-focus'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-focus-follows-mouse'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-stack-offset'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-stack-default-orientation'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-outer-gaps'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-inner-gaps'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set-workspace-names'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;set'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;get'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;save'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;reload'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;config;help'= {
            cand set-animate 'Update animation settings'
            cand set-animation-duration 'set-animation-duration'
            cand set-animation-fps 'set-animation-fps'
            cand set-animation-easing 'set-animation-easing'
            cand set-mouse-follows-focus 'Update mouse settings'
            cand set-mouse-hides-on-focus 'set-mouse-hides-on-focus'
            cand set-focus-follows-mouse 'set-focus-follows-mouse'
            cand set-stack-offset 'Update layout settings'
            cand set-stack-default-orientation 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"'
            cand set-outer-gaps 'set-outer-gaps'
            cand set-inner-gaps 'set-inner-gaps'
            cand set-workspace-names 'Update workspace settings'
            cand set 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true'
            cand get 'Get current config'
            cand save 'Save current config to file'
            cand reload 'Reload config from file'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;config;help;set-animate'= {
        }
        &'rift-cli;execute;config;help;set-animation-duration'= {
        }
        &'rift-cli;execute;config;help;set-animation-fps'= {
        }
        &'rift-cli;execute;config;help;set-animation-easing'= {
        }
        &'rift-cli;execute;config;help;set-mouse-follows-focus'= {
        }
        &'rift-cli;execute;config;help;set-mouse-hides-on-focus'= {
        }
        &'rift-cli;execute;config;help;set-focus-follows-mouse'= {
        }
        &'rift-cli;execute;config;help;set-stack-offset'= {
        }
        &'rift-cli;execute;config;help;set-stack-default-orientation'= {
        }
        &'rift-cli;execute;config;help;set-outer-gaps'= {
        }
        &'rift-cli;execute;config;help;set-inner-gaps'= {
        }
        &'rift-cli;execute;config;help;set-workspace-names'= {
        }
        &'rift-cli;execute;config;help;set'= {
        }
        &'rift-cli;execute;config;help;get'= {
        }
        &'rift-cli;execute;config;help;save'= {
        }
        &'rift-cli;execute;config;help;reload'= {
        }
        &'rift-cli;execute;config;help;help'= {
        }
        &'rift-cli;execute;mission-control'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand show-all 'Show all workspaces in mission control'
            cand show-current 'Show current workspace in mission control'
            cand dismiss 'Dismiss mission control'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;mission-control;show-all'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;mission-control;show-current'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;mission-control;dismiss'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;mission-control;help'= {
            cand show-all 'Show all workspaces in mission control'
            cand show-current 'Show current workspace in mission control'
            cand dismiss 'Dismiss mission control'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;mission-control;help;show-all'= {
        }
        &'rift-cli;execute;mission-control;help;show-current'= {
        }
        &'rift-cli;execute;mission-control;help;dismiss'= {
        }
        &'rift-cli;execute;mission-control;help;help'= {
        }
        &'rift-cli;execute;display'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand focus 'Focus a display by direction, index, or UUID'
            cand move-mouse-to-index 'Move mouse cursor to a display by index (0-based)'
            cand move-mouse-to-uuid 'Move mouse cursor to a display by UUID'
            cand move-window 'Move a window to a display by direction, index, or UUID'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;display;focus'= {
            cand --direction 'Direction relative to the current display (left, right, up, down)'
            cand --index 'Display index (0-based)'
            cand --uuid 'Display UUID'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;display;move-mouse-to-index'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;display;move-mouse-to-uuid'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;display;move-window'= {
            cand --direction 'Direction relative to the window''s current display (left, right, up, down)'
            cand --index 'Display index (0-based)'
            cand --uuid 'Display UUID'
            cand --window-id 'Optional window id (window idx); defaults to the focused window if omitted'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;display;help'= {
            cand focus 'Focus a display by direction, index, or UUID'
            cand move-mouse-to-index 'Move mouse cursor to a display by index (0-based)'
            cand move-mouse-to-uuid 'Move mouse cursor to a display by UUID'
            cand move-window 'Move a window to a display by direction, index, or UUID'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;display;help;focus'= {
        }
        &'rift-cli;execute;display;help;move-mouse-to-index'= {
        }
        &'rift-cli;execute;display;help;move-mouse-to-uuid'= {
        }
        &'rift-cli;execute;display;help;move-window'= {
        }
        &'rift-cli;execute;display;help;help'= {
        }
        &'rift-cli;execute;save-and-exit'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;debug'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;serialize'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;toggle-space-activated'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;show-timing'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;execute;help'= {
            cand window 'Window management commands'
            cand workspace 'Virtual workspace commands'
            cand layout 'Layout commands'
            cand config 'Configuration management commands'
            cand mission-control 'Mission control commands'
            cand display 'Display/mouse commands'
            cand save-and-exit 'Save current state and exit rift'
            cand debug 'Print layout tree debugging output in the running rift instance'
            cand serialize 'Serialize and print runtime state'
            cand toggle-space-activated 'Toggle whether the current space is managed by rift'
            cand show-timing 'Show timing metrics'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;execute;help;window'= {
            cand next 'Focus the next window'
            cand prev 'Focus the previous window'
            cand focus 'Move focus in a direction'
            cand toggle-float 'Toggle window floating state'
            cand toggle-fullscreen 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)'
            cand toggle-fullscreen-within-gaps 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)'
            cand resize-grow 'Grow the current window size (increments by ~5%)'
            cand resize-shrink 'Shrink the current window size (decrements by ~5%)'
            cand resize-by 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%'
            cand close 'Close a window by window server identifier'
            cand add-scratchpad 'add-scratchpad'
            cand toggle-scratchpad 'toggle-scratchpad'
        }
        &'rift-cli;execute;help;window;next'= {
        }
        &'rift-cli;execute;help;window;prev'= {
        }
        &'rift-cli;execute;help;window;focus'= {
        }
        &'rift-cli;execute;help;window;toggle-float'= {
        }
        &'rift-cli;execute;help;window;toggle-fullscreen'= {
        }
        &'rift-cli;execute;help;window;toggle-fullscreen-within-gaps'= {
        }
        &'rift-cli;execute;help;window;resize-grow'= {
        }
        &'rift-cli;execute;help;window;resize-shrink'= {
        }
        &'rift-cli;execute;help;window;resize-by'= {
        }
        &'rift-cli;execute;help;window;close'= {
        }
        &'rift-cli;execute;help;window;add-scratchpad'= {
        }
        &'rift-cli;execute;help;window;toggle-scratchpad'= {
        }
        &'rift-cli;execute;help;workspace'= {
            cand next 'Switch to next workspace'
            cand prev 'Switch to previous workspace'
            cand switch 'Switch to specific workspace'
            cand move-window 'Move current window to workspace'
            cand create 'Create a new workspace'
            cand last 'Switch to the last workspace'
            cand set-layout 'Set layout mode for a workspace (or active workspace when omitted)'
        }
        &'rift-cli;execute;help;workspace;next'= {
        }
        &'rift-cli;execute;help;workspace;prev'= {
        }
        &'rift-cli;execute;help;workspace;switch'= {
        }
        &'rift-cli;execute;help;workspace;move-window'= {
        }
        &'rift-cli;execute;help;workspace;create'= {
        }
        &'rift-cli;execute;help;workspace;last'= {
        }
        &'rift-cli;execute;help;workspace;set-layout'= {
        }
        &'rift-cli;execute;help;layout'= {
            cand ascend 'Move selection up the tree'
            cand descend 'Move selection down the tree'
            cand move-node 'Move the selected node in a direction'
            cand join-window 'Join the selected window with neighbor in a direction'
            cand toggle-stack 'Toggle stacked state for the selected container'
            cand toggle-orientation 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)'
            cand unjoin 'Unjoin previously joined windows'
            cand toggle-focus-float 'Toggle floating on the focused selection (tree focus)'
            cand adjust-master-ratio 'Adjust master ratio by a delta (master/stack layout only)'
            cand adjust-master-count 'Adjust master count by a delta (master/stack layout only)'
            cand promote-to-master 'Promote the selected window into the master area (master/stack layout only)'
            cand swap-master-stack 'Swap the first master with the first stack window (master/stack layout only)'
            cand swap-windows 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)'
            cand scroll-strip 'Scroll the strip by a normalized delta (scrolling layout only)'
            cand snap-strip 'Snap the strip to the nearest column boundary (scrolling layout only)'
            cand center-selection 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed'
        }
        &'rift-cli;execute;help;layout;ascend'= {
        }
        &'rift-cli;execute;help;layout;descend'= {
        }
        &'rift-cli;execute;help;layout;move-node'= {
        }
        &'rift-cli;execute;help;layout;join-window'= {
        }
        &'rift-cli;execute;help;layout;toggle-stack'= {
        }
        &'rift-cli;execute;help;layout;toggle-orientation'= {
        }
        &'rift-cli;execute;help;layout;unjoin'= {
        }
        &'rift-cli;execute;help;layout;toggle-focus-float'= {
        }
        &'rift-cli;execute;help;layout;adjust-master-ratio'= {
        }
        &'rift-cli;execute;help;layout;adjust-master-count'= {
        }
        &'rift-cli;execute;help;layout;promote-to-master'= {
        }
        &'rift-cli;execute;help;layout;swap-master-stack'= {
        }
        &'rift-cli;execute;help;layout;swap-windows'= {
        }
        &'rift-cli;execute;help;layout;scroll-strip'= {
        }
        &'rift-cli;execute;help;layout;snap-strip'= {
        }
        &'rift-cli;execute;help;layout;center-selection'= {
        }
        &'rift-cli;execute;help;config'= {
            cand set-animate 'Update animation settings'
            cand set-animation-duration 'set-animation-duration'
            cand set-animation-fps 'set-animation-fps'
            cand set-animation-easing 'set-animation-easing'
            cand set-mouse-follows-focus 'Update mouse settings'
            cand set-mouse-hides-on-focus 'set-mouse-hides-on-focus'
            cand set-focus-follows-mouse 'set-focus-follows-mouse'
            cand set-stack-offset 'Update layout settings'
            cand set-stack-default-orientation 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"'
            cand set-outer-gaps 'set-outer-gaps'
            cand set-inner-gaps 'set-inner-gaps'
            cand set-workspace-names 'Update workspace settings'
            cand set 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true'
            cand get 'Get current config'
            cand save 'Save current config to file'
            cand reload 'Reload config from file'
        }
        &'rift-cli;execute;help;config;set-animate'= {
        }
        &'rift-cli;execute;help;config;set-animation-duration'= {
        }
        &'rift-cli;execute;help;config;set-animation-fps'= {
        }
        &'rift-cli;execute;help;config;set-animation-easing'= {
        }
        &'rift-cli;execute;help;config;set-mouse-follows-focus'= {
        }
        &'rift-cli;execute;help;config;set-mouse-hides-on-focus'= {
        }
        &'rift-cli;execute;help;config;set-focus-follows-mouse'= {
        }
        &'rift-cli;execute;help;config;set-stack-offset'= {
        }
        &'rift-cli;execute;help;config;set-stack-default-orientation'= {
        }
        &'rift-cli;execute;help;config;set-outer-gaps'= {
        }
        &'rift-cli;execute;help;config;set-inner-gaps'= {
        }
        &'rift-cli;execute;help;config;set-workspace-names'= {
        }
        &'rift-cli;execute;help;config;set'= {
        }
        &'rift-cli;execute;help;config;get'= {
        }
        &'rift-cli;execute;help;config;save'= {
        }
        &'rift-cli;execute;help;config;reload'= {
        }
        &'rift-cli;execute;help;mission-control'= {
            cand show-all 'Show all workspaces in mission control'
            cand show-current 'Show current workspace in mission control'
            cand dismiss 'Dismiss mission control'
        }
        &'rift-cli;execute;help;mission-control;show-all'= {
        }
        &'rift-cli;execute;help;mission-control;show-current'= {
        }
        &'rift-cli;execute;help;mission-control;dismiss'= {
        }
        &'rift-cli;execute;help;display'= {
            cand focus 'Focus a display by direction, index, or UUID'
            cand move-mouse-to-index 'Move mouse cursor to a display by index (0-based)'
            cand move-mouse-to-uuid 'Move mouse cursor to a display by UUID'
            cand move-window 'Move a window to a display by direction, index, or UUID'
        }
        &'rift-cli;execute;help;display;focus'= {
        }
        &'rift-cli;execute;help;display;move-mouse-to-index'= {
        }
        &'rift-cli;execute;help;display;move-mouse-to-uuid'= {
        }
        &'rift-cli;execute;help;display;move-window'= {
        }
        &'rift-cli;execute;help;save-and-exit'= {
        }
        &'rift-cli;execute;help;debug'= {
        }
        &'rift-cli;execute;help;serialize'= {
        }
        &'rift-cli;execute;help;toggle-space-activated'= {
        }
        &'rift-cli;execute;help;show-timing'= {
        }
        &'rift-cli;execute;help;help'= {
        }
        &'rift-cli;subscribe'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand mach 'Subscribe to Mach IPC events'
            cand cli 'Subscribe to events via CLI command execution'
            cand unsub-mach 'Unsubscribe from Mach IPC events'
            cand unsub-cli 'Unsubscribe from CLI events'
            cand list-cli 'List current CLI subscriptions'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;subscribe;mach'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;subscribe;cli'= {
            cand --event 'Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)'
            cand --command 'Command to execute when event occurs'
            cand --args 'Arguments to pass to command (event data will be appended as JSON)'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;subscribe;unsub-mach'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;subscribe;unsub-cli'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;subscribe;list-cli'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;subscribe;help'= {
            cand mach 'Subscribe to Mach IPC events'
            cand cli 'Subscribe to events via CLI command execution'
            cand unsub-mach 'Unsubscribe from Mach IPC events'
            cand unsub-cli 'Unsubscribe from CLI events'
            cand list-cli 'List current CLI subscriptions'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;subscribe;help;mach'= {
        }
        &'rift-cli;subscribe;help;cli'= {
        }
        &'rift-cli;subscribe;help;unsub-mach'= {
        }
        &'rift-cli;subscribe;help;unsub-cli'= {
        }
        &'rift-cli;subscribe;help;list-cli'= {
        }
        &'rift-cli;subscribe;help;help'= {
        }
        &'rift-cli;service'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand install 'Install the per-user launchd service'
            cand uninstall 'Uninstall the per-user launchd service'
            cand start 'Start (or bootstrap) the service'
            cand stop 'Stop (or bootout/kill) the service'
            cand restart 'Restart the service (kickstart -k)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;service;install'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;service;uninstall'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;service;start'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;service;stop'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;service;restart'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;service;help'= {
            cand install 'Install the per-user launchd service'
            cand uninstall 'Uninstall the per-user launchd service'
            cand start 'Start (or bootstrap) the service'
            cand stop 'Stop (or bootout/kill) the service'
            cand restart 'Restart the service (kickstart -k)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;service;help;install'= {
        }
        &'rift-cli;service;help;uninstall'= {
        }
        &'rift-cli;service;help;start'= {
        }
        &'rift-cli;service;help;stop'= {
        }
        &'rift-cli;service;help;restart'= {
        }
        &'rift-cli;service;help;help'= {
        }
        &'rift-cli;verify'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift-cli;help'= {
            cand query 'Query information from rift'
            cand execute 'Execute commands in rift'
            cand subscribe 'Event subscription commands'
            cand service 'Manage the launchd service for rift'
            cand verify 'verify'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift-cli;help;query'= {
            cand workspaces 'List virtual workspaces (optionally for a specific MacOS space)'
            cand windows 'List windows (optionally filtered by space)'
            cand displays 'List connected displays'
            cand window 'Get information about a specific window'
            cand applications 'List running applications'
            cand layout 'Get layout state for a space'
            cand workspace-layout 'Get workspace layout-engine mode(s)'
            cand metrics 'Get performance metrics'
        }
        &'rift-cli;help;query;workspaces'= {
        }
        &'rift-cli;help;query;windows'= {
        }
        &'rift-cli;help;query;displays'= {
        }
        &'rift-cli;help;query;window'= {
        }
        &'rift-cli;help;query;applications'= {
        }
        &'rift-cli;help;query;layout'= {
        }
        &'rift-cli;help;query;workspace-layout'= {
        }
        &'rift-cli;help;query;metrics'= {
        }
        &'rift-cli;help;execute'= {
            cand window 'Window management commands'
            cand workspace 'Virtual workspace commands'
            cand layout 'Layout commands'
            cand config 'Configuration management commands'
            cand mission-control 'Mission control commands'
            cand display 'Display/mouse commands'
            cand save-and-exit 'Save current state and exit rift'
            cand debug 'Print layout tree debugging output in the running rift instance'
            cand serialize 'Serialize and print runtime state'
            cand toggle-space-activated 'Toggle whether the current space is managed by rift'
            cand show-timing 'Show timing metrics'
        }
        &'rift-cli;help;execute;window'= {
            cand next 'Focus the next window'
            cand prev 'Focus the previous window'
            cand focus 'Move focus in a direction'
            cand toggle-float 'Toggle window floating state'
            cand toggle-fullscreen 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)'
            cand toggle-fullscreen-within-gaps 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)'
            cand resize-grow 'Grow the current window size (increments by ~5%)'
            cand resize-shrink 'Shrink the current window size (decrements by ~5%)'
            cand resize-by 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%'
            cand close 'Close a window by window server identifier'
            cand add-scratchpad 'add-scratchpad'
            cand toggle-scratchpad 'toggle-scratchpad'
        }
        &'rift-cli;help;execute;window;next'= {
        }
        &'rift-cli;help;execute;window;prev'= {
        }
        &'rift-cli;help;execute;window;focus'= {
        }
        &'rift-cli;help;execute;window;toggle-float'= {
        }
        &'rift-cli;help;execute;window;toggle-fullscreen'= {
        }
        &'rift-cli;help;execute;window;toggle-fullscreen-within-gaps'= {
        }
        &'rift-cli;help;execute;window;resize-grow'= {
        }
        &'rift-cli;help;execute;window;resize-shrink'= {
        }
        &'rift-cli;help;execute;window;resize-by'= {
        }
        &'rift-cli;help;execute;window;close'= {
        }
        &'rift-cli;help;execute;window;add-scratchpad'= {
        }
        &'rift-cli;help;execute;window;toggle-scratchpad'= {
        }
        &'rift-cli;help;execute;workspace'= {
            cand next 'Switch to next workspace'
            cand prev 'Switch to previous workspace'
            cand switch 'Switch to specific workspace'
            cand move-window 'Move current window to workspace'
            cand create 'Create a new workspace'
            cand last 'Switch to the last workspace'
            cand set-layout 'Set layout mode for a workspace (or active workspace when omitted)'
        }
        &'rift-cli;help;execute;workspace;next'= {
        }
        &'rift-cli;help;execute;workspace;prev'= {
        }
        &'rift-cli;help;execute;workspace;switch'= {
        }
        &'rift-cli;help;execute;workspace;move-window'= {
        }
        &'rift-cli;help;execute;workspace;create'= {
        }
        &'rift-cli;help;execute;workspace;last'= {
        }
        &'rift-cli;help;execute;workspace;set-layout'= {
        }
        &'rift-cli;help;execute;layout'= {
            cand ascend 'Move selection up the tree'
            cand descend 'Move selection down the tree'
            cand move-node 'Move the selected node in a direction'
            cand join-window 'Join the selected window with neighbor in a direction'
            cand toggle-stack 'Toggle stacked state for the selected container'
            cand toggle-orientation 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)'
            cand unjoin 'Unjoin previously joined windows'
            cand toggle-focus-float 'Toggle floating on the focused selection (tree focus)'
            cand adjust-master-ratio 'Adjust master ratio by a delta (master/stack layout only)'
            cand adjust-master-count 'Adjust master count by a delta (master/stack layout only)'
            cand promote-to-master 'Promote the selected window into the master area (master/stack layout only)'
            cand swap-master-stack 'Swap the first master with the first stack window (master/stack layout only)'
            cand swap-windows 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)'
            cand scroll-strip 'Scroll the strip by a normalized delta (scrolling layout only)'
            cand snap-strip 'Snap the strip to the nearest column boundary (scrolling layout only)'
            cand center-selection 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed'
        }
        &'rift-cli;help;execute;layout;ascend'= {
        }
        &'rift-cli;help;execute;layout;descend'= {
        }
        &'rift-cli;help;execute;layout;move-node'= {
        }
        &'rift-cli;help;execute;layout;join-window'= {
        }
        &'rift-cli;help;execute;layout;toggle-stack'= {
        }
        &'rift-cli;help;execute;layout;toggle-orientation'= {
        }
        &'rift-cli;help;execute;layout;unjoin'= {
        }
        &'rift-cli;help;execute;layout;toggle-focus-float'= {
        }
        &'rift-cli;help;execute;layout;adjust-master-ratio'= {
        }
        &'rift-cli;help;execute;layout;adjust-master-count'= {
        }
        &'rift-cli;help;execute;layout;promote-to-master'= {
        }
        &'rift-cli;help;execute;layout;swap-master-stack'= {
        }
        &'rift-cli;help;execute;layout;swap-windows'= {
        }
        &'rift-cli;help;execute;layout;scroll-strip'= {
        }
        &'rift-cli;help;execute;layout;snap-strip'= {
        }
        &'rift-cli;help;execute;layout;center-selection'= {
        }
        &'rift-cli;help;execute;config'= {
            cand set-animate 'Update animation settings'
            cand set-animation-duration 'set-animation-duration'
            cand set-animation-fps 'set-animation-fps'
            cand set-animation-easing 'set-animation-easing'
            cand set-mouse-follows-focus 'Update mouse settings'
            cand set-mouse-hides-on-focus 'set-mouse-hides-on-focus'
            cand set-focus-follows-mouse 'set-focus-follows-mouse'
            cand set-stack-offset 'Update layout settings'
            cand set-stack-default-orientation 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"'
            cand set-outer-gaps 'set-outer-gaps'
            cand set-inner-gaps 'set-inner-gaps'
            cand set-workspace-names 'Update workspace settings'
            cand set 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true'
            cand get 'Get current config'
            cand save 'Save current config to file'
            cand reload 'Reload config from file'
        }
        &'rift-cli;help;execute;config;set-animate'= {
        }
        &'rift-cli;help;execute;config;set-animation-duration'= {
        }
        &'rift-cli;help;execute;config;set-animation-fps'= {
        }
        &'rift-cli;help;execute;config;set-animation-easing'= {
        }
        &'rift-cli;help;execute;config;set-mouse-follows-focus'= {
        }
        &'rift-cli;help;execute;config;set-mouse-hides-on-focus'= {
        }
        &'rift-cli;help;execute;config;set-focus-follows-mouse'= {
        }
        &'rift-cli;help;execute;config;set-stack-offset'= {
        }
        &'rift-cli;help;execute;config;set-stack-default-orientation'= {
        }
        &'rift-cli;help;execute;config;set-outer-gaps'= {
        }
        &'rift-cli;help;execute;config;set-inner-gaps'= {
        }
        &'rift-cli;help;execute;config;set-workspace-names'= {
        }
        &'rift-cli;help;execute;config;set'= {
        }
        &'rift-cli;help;execute;config;get'= {
        }
        &'rift-cli;help;execute;config;save'= {
        }
        &'rift-cli;help;execute;config;reload'= {
        }
        &'rift-cli;help;execute;mission-control'= {
            cand show-all 'Show all workspaces in mission control'
            cand show-current 'Show current workspace in mission control'
            cand dismiss 'Dismiss mission control'
        }
        &'rift-cli;help;execute;mission-control;show-all'= {
        }
        &'rift-cli;help;execute;mission-control;show-current'= {
        }
        &'rift-cli;help;execute;mission-control;dismiss'= {
        }
        &'rift-cli;help;execute;display'= {
            cand focus 'Focus a display by direction, index, or UUID'
            cand move-mouse-to-index 'Move mouse cursor to a display by index (0-based)'
            cand move-mouse-to-uuid 'Move mouse cursor to a display by UUID'
            cand move-window 'Move a window to a display by direction, index, or UUID'
        }
        &'rift-cli;help;execute;display;focus'= {
        }
        &'rift-cli;help;execute;display;move-mouse-to-index'= {
        }
        &'rift-cli;help;execute;display;move-mouse-to-uuid'= {
        }
        &'rift-cli;help;execute;display;move-window'= {
        }
        &'rift-cli;help;execute;save-and-exit'= {
        }
        &'rift-cli;help;execute;debug'= {
        }
        &'rift-cli;help;execute;serialize'= {
        }
        &'rift-cli;help;execute;toggle-space-activated'= {
        }
        &'rift-cli;help;execute;show-timing'= {
        }
        &'rift-cli;help;subscribe'= {
            cand mach 'Subscribe to Mach IPC events'
            cand cli 'Subscribe to events via CLI command execution'
            cand unsub-mach 'Unsubscribe from Mach IPC events'
            cand unsub-cli 'Unsubscribe from CLI events'
            cand list-cli 'List current CLI subscriptions'
        }
        &'rift-cli;help;subscribe;mach'= {
        }
        &'rift-cli;help;subscribe;cli'= {
        }
        &'rift-cli;help;subscribe;unsub-mach'= {
        }
        &'rift-cli;help;subscribe;unsub-cli'= {
        }
        &'rift-cli;help;subscribe;list-cli'= {
        }
        &'rift-cli;help;service'= {
            cand install 'Install the per-user launchd service'
            cand uninstall 'Uninstall the per-user launchd service'
            cand start 'Start (or bootstrap) the service'
            cand stop 'Stop (or bootout/kill) the service'
            cand restart 'Restart the service (kickstart -k)'
        }
        &'rift-cli;help;service;install'= {
        }
        &'rift-cli;help;service;uninstall'= {
        }
        &'rift-cli;help;service;start'= {
        }
        &'rift-cli;help;service;stop'= {
        }
        &'rift-cli;help;service;restart'= {
        }
        &'rift-cli;help;verify'= {
        }
        &'rift-cli;help;help'= {
        }
    ]
    $completions[$command]
}
