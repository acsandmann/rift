module completions {

  # Command-line interface for rift window manager
  export extern rift-cli [
    --help(-h)                # Print help
  ]

  # Query information from rift
  export extern "rift-cli query" [
    --help(-h)                # Print help
  ]

  # List virtual workspaces (optionally for a specific MacOS space)
  export extern "rift-cli query workspaces" [
    --space-id: string
    --help(-h)                # Print help
  ]

  # List windows (optionally filtered by space)
  export extern "rift-cli query windows" [
    --space-id: string
    --help(-h)                # Print help
  ]

  # List connected displays
  export extern "rift-cli query displays" [
    --help(-h)                # Print help
  ]

  # Get information about a specific window
  export extern "rift-cli query window" [
    --help(-h)                # Print help
    window_id: string
  ]

  # List running applications
  export extern "rift-cli query applications" [
    --help(-h)                # Print help
  ]

  # Get layout state for a space
  export extern "rift-cli query layout" [
    --help(-h)                # Print help
    space_id: string
  ]

  # Get workspace layout-engine mode(s)
  export extern "rift-cli query workspace-layout" [
    --space-id: string
    --workspace-id: string
    --help(-h)                # Print help
  ]

  # Get performance metrics
  export extern "rift-cli query metrics" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli query help" [
  ]

  # List virtual workspaces (optionally for a specific MacOS space)
  export extern "rift-cli query help workspaces" [
  ]

  # List windows (optionally filtered by space)
  export extern "rift-cli query help windows" [
  ]

  # List connected displays
  export extern "rift-cli query help displays" [
  ]

  # Get information about a specific window
  export extern "rift-cli query help window" [
  ]

  # List running applications
  export extern "rift-cli query help applications" [
  ]

  # Get layout state for a space
  export extern "rift-cli query help layout" [
  ]

  # Get workspace layout-engine mode(s)
  export extern "rift-cli query help workspace-layout" [
  ]

  # Get performance metrics
  export extern "rift-cli query help metrics" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli query help help" [
  ]

  # Execute commands in rift
  export extern "rift-cli execute" [
    --help(-h)                # Print help
  ]

  # Window management commands
  export extern "rift-cli execute window" [
    --help(-h)                # Print help
  ]

  # Focus the next window
  export extern "rift-cli execute window next" [
    --help(-h)                # Print help
  ]

  # Focus the previous window
  export extern "rift-cli execute window prev" [
    --help(-h)                # Print help
  ]

  # Move focus in a direction
  export extern "rift-cli execute window focus" [
    --help(-h)                # Print help
    direction: string
  ]

  # Toggle window floating state
  export extern "rift-cli execute window toggle-float" [
    --help(-h)                # Print help
  ]

  # Toggle fullscreen mode (fills the whole screen, ignores outer gaps)
  export extern "rift-cli execute window toggle-fullscreen" [
    --help(-h)                # Print help
  ]

  # Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)
  export extern "rift-cli execute window toggle-fullscreen-within-gaps" [
    --help(-h)                # Print help
  ]

  # Grow the current window size (increments by ~5%)
  export extern "rift-cli execute window resize-grow" [
    --help(-h)                # Print help
  ]

  # Shrink the current window size (decrements by ~5%)
  export extern "rift-cli execute window resize-shrink" [
    --help(-h)                # Print help
  ]

  # Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%
  export extern "rift-cli execute window resize-by" [
    --help(-h)                # Print help
    amount: string
  ]

  # Close a window by window server identifier
  export extern "rift-cli execute window close" [
    --window-id: string       # Window Id (window server id or idx from window id)
    --help(-h)                # Print help
  ]

  export extern "rift-cli execute window add-scratchpad" [
    --help(-h)                # Print help
  ]

  export extern "rift-cli execute window toggle-scratchpad" [
    --name: string            # Name of the scratchpad (optional, defaults to "default")
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute window help" [
  ]

  # Focus the next window
  export extern "rift-cli execute window help next" [
  ]

  # Focus the previous window
  export extern "rift-cli execute window help prev" [
  ]

  # Move focus in a direction
  export extern "rift-cli execute window help focus" [
  ]

  # Toggle window floating state
  export extern "rift-cli execute window help toggle-float" [
  ]

  # Toggle fullscreen mode (fills the whole screen, ignores outer gaps)
  export extern "rift-cli execute window help toggle-fullscreen" [
  ]

  # Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)
  export extern "rift-cli execute window help toggle-fullscreen-within-gaps" [
  ]

  # Grow the current window size (increments by ~5%)
  export extern "rift-cli execute window help resize-grow" [
  ]

  # Shrink the current window size (decrements by ~5%)
  export extern "rift-cli execute window help resize-shrink" [
  ]

  # Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%
  export extern "rift-cli execute window help resize-by" [
  ]

  # Close a window by window server identifier
  export extern "rift-cli execute window help close" [
  ]

  export extern "rift-cli execute window help add-scratchpad" [
  ]

  export extern "rift-cli execute window help toggle-scratchpad" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute window help help" [
  ]

  # Virtual workspace commands
  export extern "rift-cli execute workspace" [
    --help(-h)                # Print help
  ]

  def "nu-complete rift-cli execute workspace next skip_empty" [] {
    [ "true" "false" ]
  }

  # Switch to next workspace
  export extern "rift-cli execute workspace next" [
    --help(-h)                # Print help
    skip_empty?: string@"nu-complete rift-cli execute workspace next skip_empty"
  ]

  def "nu-complete rift-cli execute workspace prev skip_empty" [] {
    [ "true" "false" ]
  }

  # Switch to previous workspace
  export extern "rift-cli execute workspace prev" [
    --help(-h)                # Print help
    skip_empty?: string@"nu-complete rift-cli execute workspace prev skip_empty"
  ]

  # Switch to specific workspace
  export extern "rift-cli execute workspace switch" [
    --help(-h)                # Print help
    workspace_id: string
  ]

  # Move current window to workspace
  export extern "rift-cli execute workspace move-window" [
    --help(-h)                # Print help
    workspace_id: string
    window_id?: string
  ]

  # Create a new workspace
  export extern "rift-cli execute workspace create" [
    --help(-h)                # Print help
  ]

  # Switch to the last workspace
  export extern "rift-cli execute workspace last" [
    --help(-h)                # Print help
  ]

  # Set layout mode for a workspace (or active workspace when omitted)
  export extern "rift-cli execute workspace set-layout" [
    --workspace-id: string    # Workspace index (0-based). Defaults to active workspace if omitted
    --help(-h)                # Print help
    mode: string              # Layout mode: traditional, bsp, stack, master_stack, scrolling
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute workspace help" [
  ]

  # Switch to next workspace
  export extern "rift-cli execute workspace help next" [
  ]

  # Switch to previous workspace
  export extern "rift-cli execute workspace help prev" [
  ]

  # Switch to specific workspace
  export extern "rift-cli execute workspace help switch" [
  ]

  # Move current window to workspace
  export extern "rift-cli execute workspace help move-window" [
  ]

  # Create a new workspace
  export extern "rift-cli execute workspace help create" [
  ]

  # Switch to the last workspace
  export extern "rift-cli execute workspace help last" [
  ]

  # Set layout mode for a workspace (or active workspace when omitted)
  export extern "rift-cli execute workspace help set-layout" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute workspace help help" [
  ]

  # Layout commands
  export extern "rift-cli execute layout" [
    --help(-h)                # Print help
  ]

  # Move selection up the tree
  export extern "rift-cli execute layout ascend" [
    --help(-h)                # Print help
  ]

  # Move selection down the tree
  export extern "rift-cli execute layout descend" [
    --help(-h)                # Print help
  ]

  # Move the selected node in a direction
  export extern "rift-cli execute layout move-node" [
    --help(-h)                # Print help
    direction: string
  ]

  # Join the selected window with neighbor in a direction
  export extern "rift-cli execute layout join-window" [
    --help(-h)                # Print help
    direction: string
  ]

  # Toggle stacked state for the selected container
  export extern "rift-cli execute layout toggle-stack" [
    --help(-h)                # Print help
  ]

  # Global orientation toggle that works consistently across layout modes (and between splits/stacks)
  export extern "rift-cli execute layout toggle-orientation" [
    --help(-h)                # Print help
  ]

  # Unjoin previously joined windows
  export extern "rift-cli execute layout unjoin" [
    --help(-h)                # Print help
  ]

  # Toggle floating on the focused selection (tree focus)
  export extern "rift-cli execute layout toggle-focus-float" [
    --help(-h)                # Print help
  ]

  # Adjust master ratio by a delta (master/stack layout only)
  export extern "rift-cli execute layout adjust-master-ratio" [
    --help(-h)                # Print help
    delta: string
  ]

  # Adjust master count by a delta (master/stack layout only)
  export extern "rift-cli execute layout adjust-master-count" [
    --help(-h)                # Print help
    delta: string
  ]

  # Promote the selected window into the master area (master/stack layout only)
  export extern "rift-cli execute layout promote-to-master" [
    --help(-h)                # Print help
  ]

  # Swap the first master with the first stack window (master/stack layout only)
  export extern "rift-cli execute layout swap-master-stack" [
    --help(-h)                # Print help
  ]

  # Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)
  export extern "rift-cli execute layout swap-windows" [
    --help(-h)                # Print help
    a: string
    b: string
  ]

  # Scroll the strip by a normalized delta (scrolling layout only)
  export extern "rift-cli execute layout scroll-strip" [
    --help(-h)                # Print help
    delta: string
  ]

  # Snap the strip to the nearest column boundary (scrolling layout only)
  export extern "rift-cli execute layout snap-strip" [
    --help(-h)                # Print help
  ]

  # Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed
  export extern "rift-cli execute layout center-selection" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute layout help" [
  ]

  # Move selection up the tree
  export extern "rift-cli execute layout help ascend" [
  ]

  # Move selection down the tree
  export extern "rift-cli execute layout help descend" [
  ]

  # Move the selected node in a direction
  export extern "rift-cli execute layout help move-node" [
  ]

  # Join the selected window with neighbor in a direction
  export extern "rift-cli execute layout help join-window" [
  ]

  # Toggle stacked state for the selected container
  export extern "rift-cli execute layout help toggle-stack" [
  ]

  # Global orientation toggle that works consistently across layout modes (and between splits/stacks)
  export extern "rift-cli execute layout help toggle-orientation" [
  ]

  # Unjoin previously joined windows
  export extern "rift-cli execute layout help unjoin" [
  ]

  # Toggle floating on the focused selection (tree focus)
  export extern "rift-cli execute layout help toggle-focus-float" [
  ]

  # Adjust master ratio by a delta (master/stack layout only)
  export extern "rift-cli execute layout help adjust-master-ratio" [
  ]

  # Adjust master count by a delta (master/stack layout only)
  export extern "rift-cli execute layout help adjust-master-count" [
  ]

  # Promote the selected window into the master area (master/stack layout only)
  export extern "rift-cli execute layout help promote-to-master" [
  ]

  # Swap the first master with the first stack window (master/stack layout only)
  export extern "rift-cli execute layout help swap-master-stack" [
  ]

  # Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)
  export extern "rift-cli execute layout help swap-windows" [
  ]

  # Scroll the strip by a normalized delta (scrolling layout only)
  export extern "rift-cli execute layout help scroll-strip" [
  ]

  # Snap the strip to the nearest column boundary (scrolling layout only)
  export extern "rift-cli execute layout help snap-strip" [
  ]

  # Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed
  export extern "rift-cli execute layout help center-selection" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute layout help help" [
  ]

  # Configuration management commands
  export extern "rift-cli execute config" [
    --help(-h)                # Print help
  ]

  # Update animation settings
  export extern "rift-cli execute config set-animate" [
    --help(-h)                # Print help
    value: string
  ]

  export extern "rift-cli execute config set-animation-duration" [
    --help(-h)                # Print help
    value: string
  ]

  export extern "rift-cli execute config set-animation-fps" [
    --help(-h)                # Print help
    value: string
  ]

  export extern "rift-cli execute config set-animation-easing" [
    --help(-h)                # Print help
    value: string
  ]

  def "nu-complete rift-cli execute config set-mouse-follows-focus value" [] {
    [ "true" "false" ]
  }

  # Update mouse settings
  export extern "rift-cli execute config set-mouse-follows-focus" [
    --help(-h)                # Print help
    value: string@"nu-complete rift-cli execute config set-mouse-follows-focus value"
  ]

  def "nu-complete rift-cli execute config set-mouse-hides-on-focus value" [] {
    [ "true" "false" ]
  }

  export extern "rift-cli execute config set-mouse-hides-on-focus" [
    --help(-h)                # Print help
    value: string@"nu-complete rift-cli execute config set-mouse-hides-on-focus value"
  ]

  def "nu-complete rift-cli execute config set-focus-follows-mouse value" [] {
    [ "true" "false" ]
  }

  export extern "rift-cli execute config set-focus-follows-mouse" [
    --help(-h)                # Print help
    value: string@"nu-complete rift-cli execute config set-focus-follows-mouse value"
  ]

  # Update layout settings
  export extern "rift-cli execute config set-stack-offset" [
    --help(-h)                # Print help
    value: string
  ]

  # Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"
  export extern "rift-cli execute config set-stack-default-orientation" [
    --help(-h)                # Print help
    value: string
  ]

  export extern "rift-cli execute config set-outer-gaps" [
    --help(-h)                # Print help
    top: string
    left: string
    bottom: string
    right: string
  ]

  export extern "rift-cli execute config set-inner-gaps" [
    --help(-h)                # Print help
    horizontal: string
    vertical: string
  ]

  # Update workspace settings
  export extern "rift-cli execute config set-workspace-names" [
    --help(-h)                # Print help
    ...names: string
  ]

  # Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true
  export extern "rift-cli execute config set" [
    --help(-h)                # Print help
    key: string               # Dot-separated key path (e.g. settings.animate or settings.layout.gaps.outer.top)
    value: string             # Value should be valid JSON (true, 1, "string", {"a":1}), but if it's not valid JSON it will be treated as a string
  ]

  # Get current config
  export extern "rift-cli execute config get" [
    --help(-h)                # Print help
  ]

  # Save current config to file
  export extern "rift-cli execute config save" [
    --help(-h)                # Print help
  ]

  # Reload config from file
  export extern "rift-cli execute config reload" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute config help" [
  ]

  # Update animation settings
  export extern "rift-cli execute config help set-animate" [
  ]

  export extern "rift-cli execute config help set-animation-duration" [
  ]

  export extern "rift-cli execute config help set-animation-fps" [
  ]

  export extern "rift-cli execute config help set-animation-easing" [
  ]

  # Update mouse settings
  export extern "rift-cli execute config help set-mouse-follows-focus" [
  ]

  export extern "rift-cli execute config help set-mouse-hides-on-focus" [
  ]

  export extern "rift-cli execute config help set-focus-follows-mouse" [
  ]

  # Update layout settings
  export extern "rift-cli execute config help set-stack-offset" [
  ]

  # Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"
  export extern "rift-cli execute config help set-stack-default-orientation" [
  ]

  export extern "rift-cli execute config help set-outer-gaps" [
  ]

  export extern "rift-cli execute config help set-inner-gaps" [
  ]

  # Update workspace settings
  export extern "rift-cli execute config help set-workspace-names" [
  ]

  # Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true
  export extern "rift-cli execute config help set" [
  ]

  # Get current config
  export extern "rift-cli execute config help get" [
  ]

  # Save current config to file
  export extern "rift-cli execute config help save" [
  ]

  # Reload config from file
  export extern "rift-cli execute config help reload" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute config help help" [
  ]

  # Mission control commands
  export extern "rift-cli execute mission-control" [
    --help(-h)                # Print help
  ]

  # Show all workspaces in mission control
  export extern "rift-cli execute mission-control show-all" [
    --help(-h)                # Print help
  ]

  # Show current workspace in mission control
  export extern "rift-cli execute mission-control show-current" [
    --help(-h)                # Print help
  ]

  # Dismiss mission control
  export extern "rift-cli execute mission-control dismiss" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute mission-control help" [
  ]

  # Show all workspaces in mission control
  export extern "rift-cli execute mission-control help show-all" [
  ]

  # Show current workspace in mission control
  export extern "rift-cli execute mission-control help show-current" [
  ]

  # Dismiss mission control
  export extern "rift-cli execute mission-control help dismiss" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute mission-control help help" [
  ]

  # Display/mouse commands
  export extern "rift-cli execute display" [
    --help(-h)                # Print help
  ]

  # Focus a display by direction, index, or UUID
  export extern "rift-cli execute display focus" [
    --direction: string       # Direction relative to the current display (left, right, up, down)
    --index: string           # Display index (0-based)
    --uuid: string            # Display UUID
    --help(-h)                # Print help
  ]

  # Move mouse cursor to a display by index (0-based)
  export extern "rift-cli execute display move-mouse-to-index" [
    --help(-h)                # Print help
    index: string             # Display index (0-based)
  ]

  # Move mouse cursor to a display by UUID
  export extern "rift-cli execute display move-mouse-to-uuid" [
    --help(-h)                # Print help
    uuid: string              # Display UUID
  ]

  # Move a window to a display by direction, index, or UUID
  export extern "rift-cli execute display move-window" [
    --direction: string       # Direction relative to the window's current display (left, right, up, down)
    --index: string           # Display index (0-based)
    --uuid: string            # Display UUID
    --window-id: string       # Optional window id (window idx); defaults to the focused window if omitted
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute display help" [
  ]

  # Focus a display by direction, index, or UUID
  export extern "rift-cli execute display help focus" [
  ]

  # Move mouse cursor to a display by index (0-based)
  export extern "rift-cli execute display help move-mouse-to-index" [
  ]

  # Move mouse cursor to a display by UUID
  export extern "rift-cli execute display help move-mouse-to-uuid" [
  ]

  # Move a window to a display by direction, index, or UUID
  export extern "rift-cli execute display help move-window" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute display help help" [
  ]

  # Save current state and exit rift
  export extern "rift-cli execute save-and-exit" [
    --help(-h)                # Print help
  ]

  # Print layout tree debugging output in the running rift instance
  export extern "rift-cli execute debug" [
    --help(-h)                # Print help
  ]

  # Serialize and print runtime state
  export extern "rift-cli execute serialize" [
    --help(-h)                # Print help
  ]

  # Toggle whether the current space is managed by rift
  export extern "rift-cli execute toggle-space-activated" [
    --help(-h)                # Print help
  ]

  # Show timing metrics
  export extern "rift-cli execute show-timing" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute help" [
  ]

  # Window management commands
  export extern "rift-cli execute help window" [
  ]

  # Focus the next window
  export extern "rift-cli execute help window next" [
  ]

  # Focus the previous window
  export extern "rift-cli execute help window prev" [
  ]

  # Move focus in a direction
  export extern "rift-cli execute help window focus" [
  ]

  # Toggle window floating state
  export extern "rift-cli execute help window toggle-float" [
  ]

  # Toggle fullscreen mode (fills the whole screen, ignores outer gaps)
  export extern "rift-cli execute help window toggle-fullscreen" [
  ]

  # Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)
  export extern "rift-cli execute help window toggle-fullscreen-within-gaps" [
  ]

  # Grow the current window size (increments by ~5%)
  export extern "rift-cli execute help window resize-grow" [
  ]

  # Shrink the current window size (decrements by ~5%)
  export extern "rift-cli execute help window resize-shrink" [
  ]

  # Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%
  export extern "rift-cli execute help window resize-by" [
  ]

  # Close a window by window server identifier
  export extern "rift-cli execute help window close" [
  ]

  export extern "rift-cli execute help window add-scratchpad" [
  ]

  export extern "rift-cli execute help window toggle-scratchpad" [
  ]

  # Virtual workspace commands
  export extern "rift-cli execute help workspace" [
  ]

  # Switch to next workspace
  export extern "rift-cli execute help workspace next" [
  ]

  # Switch to previous workspace
  export extern "rift-cli execute help workspace prev" [
  ]

  # Switch to specific workspace
  export extern "rift-cli execute help workspace switch" [
  ]

  # Move current window to workspace
  export extern "rift-cli execute help workspace move-window" [
  ]

  # Create a new workspace
  export extern "rift-cli execute help workspace create" [
  ]

  # Switch to the last workspace
  export extern "rift-cli execute help workspace last" [
  ]

  # Set layout mode for a workspace (or active workspace when omitted)
  export extern "rift-cli execute help workspace set-layout" [
  ]

  # Layout commands
  export extern "rift-cli execute help layout" [
  ]

  # Move selection up the tree
  export extern "rift-cli execute help layout ascend" [
  ]

  # Move selection down the tree
  export extern "rift-cli execute help layout descend" [
  ]

  # Move the selected node in a direction
  export extern "rift-cli execute help layout move-node" [
  ]

  # Join the selected window with neighbor in a direction
  export extern "rift-cli execute help layout join-window" [
  ]

  # Toggle stacked state for the selected container
  export extern "rift-cli execute help layout toggle-stack" [
  ]

  # Global orientation toggle that works consistently across layout modes (and between splits/stacks)
  export extern "rift-cli execute help layout toggle-orientation" [
  ]

  # Unjoin previously joined windows
  export extern "rift-cli execute help layout unjoin" [
  ]

  # Toggle floating on the focused selection (tree focus)
  export extern "rift-cli execute help layout toggle-focus-float" [
  ]

  # Adjust master ratio by a delta (master/stack layout only)
  export extern "rift-cli execute help layout adjust-master-ratio" [
  ]

  # Adjust master count by a delta (master/stack layout only)
  export extern "rift-cli execute help layout adjust-master-count" [
  ]

  # Promote the selected window into the master area (master/stack layout only)
  export extern "rift-cli execute help layout promote-to-master" [
  ]

  # Swap the first master with the first stack window (master/stack layout only)
  export extern "rift-cli execute help layout swap-master-stack" [
  ]

  # Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)
  export extern "rift-cli execute help layout swap-windows" [
  ]

  # Scroll the strip by a normalized delta (scrolling layout only)
  export extern "rift-cli execute help layout scroll-strip" [
  ]

  # Snap the strip to the nearest column boundary (scrolling layout only)
  export extern "rift-cli execute help layout snap-strip" [
  ]

  # Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed
  export extern "rift-cli execute help layout center-selection" [
  ]

  # Configuration management commands
  export extern "rift-cli execute help config" [
  ]

  # Update animation settings
  export extern "rift-cli execute help config set-animate" [
  ]

  export extern "rift-cli execute help config set-animation-duration" [
  ]

  export extern "rift-cli execute help config set-animation-fps" [
  ]

  export extern "rift-cli execute help config set-animation-easing" [
  ]

  # Update mouse settings
  export extern "rift-cli execute help config set-mouse-follows-focus" [
  ]

  export extern "rift-cli execute help config set-mouse-hides-on-focus" [
  ]

  export extern "rift-cli execute help config set-focus-follows-mouse" [
  ]

  # Update layout settings
  export extern "rift-cli execute help config set-stack-offset" [
  ]

  # Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"
  export extern "rift-cli execute help config set-stack-default-orientation" [
  ]

  export extern "rift-cli execute help config set-outer-gaps" [
  ]

  export extern "rift-cli execute help config set-inner-gaps" [
  ]

  # Update workspace settings
  export extern "rift-cli execute help config set-workspace-names" [
  ]

  # Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true
  export extern "rift-cli execute help config set" [
  ]

  # Get current config
  export extern "rift-cli execute help config get" [
  ]

  # Save current config to file
  export extern "rift-cli execute help config save" [
  ]

  # Reload config from file
  export extern "rift-cli execute help config reload" [
  ]

  # Mission control commands
  export extern "rift-cli execute help mission-control" [
  ]

  # Show all workspaces in mission control
  export extern "rift-cli execute help mission-control show-all" [
  ]

  # Show current workspace in mission control
  export extern "rift-cli execute help mission-control show-current" [
  ]

  # Dismiss mission control
  export extern "rift-cli execute help mission-control dismiss" [
  ]

  # Display/mouse commands
  export extern "rift-cli execute help display" [
  ]

  # Focus a display by direction, index, or UUID
  export extern "rift-cli execute help display focus" [
  ]

  # Move mouse cursor to a display by index (0-based)
  export extern "rift-cli execute help display move-mouse-to-index" [
  ]

  # Move mouse cursor to a display by UUID
  export extern "rift-cli execute help display move-mouse-to-uuid" [
  ]

  # Move a window to a display by direction, index, or UUID
  export extern "rift-cli execute help display move-window" [
  ]

  # Save current state and exit rift
  export extern "rift-cli execute help save-and-exit" [
  ]

  # Print layout tree debugging output in the running rift instance
  export extern "rift-cli execute help debug" [
  ]

  # Serialize and print runtime state
  export extern "rift-cli execute help serialize" [
  ]

  # Toggle whether the current space is managed by rift
  export extern "rift-cli execute help toggle-space-activated" [
  ]

  # Show timing metrics
  export extern "rift-cli execute help show-timing" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli execute help help" [
  ]

  # Event subscription commands
  export extern "rift-cli subscribe" [
    --help(-h)                # Print help
  ]

  # Subscribe to Mach IPC events
  export extern "rift-cli subscribe mach" [
    --help(-h)                # Print help
    event: string             # Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)
  ]

  # Subscribe to events via CLI command execution
  export extern "rift-cli subscribe cli" [
    --event: string           # Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)
    --command: string         # Command to execute when event occurs
    --args: string            # Arguments to pass to command (event data will be appended as JSON)
    --help(-h)                # Print help
  ]

  # Unsubscribe from Mach IPC events
  export extern "rift-cli subscribe unsub-mach" [
    --help(-h)                # Print help
    event: string             # Event to unsubscribe from
  ]

  # Unsubscribe from CLI events
  export extern "rift-cli subscribe unsub-cli" [
    --help(-h)                # Print help
    event: string             # Event to unsubscribe from
  ]

  # List current CLI subscriptions
  export extern "rift-cli subscribe list-cli" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli subscribe help" [
  ]

  # Subscribe to Mach IPC events
  export extern "rift-cli subscribe help mach" [
  ]

  # Subscribe to events via CLI command execution
  export extern "rift-cli subscribe help cli" [
  ]

  # Unsubscribe from Mach IPC events
  export extern "rift-cli subscribe help unsub-mach" [
  ]

  # Unsubscribe from CLI events
  export extern "rift-cli subscribe help unsub-cli" [
  ]

  # List current CLI subscriptions
  export extern "rift-cli subscribe help list-cli" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli subscribe help help" [
  ]

  # Manage the launchd service for rift
  export extern "rift-cli service" [
    --help(-h)                # Print help
  ]

  # Install the per-user launchd service
  export extern "rift-cli service install" [
    --help(-h)                # Print help
  ]

  # Uninstall the per-user launchd service
  export extern "rift-cli service uninstall" [
    --help(-h)                # Print help
  ]

  # Start (or bootstrap) the service
  export extern "rift-cli service start" [
    --help(-h)                # Print help
  ]

  # Stop (or bootout/kill) the service
  export extern "rift-cli service stop" [
    --help(-h)                # Print help
  ]

  # Restart the service (kickstart -k)
  export extern "rift-cli service restart" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli service help" [
  ]

  # Install the per-user launchd service
  export extern "rift-cli service help install" [
  ]

  # Uninstall the per-user launchd service
  export extern "rift-cli service help uninstall" [
  ]

  # Start (or bootstrap) the service
  export extern "rift-cli service help start" [
  ]

  # Stop (or bootout/kill) the service
  export extern "rift-cli service help stop" [
  ]

  # Restart the service (kickstart -k)
  export extern "rift-cli service help restart" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli service help help" [
  ]

  export extern "rift-cli verify" [
    --help(-h)                # Print help
    config_path: path
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli help" [
  ]

  # Query information from rift
  export extern "rift-cli help query" [
  ]

  # List virtual workspaces (optionally for a specific MacOS space)
  export extern "rift-cli help query workspaces" [
  ]

  # List windows (optionally filtered by space)
  export extern "rift-cli help query windows" [
  ]

  # List connected displays
  export extern "rift-cli help query displays" [
  ]

  # Get information about a specific window
  export extern "rift-cli help query window" [
  ]

  # List running applications
  export extern "rift-cli help query applications" [
  ]

  # Get layout state for a space
  export extern "rift-cli help query layout" [
  ]

  # Get workspace layout-engine mode(s)
  export extern "rift-cli help query workspace-layout" [
  ]

  # Get performance metrics
  export extern "rift-cli help query metrics" [
  ]

  # Execute commands in rift
  export extern "rift-cli help execute" [
  ]

  # Window management commands
  export extern "rift-cli help execute window" [
  ]

  # Focus the next window
  export extern "rift-cli help execute window next" [
  ]

  # Focus the previous window
  export extern "rift-cli help execute window prev" [
  ]

  # Move focus in a direction
  export extern "rift-cli help execute window focus" [
  ]

  # Toggle window floating state
  export extern "rift-cli help execute window toggle-float" [
  ]

  # Toggle fullscreen mode (fills the whole screen, ignores outer gaps)
  export extern "rift-cli help execute window toggle-fullscreen" [
  ]

  # Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)
  export extern "rift-cli help execute window toggle-fullscreen-within-gaps" [
  ]

  # Grow the current window size (increments by ~5%)
  export extern "rift-cli help execute window resize-grow" [
  ]

  # Shrink the current window size (decrements by ~5%)
  export extern "rift-cli help execute window resize-shrink" [
  ]

  # Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%
  export extern "rift-cli help execute window resize-by" [
  ]

  # Close a window by window server identifier
  export extern "rift-cli help execute window close" [
  ]

  export extern "rift-cli help execute window add-scratchpad" [
  ]

  export extern "rift-cli help execute window toggle-scratchpad" [
  ]

  # Virtual workspace commands
  export extern "rift-cli help execute workspace" [
  ]

  # Switch to next workspace
  export extern "rift-cli help execute workspace next" [
  ]

  # Switch to previous workspace
  export extern "rift-cli help execute workspace prev" [
  ]

  # Switch to specific workspace
  export extern "rift-cli help execute workspace switch" [
  ]

  # Move current window to workspace
  export extern "rift-cli help execute workspace move-window" [
  ]

  # Create a new workspace
  export extern "rift-cli help execute workspace create" [
  ]

  # Switch to the last workspace
  export extern "rift-cli help execute workspace last" [
  ]

  # Set layout mode for a workspace (or active workspace when omitted)
  export extern "rift-cli help execute workspace set-layout" [
  ]

  # Layout commands
  export extern "rift-cli help execute layout" [
  ]

  # Move selection up the tree
  export extern "rift-cli help execute layout ascend" [
  ]

  # Move selection down the tree
  export extern "rift-cli help execute layout descend" [
  ]

  # Move the selected node in a direction
  export extern "rift-cli help execute layout move-node" [
  ]

  # Join the selected window with neighbor in a direction
  export extern "rift-cli help execute layout join-window" [
  ]

  # Toggle stacked state for the selected container
  export extern "rift-cli help execute layout toggle-stack" [
  ]

  # Global orientation toggle that works consistently across layout modes (and between splits/stacks)
  export extern "rift-cli help execute layout toggle-orientation" [
  ]

  # Unjoin previously joined windows
  export extern "rift-cli help execute layout unjoin" [
  ]

  # Toggle floating on the focused selection (tree focus)
  export extern "rift-cli help execute layout toggle-focus-float" [
  ]

  # Adjust master ratio by a delta (master/stack layout only)
  export extern "rift-cli help execute layout adjust-master-ratio" [
  ]

  # Adjust master count by a delta (master/stack layout only)
  export extern "rift-cli help execute layout adjust-master-count" [
  ]

  # Promote the selected window into the master area (master/stack layout only)
  export extern "rift-cli help execute layout promote-to-master" [
  ]

  # Swap the first master with the first stack window (master/stack layout only)
  export extern "rift-cli help execute layout swap-master-stack" [
  ]

  # Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)
  export extern "rift-cli help execute layout swap-windows" [
  ]

  # Scroll the strip by a normalized delta (scrolling layout only)
  export extern "rift-cli help execute layout scroll-strip" [
  ]

  # Snap the strip to the nearest column boundary (scrolling layout only)
  export extern "rift-cli help execute layout snap-strip" [
  ]

  # Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed
  export extern "rift-cli help execute layout center-selection" [
  ]

  # Configuration management commands
  export extern "rift-cli help execute config" [
  ]

  # Update animation settings
  export extern "rift-cli help execute config set-animate" [
  ]

  export extern "rift-cli help execute config set-animation-duration" [
  ]

  export extern "rift-cli help execute config set-animation-fps" [
  ]

  export extern "rift-cli help execute config set-animation-easing" [
  ]

  # Update mouse settings
  export extern "rift-cli help execute config set-mouse-follows-focus" [
  ]

  export extern "rift-cli help execute config set-mouse-hides-on-focus" [
  ]

  export extern "rift-cli help execute config set-focus-follows-mouse" [
  ]

  # Update layout settings
  export extern "rift-cli help execute config set-stack-offset" [
  ]

  # Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"
  export extern "rift-cli help execute config set-stack-default-orientation" [
  ]

  export extern "rift-cli help execute config set-outer-gaps" [
  ]

  export extern "rift-cli help execute config set-inner-gaps" [
  ]

  # Update workspace settings
  export extern "rift-cli help execute config set-workspace-names" [
  ]

  # Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true
  export extern "rift-cli help execute config set" [
  ]

  # Get current config
  export extern "rift-cli help execute config get" [
  ]

  # Save current config to file
  export extern "rift-cli help execute config save" [
  ]

  # Reload config from file
  export extern "rift-cli help execute config reload" [
  ]

  # Mission control commands
  export extern "rift-cli help execute mission-control" [
  ]

  # Show all workspaces in mission control
  export extern "rift-cli help execute mission-control show-all" [
  ]

  # Show current workspace in mission control
  export extern "rift-cli help execute mission-control show-current" [
  ]

  # Dismiss mission control
  export extern "rift-cli help execute mission-control dismiss" [
  ]

  # Display/mouse commands
  export extern "rift-cli help execute display" [
  ]

  # Focus a display by direction, index, or UUID
  export extern "rift-cli help execute display focus" [
  ]

  # Move mouse cursor to a display by index (0-based)
  export extern "rift-cli help execute display move-mouse-to-index" [
  ]

  # Move mouse cursor to a display by UUID
  export extern "rift-cli help execute display move-mouse-to-uuid" [
  ]

  # Move a window to a display by direction, index, or UUID
  export extern "rift-cli help execute display move-window" [
  ]

  # Save current state and exit rift
  export extern "rift-cli help execute save-and-exit" [
  ]

  # Print layout tree debugging output in the running rift instance
  export extern "rift-cli help execute debug" [
  ]

  # Serialize and print runtime state
  export extern "rift-cli help execute serialize" [
  ]

  # Toggle whether the current space is managed by rift
  export extern "rift-cli help execute toggle-space-activated" [
  ]

  # Show timing metrics
  export extern "rift-cli help execute show-timing" [
  ]

  # Event subscription commands
  export extern "rift-cli help subscribe" [
  ]

  # Subscribe to Mach IPC events
  export extern "rift-cli help subscribe mach" [
  ]

  # Subscribe to events via CLI command execution
  export extern "rift-cli help subscribe cli" [
  ]

  # Unsubscribe from Mach IPC events
  export extern "rift-cli help subscribe unsub-mach" [
  ]

  # Unsubscribe from CLI events
  export extern "rift-cli help subscribe unsub-cli" [
  ]

  # List current CLI subscriptions
  export extern "rift-cli help subscribe list-cli" [
  ]

  # Manage the launchd service for rift
  export extern "rift-cli help service" [
  ]

  # Install the per-user launchd service
  export extern "rift-cli help service install" [
  ]

  # Uninstall the per-user launchd service
  export extern "rift-cli help service uninstall" [
  ]

  # Start (or bootstrap) the service
  export extern "rift-cli help service start" [
  ]

  # Stop (or bootout/kill) the service
  export extern "rift-cli help service stop" [
  ]

  # Restart the service (kickstart -k)
  export extern "rift-cli help service restart" [
  ]

  export extern "rift-cli help verify" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift-cli help help" [
  ]

}

export use completions *
