
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'rift-cli' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'rift-cli'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'rift-cli' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('query', 'query', [CompletionResultType]::ParameterValue, 'Query information from rift')
            [CompletionResult]::new('execute', 'execute', [CompletionResultType]::ParameterValue, 'Execute commands in rift')
            [CompletionResult]::new('subscribe', 'subscribe', [CompletionResultType]::ParameterValue, 'Event subscription commands')
            [CompletionResult]::new('service', 'service', [CompletionResultType]::ParameterValue, 'Manage the launchd service for rift')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'verify')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;query' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('workspaces', 'workspaces', [CompletionResultType]::ParameterValue, 'List virtual workspaces (optionally for a specific MacOS space)')
            [CompletionResult]::new('windows', 'windows', [CompletionResultType]::ParameterValue, 'List windows (optionally filtered by space)')
            [CompletionResult]::new('displays', 'displays', [CompletionResultType]::ParameterValue, 'List connected displays')
            [CompletionResult]::new('window', 'window', [CompletionResultType]::ParameterValue, 'Get information about a specific window')
            [CompletionResult]::new('applications', 'applications', [CompletionResultType]::ParameterValue, 'List running applications')
            [CompletionResult]::new('layout', 'layout', [CompletionResultType]::ParameterValue, 'Get layout state for a space')
            [CompletionResult]::new('workspace-layout', 'workspace-layout', [CompletionResultType]::ParameterValue, 'Get workspace layout-engine mode(s)')
            [CompletionResult]::new('metrics', 'metrics', [CompletionResultType]::ParameterValue, 'Get performance metrics')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;query;workspaces' {
            [CompletionResult]::new('--space-id', '--space-id', [CompletionResultType]::ParameterName, 'space-id')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;windows' {
            [CompletionResult]::new('--space-id', '--space-id', [CompletionResultType]::ParameterName, 'space-id')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;displays' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;window' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;applications' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;layout' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;workspace-layout' {
            [CompletionResult]::new('--space-id', '--space-id', [CompletionResultType]::ParameterName, 'space-id')
            [CompletionResult]::new('--workspace-id', '--workspace-id', [CompletionResultType]::ParameterName, 'workspace-id')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;metrics' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;query;help' {
            [CompletionResult]::new('workspaces', 'workspaces', [CompletionResultType]::ParameterValue, 'List virtual workspaces (optionally for a specific MacOS space)')
            [CompletionResult]::new('windows', 'windows', [CompletionResultType]::ParameterValue, 'List windows (optionally filtered by space)')
            [CompletionResult]::new('displays', 'displays', [CompletionResultType]::ParameterValue, 'List connected displays')
            [CompletionResult]::new('window', 'window', [CompletionResultType]::ParameterValue, 'Get information about a specific window')
            [CompletionResult]::new('applications', 'applications', [CompletionResultType]::ParameterValue, 'List running applications')
            [CompletionResult]::new('layout', 'layout', [CompletionResultType]::ParameterValue, 'Get layout state for a space')
            [CompletionResult]::new('workspace-layout', 'workspace-layout', [CompletionResultType]::ParameterValue, 'Get workspace layout-engine mode(s)')
            [CompletionResult]::new('metrics', 'metrics', [CompletionResultType]::ParameterValue, 'Get performance metrics')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;query;help;workspaces' {
            break
        }
        'rift-cli;query;help;windows' {
            break
        }
        'rift-cli;query;help;displays' {
            break
        }
        'rift-cli;query;help;window' {
            break
        }
        'rift-cli;query;help;applications' {
            break
        }
        'rift-cli;query;help;layout' {
            break
        }
        'rift-cli;query;help;workspace-layout' {
            break
        }
        'rift-cli;query;help;metrics' {
            break
        }
        'rift-cli;query;help;help' {
            break
        }
        'rift-cli;execute' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('window', 'window', [CompletionResultType]::ParameterValue, 'Window management commands')
            [CompletionResult]::new('workspace', 'workspace', [CompletionResultType]::ParameterValue, 'Virtual workspace commands')
            [CompletionResult]::new('layout', 'layout', [CompletionResultType]::ParameterValue, 'Layout commands')
            [CompletionResult]::new('config', 'config', [CompletionResultType]::ParameterValue, 'Configuration management commands')
            [CompletionResult]::new('mission-control', 'mission-control', [CompletionResultType]::ParameterValue, 'Mission control commands')
            [CompletionResult]::new('display', 'display', [CompletionResultType]::ParameterValue, 'Display/mouse commands')
            [CompletionResult]::new('save-and-exit', 'save-and-exit', [CompletionResultType]::ParameterValue, 'Save current state and exit rift')
            [CompletionResult]::new('debug', 'debug', [CompletionResultType]::ParameterValue, 'Print layout tree debugging output in the running rift instance')
            [CompletionResult]::new('serialize', 'serialize', [CompletionResultType]::ParameterValue, 'Serialize and print runtime state')
            [CompletionResult]::new('toggle-space-activated', 'toggle-space-activated', [CompletionResultType]::ParameterValue, 'Toggle whether the current space is managed by rift')
            [CompletionResult]::new('show-timing', 'show-timing', [CompletionResultType]::ParameterValue, 'Show timing metrics')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;window' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Focus the next window')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Focus the previous window')
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Move focus in a direction')
            [CompletionResult]::new('toggle-float', 'toggle-float', [CompletionResultType]::ParameterValue, 'Toggle window floating state')
            [CompletionResult]::new('toggle-fullscreen', 'toggle-fullscreen', [CompletionResultType]::ParameterValue, 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)')
            [CompletionResult]::new('toggle-fullscreen-within-gaps', 'toggle-fullscreen-within-gaps', [CompletionResultType]::ParameterValue, 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)')
            [CompletionResult]::new('resize-grow', 'resize-grow', [CompletionResultType]::ParameterValue, 'Grow the current window size (increments by ~5%)')
            [CompletionResult]::new('resize-shrink', 'resize-shrink', [CompletionResultType]::ParameterValue, 'Shrink the current window size (decrements by ~5%)')
            [CompletionResult]::new('resize-by', 'resize-by', [CompletionResultType]::ParameterValue, 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%')
            [CompletionResult]::new('close', 'close', [CompletionResultType]::ParameterValue, 'Close a window by window server identifier')
            [CompletionResult]::new('add-scratchpad', 'add-scratchpad', [CompletionResultType]::ParameterValue, 'add-scratchpad')
            [CompletionResult]::new('toggle-scratchpad', 'toggle-scratchpad', [CompletionResultType]::ParameterValue, 'toggle-scratchpad')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;window;next' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;prev' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;focus' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;toggle-float' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;toggle-fullscreen' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;toggle-fullscreen-within-gaps' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;resize-grow' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;resize-shrink' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;resize-by' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;close' {
            [CompletionResult]::new('--window-id', '--window-id', [CompletionResultType]::ParameterName, 'Window Id (window server id or idx from window id)')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;add-scratchpad' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;toggle-scratchpad' {
            [CompletionResult]::new('--name', '--name', [CompletionResultType]::ParameterName, 'Name of the scratchpad (optional, defaults to "default")')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;window;help' {
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Focus the next window')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Focus the previous window')
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Move focus in a direction')
            [CompletionResult]::new('toggle-float', 'toggle-float', [CompletionResultType]::ParameterValue, 'Toggle window floating state')
            [CompletionResult]::new('toggle-fullscreen', 'toggle-fullscreen', [CompletionResultType]::ParameterValue, 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)')
            [CompletionResult]::new('toggle-fullscreen-within-gaps', 'toggle-fullscreen-within-gaps', [CompletionResultType]::ParameterValue, 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)')
            [CompletionResult]::new('resize-grow', 'resize-grow', [CompletionResultType]::ParameterValue, 'Grow the current window size (increments by ~5%)')
            [CompletionResult]::new('resize-shrink', 'resize-shrink', [CompletionResultType]::ParameterValue, 'Shrink the current window size (decrements by ~5%)')
            [CompletionResult]::new('resize-by', 'resize-by', [CompletionResultType]::ParameterValue, 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%')
            [CompletionResult]::new('close', 'close', [CompletionResultType]::ParameterValue, 'Close a window by window server identifier')
            [CompletionResult]::new('add-scratchpad', 'add-scratchpad', [CompletionResultType]::ParameterValue, 'add-scratchpad')
            [CompletionResult]::new('toggle-scratchpad', 'toggle-scratchpad', [CompletionResultType]::ParameterValue, 'toggle-scratchpad')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;window;help;next' {
            break
        }
        'rift-cli;execute;window;help;prev' {
            break
        }
        'rift-cli;execute;window;help;focus' {
            break
        }
        'rift-cli;execute;window;help;toggle-float' {
            break
        }
        'rift-cli;execute;window;help;toggle-fullscreen' {
            break
        }
        'rift-cli;execute;window;help;toggle-fullscreen-within-gaps' {
            break
        }
        'rift-cli;execute;window;help;resize-grow' {
            break
        }
        'rift-cli;execute;window;help;resize-shrink' {
            break
        }
        'rift-cli;execute;window;help;resize-by' {
            break
        }
        'rift-cli;execute;window;help;close' {
            break
        }
        'rift-cli;execute;window;help;add-scratchpad' {
            break
        }
        'rift-cli;execute;window;help;toggle-scratchpad' {
            break
        }
        'rift-cli;execute;window;help;help' {
            break
        }
        'rift-cli;execute;workspace' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Switch to next workspace')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Switch to previous workspace')
            [CompletionResult]::new('switch', 'switch', [CompletionResultType]::ParameterValue, 'Switch to specific workspace')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move current window to workspace')
            [CompletionResult]::new('create', 'create', [CompletionResultType]::ParameterValue, 'Create a new workspace')
            [CompletionResult]::new('last', 'last', [CompletionResultType]::ParameterValue, 'Switch to the last workspace')
            [CompletionResult]::new('set-layout', 'set-layout', [CompletionResultType]::ParameterValue, 'Set layout mode for a workspace (or active workspace when omitted)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;workspace;next' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;prev' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;switch' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;move-window' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;create' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;last' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;set-layout' {
            [CompletionResult]::new('--workspace-id', '--workspace-id', [CompletionResultType]::ParameterName, 'Workspace index (0-based). Defaults to active workspace if omitted')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;workspace;help' {
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Switch to next workspace')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Switch to previous workspace')
            [CompletionResult]::new('switch', 'switch', [CompletionResultType]::ParameterValue, 'Switch to specific workspace')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move current window to workspace')
            [CompletionResult]::new('create', 'create', [CompletionResultType]::ParameterValue, 'Create a new workspace')
            [CompletionResult]::new('last', 'last', [CompletionResultType]::ParameterValue, 'Switch to the last workspace')
            [CompletionResult]::new('set-layout', 'set-layout', [CompletionResultType]::ParameterValue, 'Set layout mode for a workspace (or active workspace when omitted)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;workspace;help;next' {
            break
        }
        'rift-cli;execute;workspace;help;prev' {
            break
        }
        'rift-cli;execute;workspace;help;switch' {
            break
        }
        'rift-cli;execute;workspace;help;move-window' {
            break
        }
        'rift-cli;execute;workspace;help;create' {
            break
        }
        'rift-cli;execute;workspace;help;last' {
            break
        }
        'rift-cli;execute;workspace;help;set-layout' {
            break
        }
        'rift-cli;execute;workspace;help;help' {
            break
        }
        'rift-cli;execute;layout' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('ascend', 'ascend', [CompletionResultType]::ParameterValue, 'Move selection up the tree')
            [CompletionResult]::new('descend', 'descend', [CompletionResultType]::ParameterValue, 'Move selection down the tree')
            [CompletionResult]::new('move-node', 'move-node', [CompletionResultType]::ParameterValue, 'Move the selected node in a direction')
            [CompletionResult]::new('join-window', 'join-window', [CompletionResultType]::ParameterValue, 'Join the selected window with neighbor in a direction')
            [CompletionResult]::new('toggle-stack', 'toggle-stack', [CompletionResultType]::ParameterValue, 'Toggle stacked state for the selected container')
            [CompletionResult]::new('toggle-orientation', 'toggle-orientation', [CompletionResultType]::ParameterValue, 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)')
            [CompletionResult]::new('unjoin', 'unjoin', [CompletionResultType]::ParameterValue, 'Unjoin previously joined windows')
            [CompletionResult]::new('toggle-focus-float', 'toggle-focus-float', [CompletionResultType]::ParameterValue, 'Toggle floating on the focused selection (tree focus)')
            [CompletionResult]::new('adjust-master-ratio', 'adjust-master-ratio', [CompletionResultType]::ParameterValue, 'Adjust master ratio by a delta (master/stack layout only)')
            [CompletionResult]::new('adjust-master-count', 'adjust-master-count', [CompletionResultType]::ParameterValue, 'Adjust master count by a delta (master/stack layout only)')
            [CompletionResult]::new('promote-to-master', 'promote-to-master', [CompletionResultType]::ParameterValue, 'Promote the selected window into the master area (master/stack layout only)')
            [CompletionResult]::new('swap-master-stack', 'swap-master-stack', [CompletionResultType]::ParameterValue, 'Swap the first master with the first stack window (master/stack layout only)')
            [CompletionResult]::new('swap-windows', 'swap-windows', [CompletionResultType]::ParameterValue, 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)')
            [CompletionResult]::new('scroll-strip', 'scroll-strip', [CompletionResultType]::ParameterValue, 'Scroll the strip by a normalized delta (scrolling layout only)')
            [CompletionResult]::new('snap-strip', 'snap-strip', [CompletionResultType]::ParameterValue, 'Snap the strip to the nearest column boundary (scrolling layout only)')
            [CompletionResult]::new('center-selection', 'center-selection', [CompletionResultType]::ParameterValue, 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;layout;ascend' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;descend' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;move-node' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;join-window' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;toggle-stack' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;toggle-orientation' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;unjoin' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;toggle-focus-float' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;adjust-master-ratio' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;adjust-master-count' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;promote-to-master' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;swap-master-stack' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;swap-windows' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;scroll-strip' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;snap-strip' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;center-selection' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;layout;help' {
            [CompletionResult]::new('ascend', 'ascend', [CompletionResultType]::ParameterValue, 'Move selection up the tree')
            [CompletionResult]::new('descend', 'descend', [CompletionResultType]::ParameterValue, 'Move selection down the tree')
            [CompletionResult]::new('move-node', 'move-node', [CompletionResultType]::ParameterValue, 'Move the selected node in a direction')
            [CompletionResult]::new('join-window', 'join-window', [CompletionResultType]::ParameterValue, 'Join the selected window with neighbor in a direction')
            [CompletionResult]::new('toggle-stack', 'toggle-stack', [CompletionResultType]::ParameterValue, 'Toggle stacked state for the selected container')
            [CompletionResult]::new('toggle-orientation', 'toggle-orientation', [CompletionResultType]::ParameterValue, 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)')
            [CompletionResult]::new('unjoin', 'unjoin', [CompletionResultType]::ParameterValue, 'Unjoin previously joined windows')
            [CompletionResult]::new('toggle-focus-float', 'toggle-focus-float', [CompletionResultType]::ParameterValue, 'Toggle floating on the focused selection (tree focus)')
            [CompletionResult]::new('adjust-master-ratio', 'adjust-master-ratio', [CompletionResultType]::ParameterValue, 'Adjust master ratio by a delta (master/stack layout only)')
            [CompletionResult]::new('adjust-master-count', 'adjust-master-count', [CompletionResultType]::ParameterValue, 'Adjust master count by a delta (master/stack layout only)')
            [CompletionResult]::new('promote-to-master', 'promote-to-master', [CompletionResultType]::ParameterValue, 'Promote the selected window into the master area (master/stack layout only)')
            [CompletionResult]::new('swap-master-stack', 'swap-master-stack', [CompletionResultType]::ParameterValue, 'Swap the first master with the first stack window (master/stack layout only)')
            [CompletionResult]::new('swap-windows', 'swap-windows', [CompletionResultType]::ParameterValue, 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)')
            [CompletionResult]::new('scroll-strip', 'scroll-strip', [CompletionResultType]::ParameterValue, 'Scroll the strip by a normalized delta (scrolling layout only)')
            [CompletionResult]::new('snap-strip', 'snap-strip', [CompletionResultType]::ParameterValue, 'Snap the strip to the nearest column boundary (scrolling layout only)')
            [CompletionResult]::new('center-selection', 'center-selection', [CompletionResultType]::ParameterValue, 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;layout;help;ascend' {
            break
        }
        'rift-cli;execute;layout;help;descend' {
            break
        }
        'rift-cli;execute;layout;help;move-node' {
            break
        }
        'rift-cli;execute;layout;help;join-window' {
            break
        }
        'rift-cli;execute;layout;help;toggle-stack' {
            break
        }
        'rift-cli;execute;layout;help;toggle-orientation' {
            break
        }
        'rift-cli;execute;layout;help;unjoin' {
            break
        }
        'rift-cli;execute;layout;help;toggle-focus-float' {
            break
        }
        'rift-cli;execute;layout;help;adjust-master-ratio' {
            break
        }
        'rift-cli;execute;layout;help;adjust-master-count' {
            break
        }
        'rift-cli;execute;layout;help;promote-to-master' {
            break
        }
        'rift-cli;execute;layout;help;swap-master-stack' {
            break
        }
        'rift-cli;execute;layout;help;swap-windows' {
            break
        }
        'rift-cli;execute;layout;help;scroll-strip' {
            break
        }
        'rift-cli;execute;layout;help;snap-strip' {
            break
        }
        'rift-cli;execute;layout;help;center-selection' {
            break
        }
        'rift-cli;execute;layout;help;help' {
            break
        }
        'rift-cli;execute;config' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('set-animate', 'set-animate', [CompletionResultType]::ParameterValue, 'Update animation settings')
            [CompletionResult]::new('set-animation-duration', 'set-animation-duration', [CompletionResultType]::ParameterValue, 'set-animation-duration')
            [CompletionResult]::new('set-animation-fps', 'set-animation-fps', [CompletionResultType]::ParameterValue, 'set-animation-fps')
            [CompletionResult]::new('set-animation-easing', 'set-animation-easing', [CompletionResultType]::ParameterValue, 'set-animation-easing')
            [CompletionResult]::new('set-mouse-follows-focus', 'set-mouse-follows-focus', [CompletionResultType]::ParameterValue, 'Update mouse settings')
            [CompletionResult]::new('set-mouse-hides-on-focus', 'set-mouse-hides-on-focus', [CompletionResultType]::ParameterValue, 'set-mouse-hides-on-focus')
            [CompletionResult]::new('set-focus-follows-mouse', 'set-focus-follows-mouse', [CompletionResultType]::ParameterValue, 'set-focus-follows-mouse')
            [CompletionResult]::new('set-stack-offset', 'set-stack-offset', [CompletionResultType]::ParameterValue, 'Update layout settings')
            [CompletionResult]::new('set-stack-default-orientation', 'set-stack-default-orientation', [CompletionResultType]::ParameterValue, 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"')
            [CompletionResult]::new('set-outer-gaps', 'set-outer-gaps', [CompletionResultType]::ParameterValue, 'set-outer-gaps')
            [CompletionResult]::new('set-inner-gaps', 'set-inner-gaps', [CompletionResultType]::ParameterValue, 'set-inner-gaps')
            [CompletionResult]::new('set-workspace-names', 'set-workspace-names', [CompletionResultType]::ParameterValue, 'Update workspace settings')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Get current config')
            [CompletionResult]::new('save', 'save', [CompletionResultType]::ParameterValue, 'Save current config to file')
            [CompletionResult]::new('reload', 'reload', [CompletionResultType]::ParameterValue, 'Reload config from file')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;config;set-animate' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-animation-duration' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-animation-fps' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-animation-easing' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-mouse-follows-focus' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-mouse-hides-on-focus' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-focus-follows-mouse' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-stack-offset' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-stack-default-orientation' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-outer-gaps' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-inner-gaps' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set-workspace-names' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;set' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;get' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;save' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;reload' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;config;help' {
            [CompletionResult]::new('set-animate', 'set-animate', [CompletionResultType]::ParameterValue, 'Update animation settings')
            [CompletionResult]::new('set-animation-duration', 'set-animation-duration', [CompletionResultType]::ParameterValue, 'set-animation-duration')
            [CompletionResult]::new('set-animation-fps', 'set-animation-fps', [CompletionResultType]::ParameterValue, 'set-animation-fps')
            [CompletionResult]::new('set-animation-easing', 'set-animation-easing', [CompletionResultType]::ParameterValue, 'set-animation-easing')
            [CompletionResult]::new('set-mouse-follows-focus', 'set-mouse-follows-focus', [CompletionResultType]::ParameterValue, 'Update mouse settings')
            [CompletionResult]::new('set-mouse-hides-on-focus', 'set-mouse-hides-on-focus', [CompletionResultType]::ParameterValue, 'set-mouse-hides-on-focus')
            [CompletionResult]::new('set-focus-follows-mouse', 'set-focus-follows-mouse', [CompletionResultType]::ParameterValue, 'set-focus-follows-mouse')
            [CompletionResult]::new('set-stack-offset', 'set-stack-offset', [CompletionResultType]::ParameterValue, 'Update layout settings')
            [CompletionResult]::new('set-stack-default-orientation', 'set-stack-default-orientation', [CompletionResultType]::ParameterValue, 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"')
            [CompletionResult]::new('set-outer-gaps', 'set-outer-gaps', [CompletionResultType]::ParameterValue, 'set-outer-gaps')
            [CompletionResult]::new('set-inner-gaps', 'set-inner-gaps', [CompletionResultType]::ParameterValue, 'set-inner-gaps')
            [CompletionResult]::new('set-workspace-names', 'set-workspace-names', [CompletionResultType]::ParameterValue, 'Update workspace settings')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Get current config')
            [CompletionResult]::new('save', 'save', [CompletionResultType]::ParameterValue, 'Save current config to file')
            [CompletionResult]::new('reload', 'reload', [CompletionResultType]::ParameterValue, 'Reload config from file')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;config;help;set-animate' {
            break
        }
        'rift-cli;execute;config;help;set-animation-duration' {
            break
        }
        'rift-cli;execute;config;help;set-animation-fps' {
            break
        }
        'rift-cli;execute;config;help;set-animation-easing' {
            break
        }
        'rift-cli;execute;config;help;set-mouse-follows-focus' {
            break
        }
        'rift-cli;execute;config;help;set-mouse-hides-on-focus' {
            break
        }
        'rift-cli;execute;config;help;set-focus-follows-mouse' {
            break
        }
        'rift-cli;execute;config;help;set-stack-offset' {
            break
        }
        'rift-cli;execute;config;help;set-stack-default-orientation' {
            break
        }
        'rift-cli;execute;config;help;set-outer-gaps' {
            break
        }
        'rift-cli;execute;config;help;set-inner-gaps' {
            break
        }
        'rift-cli;execute;config;help;set-workspace-names' {
            break
        }
        'rift-cli;execute;config;help;set' {
            break
        }
        'rift-cli;execute;config;help;get' {
            break
        }
        'rift-cli;execute;config;help;save' {
            break
        }
        'rift-cli;execute;config;help;reload' {
            break
        }
        'rift-cli;execute;config;help;help' {
            break
        }
        'rift-cli;execute;mission-control' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('show-all', 'show-all', [CompletionResultType]::ParameterValue, 'Show all workspaces in mission control')
            [CompletionResult]::new('show-current', 'show-current', [CompletionResultType]::ParameterValue, 'Show current workspace in mission control')
            [CompletionResult]::new('dismiss', 'dismiss', [CompletionResultType]::ParameterValue, 'Dismiss mission control')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;mission-control;show-all' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;mission-control;show-current' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;mission-control;dismiss' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;mission-control;help' {
            [CompletionResult]::new('show-all', 'show-all', [CompletionResultType]::ParameterValue, 'Show all workspaces in mission control')
            [CompletionResult]::new('show-current', 'show-current', [CompletionResultType]::ParameterValue, 'Show current workspace in mission control')
            [CompletionResult]::new('dismiss', 'dismiss', [CompletionResultType]::ParameterValue, 'Dismiss mission control')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;mission-control;help;show-all' {
            break
        }
        'rift-cli;execute;mission-control;help;show-current' {
            break
        }
        'rift-cli;execute;mission-control;help;dismiss' {
            break
        }
        'rift-cli;execute;mission-control;help;help' {
            break
        }
        'rift-cli;execute;display' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Focus a display by direction, index, or UUID')
            [CompletionResult]::new('move-mouse-to-index', 'move-mouse-to-index', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by index (0-based)')
            [CompletionResult]::new('move-mouse-to-uuid', 'move-mouse-to-uuid', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by UUID')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move a window to a display by direction, index, or UUID')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;display;focus' {
            [CompletionResult]::new('--direction', '--direction', [CompletionResultType]::ParameterName, 'Direction relative to the current display (left, right, up, down)')
            [CompletionResult]::new('--index', '--index', [CompletionResultType]::ParameterName, 'Display index (0-based)')
            [CompletionResult]::new('--uuid', '--uuid', [CompletionResultType]::ParameterName, 'Display UUID')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;display;move-mouse-to-index' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;display;move-mouse-to-uuid' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;display;move-window' {
            [CompletionResult]::new('--direction', '--direction', [CompletionResultType]::ParameterName, 'Direction relative to the window''s current display (left, right, up, down)')
            [CompletionResult]::new('--index', '--index', [CompletionResultType]::ParameterName, 'Display index (0-based)')
            [CompletionResult]::new('--uuid', '--uuid', [CompletionResultType]::ParameterName, 'Display UUID')
            [CompletionResult]::new('--window-id', '--window-id', [CompletionResultType]::ParameterName, 'Optional window id (window idx); defaults to the focused window if omitted')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;display;help' {
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Focus a display by direction, index, or UUID')
            [CompletionResult]::new('move-mouse-to-index', 'move-mouse-to-index', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by index (0-based)')
            [CompletionResult]::new('move-mouse-to-uuid', 'move-mouse-to-uuid', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by UUID')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move a window to a display by direction, index, or UUID')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;display;help;focus' {
            break
        }
        'rift-cli;execute;display;help;move-mouse-to-index' {
            break
        }
        'rift-cli;execute;display;help;move-mouse-to-uuid' {
            break
        }
        'rift-cli;execute;display;help;move-window' {
            break
        }
        'rift-cli;execute;display;help;help' {
            break
        }
        'rift-cli;execute;save-and-exit' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;debug' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;serialize' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;toggle-space-activated' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;show-timing' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;execute;help' {
            [CompletionResult]::new('window', 'window', [CompletionResultType]::ParameterValue, 'Window management commands')
            [CompletionResult]::new('workspace', 'workspace', [CompletionResultType]::ParameterValue, 'Virtual workspace commands')
            [CompletionResult]::new('layout', 'layout', [CompletionResultType]::ParameterValue, 'Layout commands')
            [CompletionResult]::new('config', 'config', [CompletionResultType]::ParameterValue, 'Configuration management commands')
            [CompletionResult]::new('mission-control', 'mission-control', [CompletionResultType]::ParameterValue, 'Mission control commands')
            [CompletionResult]::new('display', 'display', [CompletionResultType]::ParameterValue, 'Display/mouse commands')
            [CompletionResult]::new('save-and-exit', 'save-and-exit', [CompletionResultType]::ParameterValue, 'Save current state and exit rift')
            [CompletionResult]::new('debug', 'debug', [CompletionResultType]::ParameterValue, 'Print layout tree debugging output in the running rift instance')
            [CompletionResult]::new('serialize', 'serialize', [CompletionResultType]::ParameterValue, 'Serialize and print runtime state')
            [CompletionResult]::new('toggle-space-activated', 'toggle-space-activated', [CompletionResultType]::ParameterValue, 'Toggle whether the current space is managed by rift')
            [CompletionResult]::new('show-timing', 'show-timing', [CompletionResultType]::ParameterValue, 'Show timing metrics')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;execute;help;window' {
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Focus the next window')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Focus the previous window')
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Move focus in a direction')
            [CompletionResult]::new('toggle-float', 'toggle-float', [CompletionResultType]::ParameterValue, 'Toggle window floating state')
            [CompletionResult]::new('toggle-fullscreen', 'toggle-fullscreen', [CompletionResultType]::ParameterValue, 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)')
            [CompletionResult]::new('toggle-fullscreen-within-gaps', 'toggle-fullscreen-within-gaps', [CompletionResultType]::ParameterValue, 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)')
            [CompletionResult]::new('resize-grow', 'resize-grow', [CompletionResultType]::ParameterValue, 'Grow the current window size (increments by ~5%)')
            [CompletionResult]::new('resize-shrink', 'resize-shrink', [CompletionResultType]::ParameterValue, 'Shrink the current window size (decrements by ~5%)')
            [CompletionResult]::new('resize-by', 'resize-by', [CompletionResultType]::ParameterValue, 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%')
            [CompletionResult]::new('close', 'close', [CompletionResultType]::ParameterValue, 'Close a window by window server identifier')
            [CompletionResult]::new('add-scratchpad', 'add-scratchpad', [CompletionResultType]::ParameterValue, 'add-scratchpad')
            [CompletionResult]::new('toggle-scratchpad', 'toggle-scratchpad', [CompletionResultType]::ParameterValue, 'toggle-scratchpad')
            break
        }
        'rift-cli;execute;help;window;next' {
            break
        }
        'rift-cli;execute;help;window;prev' {
            break
        }
        'rift-cli;execute;help;window;focus' {
            break
        }
        'rift-cli;execute;help;window;toggle-float' {
            break
        }
        'rift-cli;execute;help;window;toggle-fullscreen' {
            break
        }
        'rift-cli;execute;help;window;toggle-fullscreen-within-gaps' {
            break
        }
        'rift-cli;execute;help;window;resize-grow' {
            break
        }
        'rift-cli;execute;help;window;resize-shrink' {
            break
        }
        'rift-cli;execute;help;window;resize-by' {
            break
        }
        'rift-cli;execute;help;window;close' {
            break
        }
        'rift-cli;execute;help;window;add-scratchpad' {
            break
        }
        'rift-cli;execute;help;window;toggle-scratchpad' {
            break
        }
        'rift-cli;execute;help;workspace' {
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Switch to next workspace')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Switch to previous workspace')
            [CompletionResult]::new('switch', 'switch', [CompletionResultType]::ParameterValue, 'Switch to specific workspace')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move current window to workspace')
            [CompletionResult]::new('create', 'create', [CompletionResultType]::ParameterValue, 'Create a new workspace')
            [CompletionResult]::new('last', 'last', [CompletionResultType]::ParameterValue, 'Switch to the last workspace')
            [CompletionResult]::new('set-layout', 'set-layout', [CompletionResultType]::ParameterValue, 'Set layout mode for a workspace (or active workspace when omitted)')
            break
        }
        'rift-cli;execute;help;workspace;next' {
            break
        }
        'rift-cli;execute;help;workspace;prev' {
            break
        }
        'rift-cli;execute;help;workspace;switch' {
            break
        }
        'rift-cli;execute;help;workspace;move-window' {
            break
        }
        'rift-cli;execute;help;workspace;create' {
            break
        }
        'rift-cli;execute;help;workspace;last' {
            break
        }
        'rift-cli;execute;help;workspace;set-layout' {
            break
        }
        'rift-cli;execute;help;layout' {
            [CompletionResult]::new('ascend', 'ascend', [CompletionResultType]::ParameterValue, 'Move selection up the tree')
            [CompletionResult]::new('descend', 'descend', [CompletionResultType]::ParameterValue, 'Move selection down the tree')
            [CompletionResult]::new('move-node', 'move-node', [CompletionResultType]::ParameterValue, 'Move the selected node in a direction')
            [CompletionResult]::new('join-window', 'join-window', [CompletionResultType]::ParameterValue, 'Join the selected window with neighbor in a direction')
            [CompletionResult]::new('toggle-stack', 'toggle-stack', [CompletionResultType]::ParameterValue, 'Toggle stacked state for the selected container')
            [CompletionResult]::new('toggle-orientation', 'toggle-orientation', [CompletionResultType]::ParameterValue, 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)')
            [CompletionResult]::new('unjoin', 'unjoin', [CompletionResultType]::ParameterValue, 'Unjoin previously joined windows')
            [CompletionResult]::new('toggle-focus-float', 'toggle-focus-float', [CompletionResultType]::ParameterValue, 'Toggle floating on the focused selection (tree focus)')
            [CompletionResult]::new('adjust-master-ratio', 'adjust-master-ratio', [CompletionResultType]::ParameterValue, 'Adjust master ratio by a delta (master/stack layout only)')
            [CompletionResult]::new('adjust-master-count', 'adjust-master-count', [CompletionResultType]::ParameterValue, 'Adjust master count by a delta (master/stack layout only)')
            [CompletionResult]::new('promote-to-master', 'promote-to-master', [CompletionResultType]::ParameterValue, 'Promote the selected window into the master area (master/stack layout only)')
            [CompletionResult]::new('swap-master-stack', 'swap-master-stack', [CompletionResultType]::ParameterValue, 'Swap the first master with the first stack window (master/stack layout only)')
            [CompletionResult]::new('swap-windows', 'swap-windows', [CompletionResultType]::ParameterValue, 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)')
            [CompletionResult]::new('scroll-strip', 'scroll-strip', [CompletionResultType]::ParameterValue, 'Scroll the strip by a normalized delta (scrolling layout only)')
            [CompletionResult]::new('snap-strip', 'snap-strip', [CompletionResultType]::ParameterValue, 'Snap the strip to the nearest column boundary (scrolling layout only)')
            [CompletionResult]::new('center-selection', 'center-selection', [CompletionResultType]::ParameterValue, 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed')
            break
        }
        'rift-cli;execute;help;layout;ascend' {
            break
        }
        'rift-cli;execute;help;layout;descend' {
            break
        }
        'rift-cli;execute;help;layout;move-node' {
            break
        }
        'rift-cli;execute;help;layout;join-window' {
            break
        }
        'rift-cli;execute;help;layout;toggle-stack' {
            break
        }
        'rift-cli;execute;help;layout;toggle-orientation' {
            break
        }
        'rift-cli;execute;help;layout;unjoin' {
            break
        }
        'rift-cli;execute;help;layout;toggle-focus-float' {
            break
        }
        'rift-cli;execute;help;layout;adjust-master-ratio' {
            break
        }
        'rift-cli;execute;help;layout;adjust-master-count' {
            break
        }
        'rift-cli;execute;help;layout;promote-to-master' {
            break
        }
        'rift-cli;execute;help;layout;swap-master-stack' {
            break
        }
        'rift-cli;execute;help;layout;swap-windows' {
            break
        }
        'rift-cli;execute;help;layout;scroll-strip' {
            break
        }
        'rift-cli;execute;help;layout;snap-strip' {
            break
        }
        'rift-cli;execute;help;layout;center-selection' {
            break
        }
        'rift-cli;execute;help;config' {
            [CompletionResult]::new('set-animate', 'set-animate', [CompletionResultType]::ParameterValue, 'Update animation settings')
            [CompletionResult]::new('set-animation-duration', 'set-animation-duration', [CompletionResultType]::ParameterValue, 'set-animation-duration')
            [CompletionResult]::new('set-animation-fps', 'set-animation-fps', [CompletionResultType]::ParameterValue, 'set-animation-fps')
            [CompletionResult]::new('set-animation-easing', 'set-animation-easing', [CompletionResultType]::ParameterValue, 'set-animation-easing')
            [CompletionResult]::new('set-mouse-follows-focus', 'set-mouse-follows-focus', [CompletionResultType]::ParameterValue, 'Update mouse settings')
            [CompletionResult]::new('set-mouse-hides-on-focus', 'set-mouse-hides-on-focus', [CompletionResultType]::ParameterValue, 'set-mouse-hides-on-focus')
            [CompletionResult]::new('set-focus-follows-mouse', 'set-focus-follows-mouse', [CompletionResultType]::ParameterValue, 'set-focus-follows-mouse')
            [CompletionResult]::new('set-stack-offset', 'set-stack-offset', [CompletionResultType]::ParameterValue, 'Update layout settings')
            [CompletionResult]::new('set-stack-default-orientation', 'set-stack-default-orientation', [CompletionResultType]::ParameterValue, 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"')
            [CompletionResult]::new('set-outer-gaps', 'set-outer-gaps', [CompletionResultType]::ParameterValue, 'set-outer-gaps')
            [CompletionResult]::new('set-inner-gaps', 'set-inner-gaps', [CompletionResultType]::ParameterValue, 'set-inner-gaps')
            [CompletionResult]::new('set-workspace-names', 'set-workspace-names', [CompletionResultType]::ParameterValue, 'Update workspace settings')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Get current config')
            [CompletionResult]::new('save', 'save', [CompletionResultType]::ParameterValue, 'Save current config to file')
            [CompletionResult]::new('reload', 'reload', [CompletionResultType]::ParameterValue, 'Reload config from file')
            break
        }
        'rift-cli;execute;help;config;set-animate' {
            break
        }
        'rift-cli;execute;help;config;set-animation-duration' {
            break
        }
        'rift-cli;execute;help;config;set-animation-fps' {
            break
        }
        'rift-cli;execute;help;config;set-animation-easing' {
            break
        }
        'rift-cli;execute;help;config;set-mouse-follows-focus' {
            break
        }
        'rift-cli;execute;help;config;set-mouse-hides-on-focus' {
            break
        }
        'rift-cli;execute;help;config;set-focus-follows-mouse' {
            break
        }
        'rift-cli;execute;help;config;set-stack-offset' {
            break
        }
        'rift-cli;execute;help;config;set-stack-default-orientation' {
            break
        }
        'rift-cli;execute;help;config;set-outer-gaps' {
            break
        }
        'rift-cli;execute;help;config;set-inner-gaps' {
            break
        }
        'rift-cli;execute;help;config;set-workspace-names' {
            break
        }
        'rift-cli;execute;help;config;set' {
            break
        }
        'rift-cli;execute;help;config;get' {
            break
        }
        'rift-cli;execute;help;config;save' {
            break
        }
        'rift-cli;execute;help;config;reload' {
            break
        }
        'rift-cli;execute;help;mission-control' {
            [CompletionResult]::new('show-all', 'show-all', [CompletionResultType]::ParameterValue, 'Show all workspaces in mission control')
            [CompletionResult]::new('show-current', 'show-current', [CompletionResultType]::ParameterValue, 'Show current workspace in mission control')
            [CompletionResult]::new('dismiss', 'dismiss', [CompletionResultType]::ParameterValue, 'Dismiss mission control')
            break
        }
        'rift-cli;execute;help;mission-control;show-all' {
            break
        }
        'rift-cli;execute;help;mission-control;show-current' {
            break
        }
        'rift-cli;execute;help;mission-control;dismiss' {
            break
        }
        'rift-cli;execute;help;display' {
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Focus a display by direction, index, or UUID')
            [CompletionResult]::new('move-mouse-to-index', 'move-mouse-to-index', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by index (0-based)')
            [CompletionResult]::new('move-mouse-to-uuid', 'move-mouse-to-uuid', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by UUID')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move a window to a display by direction, index, or UUID')
            break
        }
        'rift-cli;execute;help;display;focus' {
            break
        }
        'rift-cli;execute;help;display;move-mouse-to-index' {
            break
        }
        'rift-cli;execute;help;display;move-mouse-to-uuid' {
            break
        }
        'rift-cli;execute;help;display;move-window' {
            break
        }
        'rift-cli;execute;help;save-and-exit' {
            break
        }
        'rift-cli;execute;help;debug' {
            break
        }
        'rift-cli;execute;help;serialize' {
            break
        }
        'rift-cli;execute;help;toggle-space-activated' {
            break
        }
        'rift-cli;execute;help;show-timing' {
            break
        }
        'rift-cli;execute;help;help' {
            break
        }
        'rift-cli;subscribe' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('mach', 'mach', [CompletionResultType]::ParameterValue, 'Subscribe to Mach IPC events')
            [CompletionResult]::new('cli', 'cli', [CompletionResultType]::ParameterValue, 'Subscribe to events via CLI command execution')
            [CompletionResult]::new('unsub-mach', 'unsub-mach', [CompletionResultType]::ParameterValue, 'Unsubscribe from Mach IPC events')
            [CompletionResult]::new('unsub-cli', 'unsub-cli', [CompletionResultType]::ParameterValue, 'Unsubscribe from CLI events')
            [CompletionResult]::new('list-cli', 'list-cli', [CompletionResultType]::ParameterValue, 'List current CLI subscriptions')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;subscribe;mach' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;subscribe;cli' {
            [CompletionResult]::new('--event', '--event', [CompletionResultType]::ParameterName, 'Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)')
            [CompletionResult]::new('--command', '--command', [CompletionResultType]::ParameterName, 'Command to execute when event occurs')
            [CompletionResult]::new('--args', '--args', [CompletionResultType]::ParameterName, 'Arguments to pass to command (event data will be appended as JSON)')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;subscribe;unsub-mach' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;subscribe;unsub-cli' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;subscribe;list-cli' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;subscribe;help' {
            [CompletionResult]::new('mach', 'mach', [CompletionResultType]::ParameterValue, 'Subscribe to Mach IPC events')
            [CompletionResult]::new('cli', 'cli', [CompletionResultType]::ParameterValue, 'Subscribe to events via CLI command execution')
            [CompletionResult]::new('unsub-mach', 'unsub-mach', [CompletionResultType]::ParameterValue, 'Unsubscribe from Mach IPC events')
            [CompletionResult]::new('unsub-cli', 'unsub-cli', [CompletionResultType]::ParameterValue, 'Unsubscribe from CLI events')
            [CompletionResult]::new('list-cli', 'list-cli', [CompletionResultType]::ParameterValue, 'List current CLI subscriptions')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;subscribe;help;mach' {
            break
        }
        'rift-cli;subscribe;help;cli' {
            break
        }
        'rift-cli;subscribe;help;unsub-mach' {
            break
        }
        'rift-cli;subscribe;help;unsub-cli' {
            break
        }
        'rift-cli;subscribe;help;list-cli' {
            break
        }
        'rift-cli;subscribe;help;help' {
            break
        }
        'rift-cli;service' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Install the per-user launchd service')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Uninstall the per-user launchd service')
            [CompletionResult]::new('start', 'start', [CompletionResultType]::ParameterValue, 'Start (or bootstrap) the service')
            [CompletionResult]::new('stop', 'stop', [CompletionResultType]::ParameterValue, 'Stop (or bootout/kill) the service')
            [CompletionResult]::new('restart', 'restart', [CompletionResultType]::ParameterValue, 'Restart the service (kickstart -k)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;service;install' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;service;uninstall' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;service;start' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;service;stop' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;service;restart' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;service;help' {
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Install the per-user launchd service')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Uninstall the per-user launchd service')
            [CompletionResult]::new('start', 'start', [CompletionResultType]::ParameterValue, 'Start (or bootstrap) the service')
            [CompletionResult]::new('stop', 'stop', [CompletionResultType]::ParameterValue, 'Stop (or bootout/kill) the service')
            [CompletionResult]::new('restart', 'restart', [CompletionResultType]::ParameterValue, 'Restart the service (kickstart -k)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;service;help;install' {
            break
        }
        'rift-cli;service;help;uninstall' {
            break
        }
        'rift-cli;service;help;start' {
            break
        }
        'rift-cli;service;help;stop' {
            break
        }
        'rift-cli;service;help;restart' {
            break
        }
        'rift-cli;service;help;help' {
            break
        }
        'rift-cli;verify' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift-cli;help' {
            [CompletionResult]::new('query', 'query', [CompletionResultType]::ParameterValue, 'Query information from rift')
            [CompletionResult]::new('execute', 'execute', [CompletionResultType]::ParameterValue, 'Execute commands in rift')
            [CompletionResult]::new('subscribe', 'subscribe', [CompletionResultType]::ParameterValue, 'Event subscription commands')
            [CompletionResult]::new('service', 'service', [CompletionResultType]::ParameterValue, 'Manage the launchd service for rift')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'verify')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift-cli;help;query' {
            [CompletionResult]::new('workspaces', 'workspaces', [CompletionResultType]::ParameterValue, 'List virtual workspaces (optionally for a specific MacOS space)')
            [CompletionResult]::new('windows', 'windows', [CompletionResultType]::ParameterValue, 'List windows (optionally filtered by space)')
            [CompletionResult]::new('displays', 'displays', [CompletionResultType]::ParameterValue, 'List connected displays')
            [CompletionResult]::new('window', 'window', [CompletionResultType]::ParameterValue, 'Get information about a specific window')
            [CompletionResult]::new('applications', 'applications', [CompletionResultType]::ParameterValue, 'List running applications')
            [CompletionResult]::new('layout', 'layout', [CompletionResultType]::ParameterValue, 'Get layout state for a space')
            [CompletionResult]::new('workspace-layout', 'workspace-layout', [CompletionResultType]::ParameterValue, 'Get workspace layout-engine mode(s)')
            [CompletionResult]::new('metrics', 'metrics', [CompletionResultType]::ParameterValue, 'Get performance metrics')
            break
        }
        'rift-cli;help;query;workspaces' {
            break
        }
        'rift-cli;help;query;windows' {
            break
        }
        'rift-cli;help;query;displays' {
            break
        }
        'rift-cli;help;query;window' {
            break
        }
        'rift-cli;help;query;applications' {
            break
        }
        'rift-cli;help;query;layout' {
            break
        }
        'rift-cli;help;query;workspace-layout' {
            break
        }
        'rift-cli;help;query;metrics' {
            break
        }
        'rift-cli;help;execute' {
            [CompletionResult]::new('window', 'window', [CompletionResultType]::ParameterValue, 'Window management commands')
            [CompletionResult]::new('workspace', 'workspace', [CompletionResultType]::ParameterValue, 'Virtual workspace commands')
            [CompletionResult]::new('layout', 'layout', [CompletionResultType]::ParameterValue, 'Layout commands')
            [CompletionResult]::new('config', 'config', [CompletionResultType]::ParameterValue, 'Configuration management commands')
            [CompletionResult]::new('mission-control', 'mission-control', [CompletionResultType]::ParameterValue, 'Mission control commands')
            [CompletionResult]::new('display', 'display', [CompletionResultType]::ParameterValue, 'Display/mouse commands')
            [CompletionResult]::new('save-and-exit', 'save-and-exit', [CompletionResultType]::ParameterValue, 'Save current state and exit rift')
            [CompletionResult]::new('debug', 'debug', [CompletionResultType]::ParameterValue, 'Print layout tree debugging output in the running rift instance')
            [CompletionResult]::new('serialize', 'serialize', [CompletionResultType]::ParameterValue, 'Serialize and print runtime state')
            [CompletionResult]::new('toggle-space-activated', 'toggle-space-activated', [CompletionResultType]::ParameterValue, 'Toggle whether the current space is managed by rift')
            [CompletionResult]::new('show-timing', 'show-timing', [CompletionResultType]::ParameterValue, 'Show timing metrics')
            break
        }
        'rift-cli;help;execute;window' {
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Focus the next window')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Focus the previous window')
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Move focus in a direction')
            [CompletionResult]::new('toggle-float', 'toggle-float', [CompletionResultType]::ParameterValue, 'Toggle window floating state')
            [CompletionResult]::new('toggle-fullscreen', 'toggle-fullscreen', [CompletionResultType]::ParameterValue, 'Toggle fullscreen mode (fills the whole screen, ignores outer gaps)')
            [CompletionResult]::new('toggle-fullscreen-within-gaps', 'toggle-fullscreen-within-gaps', [CompletionResultType]::ParameterValue, 'Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)')
            [CompletionResult]::new('resize-grow', 'resize-grow', [CompletionResultType]::ParameterValue, 'Grow the current window size (increments by ~5%)')
            [CompletionResult]::new('resize-shrink', 'resize-shrink', [CompletionResultType]::ParameterValue, 'Shrink the current window size (decrements by ~5%)')
            [CompletionResult]::new('resize-by', 'resize-by', [CompletionResultType]::ParameterValue, 'Resize the selected window by a fractional amount. - Pass a signed floating value: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. `0.05` = 5%). Examples: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%')
            [CompletionResult]::new('close', 'close', [CompletionResultType]::ParameterValue, 'Close a window by window server identifier')
            [CompletionResult]::new('add-scratchpad', 'add-scratchpad', [CompletionResultType]::ParameterValue, 'add-scratchpad')
            [CompletionResult]::new('toggle-scratchpad', 'toggle-scratchpad', [CompletionResultType]::ParameterValue, 'toggle-scratchpad')
            break
        }
        'rift-cli;help;execute;window;next' {
            break
        }
        'rift-cli;help;execute;window;prev' {
            break
        }
        'rift-cli;help;execute;window;focus' {
            break
        }
        'rift-cli;help;execute;window;toggle-float' {
            break
        }
        'rift-cli;help;execute;window;toggle-fullscreen' {
            break
        }
        'rift-cli;help;execute;window;toggle-fullscreen-within-gaps' {
            break
        }
        'rift-cli;help;execute;window;resize-grow' {
            break
        }
        'rift-cli;help;execute;window;resize-shrink' {
            break
        }
        'rift-cli;help;execute;window;resize-by' {
            break
        }
        'rift-cli;help;execute;window;close' {
            break
        }
        'rift-cli;help;execute;window;add-scratchpad' {
            break
        }
        'rift-cli;help;execute;window;toggle-scratchpad' {
            break
        }
        'rift-cli;help;execute;workspace' {
            [CompletionResult]::new('next', 'next', [CompletionResultType]::ParameterValue, 'Switch to next workspace')
            [CompletionResult]::new('prev', 'prev', [CompletionResultType]::ParameterValue, 'Switch to previous workspace')
            [CompletionResult]::new('switch', 'switch', [CompletionResultType]::ParameterValue, 'Switch to specific workspace')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move current window to workspace')
            [CompletionResult]::new('create', 'create', [CompletionResultType]::ParameterValue, 'Create a new workspace')
            [CompletionResult]::new('last', 'last', [CompletionResultType]::ParameterValue, 'Switch to the last workspace')
            [CompletionResult]::new('set-layout', 'set-layout', [CompletionResultType]::ParameterValue, 'Set layout mode for a workspace (or active workspace when omitted)')
            break
        }
        'rift-cli;help;execute;workspace;next' {
            break
        }
        'rift-cli;help;execute;workspace;prev' {
            break
        }
        'rift-cli;help;execute;workspace;switch' {
            break
        }
        'rift-cli;help;execute;workspace;move-window' {
            break
        }
        'rift-cli;help;execute;workspace;create' {
            break
        }
        'rift-cli;help;execute;workspace;last' {
            break
        }
        'rift-cli;help;execute;workspace;set-layout' {
            break
        }
        'rift-cli;help;execute;layout' {
            [CompletionResult]::new('ascend', 'ascend', [CompletionResultType]::ParameterValue, 'Move selection up the tree')
            [CompletionResult]::new('descend', 'descend', [CompletionResultType]::ParameterValue, 'Move selection down the tree')
            [CompletionResult]::new('move-node', 'move-node', [CompletionResultType]::ParameterValue, 'Move the selected node in a direction')
            [CompletionResult]::new('join-window', 'join-window', [CompletionResultType]::ParameterValue, 'Join the selected window with neighbor in a direction')
            [CompletionResult]::new('toggle-stack', 'toggle-stack', [CompletionResultType]::ParameterValue, 'Toggle stacked state for the selected container')
            [CompletionResult]::new('toggle-orientation', 'toggle-orientation', [CompletionResultType]::ParameterValue, 'Global orientation toggle that works consistently across layout modes (and between splits/stacks)')
            [CompletionResult]::new('unjoin', 'unjoin', [CompletionResultType]::ParameterValue, 'Unjoin previously joined windows')
            [CompletionResult]::new('toggle-focus-float', 'toggle-focus-float', [CompletionResultType]::ParameterValue, 'Toggle floating on the focused selection (tree focus)')
            [CompletionResult]::new('adjust-master-ratio', 'adjust-master-ratio', [CompletionResultType]::ParameterValue, 'Adjust master ratio by a delta (master/stack layout only)')
            [CompletionResult]::new('adjust-master-count', 'adjust-master-count', [CompletionResultType]::ParameterValue, 'Adjust master count by a delta (master/stack layout only)')
            [CompletionResult]::new('promote-to-master', 'promote-to-master', [CompletionResultType]::ParameterValue, 'Promote the selected window into the master area (master/stack layout only)')
            [CompletionResult]::new('swap-master-stack', 'swap-master-stack', [CompletionResultType]::ParameterValue, 'Swap the first master with the first stack window (master/stack layout only)')
            [CompletionResult]::new('swap-windows', 'swap-windows', [CompletionResultType]::ParameterValue, 'Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)')
            [CompletionResult]::new('scroll-strip', 'scroll-strip', [CompletionResultType]::ParameterValue, 'Scroll the strip by a normalized delta (scrolling layout only)')
            [CompletionResult]::new('snap-strip', 'snap-strip', [CompletionResultType]::ParameterValue, 'Snap the strip to the nearest column boundary (scrolling layout only)')
            [CompletionResult]::new('center-selection', 'center-selection', [CompletionResultType]::ParameterValue, 'Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed')
            break
        }
        'rift-cli;help;execute;layout;ascend' {
            break
        }
        'rift-cli;help;execute;layout;descend' {
            break
        }
        'rift-cli;help;execute;layout;move-node' {
            break
        }
        'rift-cli;help;execute;layout;join-window' {
            break
        }
        'rift-cli;help;execute;layout;toggle-stack' {
            break
        }
        'rift-cli;help;execute;layout;toggle-orientation' {
            break
        }
        'rift-cli;help;execute;layout;unjoin' {
            break
        }
        'rift-cli;help;execute;layout;toggle-focus-float' {
            break
        }
        'rift-cli;help;execute;layout;adjust-master-ratio' {
            break
        }
        'rift-cli;help;execute;layout;adjust-master-count' {
            break
        }
        'rift-cli;help;execute;layout;promote-to-master' {
            break
        }
        'rift-cli;help;execute;layout;swap-master-stack' {
            break
        }
        'rift-cli;help;execute;layout;swap-windows' {
            break
        }
        'rift-cli;help;execute;layout;scroll-strip' {
            break
        }
        'rift-cli;help;execute;layout;snap-strip' {
            break
        }
        'rift-cli;help;execute;layout;center-selection' {
            break
        }
        'rift-cli;help;execute;config' {
            [CompletionResult]::new('set-animate', 'set-animate', [CompletionResultType]::ParameterValue, 'Update animation settings')
            [CompletionResult]::new('set-animation-duration', 'set-animation-duration', [CompletionResultType]::ParameterValue, 'set-animation-duration')
            [CompletionResult]::new('set-animation-fps', 'set-animation-fps', [CompletionResultType]::ParameterValue, 'set-animation-fps')
            [CompletionResult]::new('set-animation-easing', 'set-animation-easing', [CompletionResultType]::ParameterValue, 'set-animation-easing')
            [CompletionResult]::new('set-mouse-follows-focus', 'set-mouse-follows-focus', [CompletionResultType]::ParameterValue, 'Update mouse settings')
            [CompletionResult]::new('set-mouse-hides-on-focus', 'set-mouse-hides-on-focus', [CompletionResultType]::ParameterValue, 'set-mouse-hides-on-focus')
            [CompletionResult]::new('set-focus-follows-mouse', 'set-focus-follows-mouse', [CompletionResultType]::ParameterValue, 'set-focus-follows-mouse')
            [CompletionResult]::new('set-stack-offset', 'set-stack-offset', [CompletionResultType]::ParameterValue, 'Update layout settings')
            [CompletionResult]::new('set-stack-default-orientation', 'set-stack-default-orientation', [CompletionResultType]::ParameterValue, 'Set the default stack orientation behavior. Value should be one of: "perpendicular", "same", "horizontal", or "vertical"')
            [CompletionResult]::new('set-outer-gaps', 'set-outer-gaps', [CompletionResultType]::ParameterValue, 'set-outer-gaps')
            [CompletionResult]::new('set-inner-gaps', 'set-inner-gaps', [CompletionResultType]::ParameterValue, 'set-inner-gaps')
            [CompletionResult]::new('set-workspace-names', 'set-workspace-names', [CompletionResultType]::ParameterValue, 'Update workspace settings')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Generic set: set an arbitrary config key (dot-separated path) to a JSON value. Example: rift-cli execute config set --key settings.animate --value true')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Get current config')
            [CompletionResult]::new('save', 'save', [CompletionResultType]::ParameterValue, 'Save current config to file')
            [CompletionResult]::new('reload', 'reload', [CompletionResultType]::ParameterValue, 'Reload config from file')
            break
        }
        'rift-cli;help;execute;config;set-animate' {
            break
        }
        'rift-cli;help;execute;config;set-animation-duration' {
            break
        }
        'rift-cli;help;execute;config;set-animation-fps' {
            break
        }
        'rift-cli;help;execute;config;set-animation-easing' {
            break
        }
        'rift-cli;help;execute;config;set-mouse-follows-focus' {
            break
        }
        'rift-cli;help;execute;config;set-mouse-hides-on-focus' {
            break
        }
        'rift-cli;help;execute;config;set-focus-follows-mouse' {
            break
        }
        'rift-cli;help;execute;config;set-stack-offset' {
            break
        }
        'rift-cli;help;execute;config;set-stack-default-orientation' {
            break
        }
        'rift-cli;help;execute;config;set-outer-gaps' {
            break
        }
        'rift-cli;help;execute;config;set-inner-gaps' {
            break
        }
        'rift-cli;help;execute;config;set-workspace-names' {
            break
        }
        'rift-cli;help;execute;config;set' {
            break
        }
        'rift-cli;help;execute;config;get' {
            break
        }
        'rift-cli;help;execute;config;save' {
            break
        }
        'rift-cli;help;execute;config;reload' {
            break
        }
        'rift-cli;help;execute;mission-control' {
            [CompletionResult]::new('show-all', 'show-all', [CompletionResultType]::ParameterValue, 'Show all workspaces in mission control')
            [CompletionResult]::new('show-current', 'show-current', [CompletionResultType]::ParameterValue, 'Show current workspace in mission control')
            [CompletionResult]::new('dismiss', 'dismiss', [CompletionResultType]::ParameterValue, 'Dismiss mission control')
            break
        }
        'rift-cli;help;execute;mission-control;show-all' {
            break
        }
        'rift-cli;help;execute;mission-control;show-current' {
            break
        }
        'rift-cli;help;execute;mission-control;dismiss' {
            break
        }
        'rift-cli;help;execute;display' {
            [CompletionResult]::new('focus', 'focus', [CompletionResultType]::ParameterValue, 'Focus a display by direction, index, or UUID')
            [CompletionResult]::new('move-mouse-to-index', 'move-mouse-to-index', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by index (0-based)')
            [CompletionResult]::new('move-mouse-to-uuid', 'move-mouse-to-uuid', [CompletionResultType]::ParameterValue, 'Move mouse cursor to a display by UUID')
            [CompletionResult]::new('move-window', 'move-window', [CompletionResultType]::ParameterValue, 'Move a window to a display by direction, index, or UUID')
            break
        }
        'rift-cli;help;execute;display;focus' {
            break
        }
        'rift-cli;help;execute;display;move-mouse-to-index' {
            break
        }
        'rift-cli;help;execute;display;move-mouse-to-uuid' {
            break
        }
        'rift-cli;help;execute;display;move-window' {
            break
        }
        'rift-cli;help;execute;save-and-exit' {
            break
        }
        'rift-cli;help;execute;debug' {
            break
        }
        'rift-cli;help;execute;serialize' {
            break
        }
        'rift-cli;help;execute;toggle-space-activated' {
            break
        }
        'rift-cli;help;execute;show-timing' {
            break
        }
        'rift-cli;help;subscribe' {
            [CompletionResult]::new('mach', 'mach', [CompletionResultType]::ParameterValue, 'Subscribe to Mach IPC events')
            [CompletionResult]::new('cli', 'cli', [CompletionResultType]::ParameterValue, 'Subscribe to events via CLI command execution')
            [CompletionResult]::new('unsub-mach', 'unsub-mach', [CompletionResultType]::ParameterValue, 'Unsubscribe from Mach IPC events')
            [CompletionResult]::new('unsub-cli', 'unsub-cli', [CompletionResultType]::ParameterValue, 'Unsubscribe from CLI events')
            [CompletionResult]::new('list-cli', 'list-cli', [CompletionResultType]::ParameterValue, 'List current CLI subscriptions')
            break
        }
        'rift-cli;help;subscribe;mach' {
            break
        }
        'rift-cli;help;subscribe;cli' {
            break
        }
        'rift-cli;help;subscribe;unsub-mach' {
            break
        }
        'rift-cli;help;subscribe;unsub-cli' {
            break
        }
        'rift-cli;help;subscribe;list-cli' {
            break
        }
        'rift-cli;help;service' {
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Install the per-user launchd service')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Uninstall the per-user launchd service')
            [CompletionResult]::new('start', 'start', [CompletionResultType]::ParameterValue, 'Start (or bootstrap) the service')
            [CompletionResult]::new('stop', 'stop', [CompletionResultType]::ParameterValue, 'Stop (or bootout/kill) the service')
            [CompletionResult]::new('restart', 'restart', [CompletionResultType]::ParameterValue, 'Restart the service (kickstart -k)')
            break
        }
        'rift-cli;help;service;install' {
            break
        }
        'rift-cli;help;service;uninstall' {
            break
        }
        'rift-cli;help;service;start' {
            break
        }
        'rift-cli;help;service;stop' {
            break
        }
        'rift-cli;help;service;restart' {
            break
        }
        'rift-cli;help;verify' {
            break
        }
        'rift-cli;help;help' {
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
