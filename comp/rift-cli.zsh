#compdef rift-cli

autoload -U is-at-least

_rift-cli() {
    typeset -A opt_args
    typeset -a _arguments_options
    local ret=1

    if is-at-least 5.2; then
        _arguments_options=(-s -S -C)
    else
        _arguments_options=(-s -C)
    fi

    local context curcontext="$curcontext" state line
    _arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli_commands" \
"*::: :->rift-cli" \
&& ret=0
    case $state in
    (rift-cli)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-command-$line[1]:"
        case $line[1] in
            (query)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__query_commands" \
"*::: :->query" \
&& ret=0

    case $state in
    (query)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-query-command-$line[1]:"
        case $line[1] in
            (workspaces)
_arguments "${_arguments_options[@]}" : \
'--space-id=[]:SPACE_ID:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(windows)
_arguments "${_arguments_options[@]}" : \
'--space-id=[]:SPACE_ID:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(displays)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(window)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':window_id:_default' \
&& ret=0
;;
(applications)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(layout)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':space_id:_default' \
&& ret=0
;;
(workspace-layout)
_arguments "${_arguments_options[@]}" : \
'--space-id=[]:SPACE_ID:_default' \
'--workspace-id=[]:WORKSPACE_ID:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(metrics)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__query__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-query-help-command-$line[1]:"
        case $line[1] in
            (workspaces)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(windows)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(displays)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(applications)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(workspace-layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(metrics)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(execute)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute_commands" \
"*::: :->execute" \
&& ret=0

    case $state in
    (execute)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-command-$line[1]:"
        case $line[1] in
            (window)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute__subcmd__window_commands" \
"*::: :->window" \
&& ret=0

    case $state in
    (window)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-window-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(focus)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':direction:_default' \
&& ret=0
;;
(toggle-float)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(toggle-fullscreen)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(toggle-fullscreen-within-gaps)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(resize-grow)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(resize-shrink)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(resize-by)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':amount:_default' \
&& ret=0
;;
(close)
_arguments "${_arguments_options[@]}" : \
'--window-id=[Window Id (window server id or idx from window id)]:WINDOW_ID:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(add-scratchpad)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(toggle-scratchpad)
_arguments "${_arguments_options[@]}" : \
'--name=[Name of the scratchpad (optional, defaults to "default")]:NAME:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__window__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-window-help-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-float)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-fullscreen)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-fullscreen-within-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-grow)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-shrink)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-by)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(close)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(add-scratchpad)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-scratchpad)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(workspace)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute__subcmd__workspace_commands" \
"*::: :->workspace" \
&& ret=0

    case $state in
    (workspace)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-workspace-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
'::skip_empty:(true false)' \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
'::skip_empty:(true false)' \
&& ret=0
;;
(switch)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':workspace_id:_default' \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':workspace_id:_default' \
'::window_id:_default' \
&& ret=0
;;
(create)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(last)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(set-layout)
_arguments "${_arguments_options[@]}" : \
'--workspace-id=[Workspace index (0-based). Defaults to active workspace if omitted]:WORKSPACE_ID:_default' \
'-h[Print help]' \
'--help[Print help]' \
':mode -- Layout mode\: traditional, bsp, stack, master_stack, scrolling:_default' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-workspace-help-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(switch)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(create)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(last)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(layout)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute__subcmd__layout_commands" \
"*::: :->layout" \
&& ret=0

    case $state in
    (layout)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-layout-command-$line[1]:"
        case $line[1] in
            (ascend)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(descend)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(move-node)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':direction:_default' \
&& ret=0
;;
(join-window)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':direction:_default' \
&& ret=0
;;
(toggle-stack)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(toggle-orientation)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(unjoin)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(toggle-focus-float)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(adjust-master-ratio)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':delta:_default' \
&& ret=0
;;
(adjust-master-count)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':delta:_default' \
&& ret=0
;;
(promote-to-master)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(swap-master-stack)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(swap-windows)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':a:_default' \
':b:_default' \
&& ret=0
;;
(scroll-strip)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':delta:_default' \
&& ret=0
;;
(snap-strip)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(center-selection)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__layout__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-layout-help-command-$line[1]:"
        case $line[1] in
            (ascend)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(descend)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-node)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(join-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-stack)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-orientation)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unjoin)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-focus-float)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(adjust-master-ratio)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(adjust-master-count)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(promote-to-master)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(swap-master-stack)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(swap-windows)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(scroll-strip)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(snap-strip)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(center-selection)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(config)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute__subcmd__config_commands" \
"*::: :->config" \
&& ret=0

    case $state in
    (config)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-config-command-$line[1]:"
        case $line[1] in
            (set-animate)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:_default' \
&& ret=0
;;
(set-animation-duration)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:_default' \
&& ret=0
;;
(set-animation-fps)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:_default' \
&& ret=0
;;
(set-animation-easing)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:_default' \
&& ret=0
;;
(set-mouse-follows-focus)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:(true false)' \
&& ret=0
;;
(set-mouse-hides-on-focus)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:(true false)' \
&& ret=0
;;
(set-focus-follows-mouse)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:(true false)' \
&& ret=0
;;
(set-stack-offset)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:_default' \
&& ret=0
;;
(set-stack-default-orientation)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':value:_default' \
&& ret=0
;;
(set-outer-gaps)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':top:_default' \
':left:_default' \
':bottom:_default' \
':right:_default' \
&& ret=0
;;
(set-inner-gaps)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':horizontal:_default' \
':vertical:_default' \
&& ret=0
;;
(set-workspace-names)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
'*::names:_default' \
&& ret=0
;;
(set)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':key -- Dot-separated key path (e.g. settings.animate or settings.layout.gaps.outer.top):_default' \
':value -- Value should be valid JSON (true, 1, "string", {"a"\:1}), but if it'\''s not valid JSON it will be treated as a string:_default' \
&& ret=0
;;
(get)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(save)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(reload)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__config__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-config-help-command-$line[1]:"
        case $line[1] in
            (set-animate)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-duration)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-fps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-easing)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-mouse-follows-focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-mouse-hides-on-focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-focus-follows-mouse)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-stack-offset)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-stack-default-orientation)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-outer-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-inner-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-workspace-names)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(get)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(save)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(reload)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(mission-control)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute__subcmd__mission-control_commands" \
"*::: :->mission-control" \
&& ret=0

    case $state in
    (mission-control)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-mission-control-command-$line[1]:"
        case $line[1] in
            (show-all)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(show-current)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(dismiss)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-mission-control-help-command-$line[1]:"
        case $line[1] in
            (show-all)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(show-current)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(dismiss)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(display)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__execute__subcmd__display_commands" \
"*::: :->display" \
&& ret=0

    case $state in
    (display)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-display-command-$line[1]:"
        case $line[1] in
            (focus)
_arguments "${_arguments_options[@]}" : \
'--direction=[Direction relative to the current display (left, right, up, down)]:DIRECTION:_default' \
'--index=[Display index (0-based)]:INDEX:_default' \
'--uuid=[Display UUID]:UUID:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(move-mouse-to-index)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':index -- Display index (0-based):_default' \
&& ret=0
;;
(move-mouse-to-uuid)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':uuid -- Display UUID:_default' \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
'--direction=[Direction relative to the window'\''s current display (left, right, up, down)]:DIRECTION:_default' \
'--index=[Display index (0-based)]:INDEX:_default' \
'--uuid=[Display UUID]:UUID:_default' \
'--window-id=[Optional window id (window idx); defaults to the focused window if omitted]:WINDOW_ID:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__display__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-display-help-command-$line[1]:"
        case $line[1] in
            (focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-mouse-to-index)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-mouse-to-uuid)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(save-and-exit)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(debug)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(serialize)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(toggle-space-activated)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(show-timing)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-command-$line[1]:"
        case $line[1] in
            (window)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help__subcmd__window_commands" \
"*::: :->window" \
&& ret=0

    case $state in
    (window)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-window-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-float)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-fullscreen)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-fullscreen-within-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-grow)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-shrink)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-by)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(close)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(add-scratchpad)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-scratchpad)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(workspace)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace_commands" \
"*::: :->workspace" \
&& ret=0

    case $state in
    (workspace)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-workspace-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(switch)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(create)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(last)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(layout)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help__subcmd__layout_commands" \
"*::: :->layout" \
&& ret=0

    case $state in
    (layout)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-layout-command-$line[1]:"
        case $line[1] in
            (ascend)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(descend)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-node)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(join-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-stack)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-orientation)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unjoin)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-focus-float)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(adjust-master-ratio)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(adjust-master-count)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(promote-to-master)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(swap-master-stack)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(swap-windows)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(scroll-strip)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(snap-strip)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(center-selection)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(config)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help__subcmd__config_commands" \
"*::: :->config" \
&& ret=0

    case $state in
    (config)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-config-command-$line[1]:"
        case $line[1] in
            (set-animate)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-duration)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-fps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-easing)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-mouse-follows-focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-mouse-hides-on-focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-focus-follows-mouse)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-stack-offset)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-stack-default-orientation)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-outer-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-inner-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-workspace-names)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(get)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(save)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(reload)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(mission-control)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control_commands" \
"*::: :->mission-control" \
&& ret=0

    case $state in
    (mission-control)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-mission-control-command-$line[1]:"
        case $line[1] in
            (show-all)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(show-current)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(dismiss)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(display)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__execute__subcmd__help__subcmd__display_commands" \
"*::: :->display" \
&& ret=0

    case $state in
    (display)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-execute-help-display-command-$line[1]:"
        case $line[1] in
            (focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-mouse-to-index)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-mouse-to-uuid)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(save-and-exit)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(debug)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(serialize)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-space-activated)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(show-timing)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(subscribe)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__subscribe_commands" \
"*::: :->subscribe" \
&& ret=0

    case $state in
    (subscribe)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-subscribe-command-$line[1]:"
        case $line[1] in
            (mach)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':event -- Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *):_default' \
&& ret=0
;;
(cli)
_arguments "${_arguments_options[@]}" : \
'--event=[Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)]:EVENT:_default' \
'--command=[Command to execute when event occurs]:COMMAND:_default' \
'*--args=[Arguments to pass to command (event data will be appended as JSON)]:ARGS:_default' \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(unsub-mach)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':event -- Event to unsubscribe from:_default' \
&& ret=0
;;
(unsub-cli)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':event -- Event to unsubscribe from:_default' \
&& ret=0
;;
(list-cli)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__subscribe__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-subscribe-help-command-$line[1]:"
        case $line[1] in
            (mach)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(cli)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unsub-mach)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unsub-cli)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(list-cli)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(service)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift-cli__subcmd__service_commands" \
"*::: :->service" \
&& ret=0

    case $state in
    (service)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-service-command-$line[1]:"
        case $line[1] in
            (install)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(uninstall)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(start)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(stop)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(restart)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__service__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-service-help-command-$line[1]:"
        case $line[1] in
            (install)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(uninstall)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(start)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(stop)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(restart)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
;;
(verify)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':config_path:_files' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-command-$line[1]:"
        case $line[1] in
            (query)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__query_commands" \
"*::: :->query" \
&& ret=0

    case $state in
    (query)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-query-command-$line[1]:"
        case $line[1] in
            (workspaces)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(windows)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(displays)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(applications)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(workspace-layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(metrics)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(execute)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute_commands" \
"*::: :->execute" \
&& ret=0

    case $state in
    (execute)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-command-$line[1]:"
        case $line[1] in
            (window)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute__subcmd__window_commands" \
"*::: :->window" \
&& ret=0

    case $state in
    (window)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-window-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-float)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-fullscreen)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-fullscreen-within-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-grow)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-shrink)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(resize-by)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(close)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(add-scratchpad)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-scratchpad)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(workspace)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace_commands" \
"*::: :->workspace" \
&& ret=0

    case $state in
    (workspace)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-workspace-command-$line[1]:"
        case $line[1] in
            (next)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(prev)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(switch)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(create)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(last)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-layout)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(layout)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute__subcmd__layout_commands" \
"*::: :->layout" \
&& ret=0

    case $state in
    (layout)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-layout-command-$line[1]:"
        case $line[1] in
            (ascend)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(descend)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-node)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(join-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-stack)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-orientation)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unjoin)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-focus-float)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(adjust-master-ratio)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(adjust-master-count)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(promote-to-master)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(swap-master-stack)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(swap-windows)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(scroll-strip)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(snap-strip)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(center-selection)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(config)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute__subcmd__config_commands" \
"*::: :->config" \
&& ret=0

    case $state in
    (config)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-config-command-$line[1]:"
        case $line[1] in
            (set-animate)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-duration)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-fps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-animation-easing)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-mouse-follows-focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-mouse-hides-on-focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-focus-follows-mouse)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-stack-offset)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-stack-default-orientation)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-outer-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-inner-gaps)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set-workspace-names)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(set)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(get)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(save)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(reload)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(mission-control)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control_commands" \
"*::: :->mission-control" \
&& ret=0

    case $state in
    (mission-control)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-mission-control-command-$line[1]:"
        case $line[1] in
            (show-all)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(show-current)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(dismiss)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(display)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__execute__subcmd__display_commands" \
"*::: :->display" \
&& ret=0

    case $state in
    (display)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-execute-display-command-$line[1]:"
        case $line[1] in
            (focus)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-mouse-to-index)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-mouse-to-uuid)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(move-window)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(save-and-exit)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(debug)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(serialize)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(toggle-space-activated)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(show-timing)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(subscribe)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__subscribe_commands" \
"*::: :->subscribe" \
&& ret=0

    case $state in
    (subscribe)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-subscribe-command-$line[1]:"
        case $line[1] in
            (mach)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(cli)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unsub-mach)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(unsub-cli)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(list-cli)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(service)
_arguments "${_arguments_options[@]}" : \
":: :_rift-cli__subcmd__help__subcmd__service_commands" \
"*::: :->service" \
&& ret=0

    case $state in
    (service)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-cli-help-service-command-$line[1]:"
        case $line[1] in
            (install)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(uninstall)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(start)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(stop)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(restart)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
(verify)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
}

(( $+functions[_rift-cli_commands] )) ||
_rift-cli_commands() {
    local commands; commands=(
'query:Query information from rift' \
'execute:Execute commands in rift' \
'subscribe:Event subscription commands' \
'service:Manage the launchd service for rift' \
'verify:' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute_commands] )) ||
_rift-cli__subcmd__execute_commands() {
    local commands; commands=(
'window:Window management commands' \
'workspace:Virtual workspace commands' \
'layout:Layout commands' \
'config:Configuration management commands' \
'mission-control:Mission control commands' \
'display:Display/mouse commands' \
'save-and-exit:Save current state and exit rift' \
'debug:Print layout tree debugging output in the running rift instance' \
'serialize:Serialize and print runtime state' \
'toggle-space-activated:Toggle whether the current space is managed by rift' \
'show-timing:Show timing metrics' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config_commands() {
    local commands; commands=(
'set-animate:Update animation settings' \
'set-animation-duration:' \
'set-animation-fps:' \
'set-animation-easing:' \
'set-mouse-follows-focus:Update mouse settings' \
'set-mouse-hides-on-focus:' \
'set-focus-follows-mouse:' \
'set-stack-offset:Update layout settings' \
'set-stack-default-orientation:Set the default stack orientation behavior. Value should be one of\: "perpendicular", "same", "horizontal", or "vertical"' \
'set-outer-gaps:' \
'set-inner-gaps:' \
'set-workspace-names:Update workspace settings' \
'set:Generic set\: set an arbitrary config key (dot-separated path) to a JSON value. Example\: rift-cli execute config set --key settings.animate --value true' \
'get:Get current config' \
'save:Save current config to file' \
'reload:Reload config from file' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute config commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__get_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__get_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config get commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help_commands() {
    local commands; commands=(
'set-animate:Update animation settings' \
'set-animation-duration:' \
'set-animation-fps:' \
'set-animation-easing:' \
'set-mouse-follows-focus:Update mouse settings' \
'set-mouse-hides-on-focus:' \
'set-focus-follows-mouse:' \
'set-stack-offset:Update layout settings' \
'set-stack-default-orientation:Set the default stack orientation behavior. Value should be one of\: "perpendicular", "same", "horizontal", or "vertical"' \
'set-outer-gaps:' \
'set-inner-gaps:' \
'set-workspace-names:Update workspace settings' \
'set:Generic set\: set an arbitrary config key (dot-separated path) to a JSON value. Example\: rift-cli execute config set --key settings.animate --value true' \
'get:Get current config' \
'save:Save current config to file' \
'reload:Reload config from file' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute config help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__get_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__get_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help get commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__reload_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__reload_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help reload commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__save_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__save_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help save commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animate_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animate_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-animate commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animation-duration_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animation-duration_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-animation-duration commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animation-easing_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animation-easing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-animation-easing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animation-fps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-animation-fps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-animation-fps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-focus-follows-mouse_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-focus-follows-mouse_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-focus-follows-mouse commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-inner-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-inner-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-inner-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-mouse-follows-focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-mouse-follows-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-mouse-follows-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-mouse-hides-on-focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-mouse-hides-on-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-mouse-hides-on-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-outer-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-outer-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-outer-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-stack-default-orientation_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-stack-default-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-stack-default-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-stack-offset_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-stack-offset_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-stack-offset commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-workspace-names_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set-workspace-names_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config help set-workspace-names commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__reload_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__reload_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config reload commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__save_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__save_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config save commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animate_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animate_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-animate commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animation-duration_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animation-duration_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-animation-duration commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animation-easing_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animation-easing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-animation-easing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animation-fps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-animation-fps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-animation-fps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-focus-follows-mouse_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-focus-follows-mouse_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-focus-follows-mouse commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-inner-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-inner-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-inner-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-mouse-follows-focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-mouse-follows-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-mouse-follows-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-mouse-hides-on-focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-mouse-hides-on-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-mouse-hides-on-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-outer-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-outer-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-outer-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-stack-default-orientation_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-stack-default-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-stack-default-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-stack-offset_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-stack-offset_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-stack-offset commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__config__subcmd__set-workspace-names_commands] )) ||
_rift-cli__subcmd__execute__subcmd__config__subcmd__set-workspace-names_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute config set-workspace-names commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__debug_commands] )) ||
_rift-cli__subcmd__execute__subcmd__debug_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute debug commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display_commands() {
    local commands; commands=(
'focus:Focus a display by direction, index, or UUID' \
'move-mouse-to-index:Move mouse cursor to a display by index (0-based)' \
'move-mouse-to-uuid:Move mouse cursor to a display by UUID' \
'move-window:Move a window to a display by direction, index, or UUID' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute display commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__help_commands() {
    local commands; commands=(
'focus:Focus a display by direction, index, or UUID' \
'move-mouse-to-index:Move mouse cursor to a display by index (0-based)' \
'move-mouse-to-uuid:Move mouse cursor to a display by UUID' \
'move-window:Move a window to a display by direction, index, or UUID' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute display help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display help focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move-mouse-to-index_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move-mouse-to-index_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display help move-mouse-to-index commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move-mouse-to-uuid_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move-mouse-to-uuid_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display help move-mouse-to-uuid commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display help move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__move-mouse-to-index_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__move-mouse-to-index_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display move-mouse-to-index commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__move-mouse-to-uuid_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__move-mouse-to-uuid_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display move-mouse-to-uuid commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__display__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__display__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute display move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help_commands() {
    local commands; commands=(
'window:Window management commands' \
'workspace:Virtual workspace commands' \
'layout:Layout commands' \
'config:Configuration management commands' \
'mission-control:Mission control commands' \
'display:Display/mouse commands' \
'save-and-exit:Save current state and exit rift' \
'debug:Print layout tree debugging output in the running rift instance' \
'serialize:Serialize and print runtime state' \
'toggle-space-activated:Toggle whether the current space is managed by rift' \
'show-timing:Show timing metrics' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config_commands() {
    local commands; commands=(
'set-animate:Update animation settings' \
'set-animation-duration:' \
'set-animation-fps:' \
'set-animation-easing:' \
'set-mouse-follows-focus:Update mouse settings' \
'set-mouse-hides-on-focus:' \
'set-focus-follows-mouse:' \
'set-stack-offset:Update layout settings' \
'set-stack-default-orientation:Set the default stack orientation behavior. Value should be one of\: "perpendicular", "same", "horizontal", or "vertical"' \
'set-outer-gaps:' \
'set-inner-gaps:' \
'set-workspace-names:Update workspace settings' \
'set:Generic set\: set an arbitrary config key (dot-separated path) to a JSON value. Example\: rift-cli execute config set --key settings.animate --value true' \
'get:Get current config' \
'save:Save current config to file' \
'reload:Reload config from file' \
    )
    _describe -t commands 'rift-cli execute help config commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__get_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__get_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config get commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__reload_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__reload_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config reload commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__save_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__save_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config save commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animate_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animate_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-animate commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animation-duration_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animation-duration_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-animation-duration commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animation-easing_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animation-easing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-animation-easing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animation-fps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-animation-fps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-animation-fps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-focus-follows-mouse_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-focus-follows-mouse_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-focus-follows-mouse commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-inner-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-inner-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-inner-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-mouse-follows-focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-mouse-follows-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-mouse-follows-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-mouse-hides-on-focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-mouse-hides-on-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-mouse-hides-on-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-outer-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-outer-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-outer-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-stack-default-orientation_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-stack-default-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-stack-default-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-stack-offset_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-stack-offset_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-stack-offset commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-workspace-names_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set-workspace-names_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help config set-workspace-names commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__debug_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__debug_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help debug commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__display_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__display_commands() {
    local commands; commands=(
'focus:Focus a display by direction, index, or UUID' \
'move-mouse-to-index:Move mouse cursor to a display by index (0-based)' \
'move-mouse-to-uuid:Move mouse cursor to a display by UUID' \
'move-window:Move a window to a display by direction, index, or UUID' \
    )
    _describe -t commands 'rift-cli execute help display commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help display focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move-mouse-to-index_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move-mouse-to-index_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help display move-mouse-to-index commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move-mouse-to-uuid_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move-mouse-to-uuid_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help display move-mouse-to-uuid commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help display move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout_commands() {
    local commands; commands=(
'ascend:Move selection up the tree' \
'descend:Move selection down the tree' \
'move-node:Move the selected node in a direction' \
'join-window:Join the selected window with neighbor in a direction' \
'toggle-stack:Toggle stacked state for the selected container' \
'toggle-orientation:Global orientation toggle that works consistently across layout modes (and between splits/stacks)' \
'unjoin:Unjoin previously joined windows' \
'toggle-focus-float:Toggle floating on the focused selection (tree focus)' \
'adjust-master-ratio:Adjust master ratio by a delta (master/stack layout only)' \
'adjust-master-count:Adjust master count by a delta (master/stack layout only)' \
'promote-to-master:Promote the selected window into the master area (master/stack layout only)' \
'swap-master-stack:Swap the first master with the first stack window (master/stack layout only)' \
'swap-windows:Swap two windows by window id (\`WindowId { pid\: ..., idx\: ... }\`)' \
'scroll-strip:Scroll the strip by a normalized delta (scrolling layout only)' \
'snap-strip:Snap the strip to the nearest column boundary (scrolling layout only)' \
'center-selection:Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed' \
    )
    _describe -t commands 'rift-cli execute help layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust-master-count_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust-master-count_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout adjust-master-count commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust-master-ratio_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust-master-ratio_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout adjust-master-ratio commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__ascend_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__ascend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout ascend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__center-selection_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__center-selection_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout center-selection commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__descend_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__descend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout descend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__join-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__join-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout join-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__move-node_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__move-node_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout move-node commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__promote-to-master_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__promote-to-master_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout promote-to-master commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__scroll-strip_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__scroll-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout scroll-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__snap-strip_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__snap-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout snap-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap-master-stack_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap-master-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout swap-master-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap-windows_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap-windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout swap-windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle-focus-float_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle-focus-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout toggle-focus-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle-orientation_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout toggle-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle-stack_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout toggle-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__unjoin_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__unjoin_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help layout unjoin commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control_commands() {
    local commands; commands=(
'show-all:Show all workspaces in mission control' \
'show-current:Show current workspace in mission control' \
'dismiss:Dismiss mission control' \
    )
    _describe -t commands 'rift-cli execute help mission-control commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control__subcmd__dismiss_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control__subcmd__dismiss_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help mission-control dismiss commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control__subcmd__show-all_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control__subcmd__show-all_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help mission-control show-all commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control__subcmd__show-current_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__mission-control__subcmd__show-current_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help mission-control show-current commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__save-and-exit_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__save-and-exit_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help save-and-exit commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__serialize_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__serialize_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help serialize commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__show-timing_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__show-timing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help show-timing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__toggle-space-activated_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__toggle-space-activated_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help toggle-space-activated commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window_commands() {
    local commands; commands=(
'next:Focus the next window' \
'prev:Focus the previous window' \
'focus:Move focus in a direction' \
'toggle-float:Toggle window floating state' \
'toggle-fullscreen:Toggle fullscreen mode (fills the whole screen, ignores outer gaps)' \
'toggle-fullscreen-within-gaps:Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)' \
'resize-grow:Grow the current window size (increments by ~5%)' \
'resize-shrink:Shrink the current window size (decrements by ~5%)' \
'resize-by:Resize the selected window by a fractional amount. - Pass a signed floating value\: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. \`0.05\` = 5%). Examples\: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%' \
'close:Close a window by window server identifier' \
'add-scratchpad:' \
'toggle-scratchpad:' \
    )
    _describe -t commands 'rift-cli execute help window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__add-scratchpad_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__add-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window add-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__close_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__close_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window close commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__next_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__prev_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize-by_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize-by_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window resize-by commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize-grow_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize-grow_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window resize-grow commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize-shrink_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize-shrink_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window resize-shrink commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-float_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window toggle-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-fullscreen_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-fullscreen_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window toggle-fullscreen commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-fullscreen-within-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-fullscreen-within-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window toggle-fullscreen-within-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-scratchpad_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help window toggle-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace_commands() {
    local commands; commands=(
'next:Switch to next workspace' \
'prev:Switch to previous workspace' \
'switch:Switch to specific workspace' \
'move-window:Move current window to workspace' \
'create:Create a new workspace' \
'last:Switch to the last workspace' \
'set-layout:Set layout mode for a workspace (or active workspace when omitted)' \
    )
    _describe -t commands 'rift-cli execute help workspace commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__create_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__create_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace create commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__last_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__last_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace last commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__next_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__prev_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__set-layout_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__set-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace set-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__switch_commands] )) ||
_rift-cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__switch_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute help workspace switch commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout_commands() {
    local commands; commands=(
'ascend:Move selection up the tree' \
'descend:Move selection down the tree' \
'move-node:Move the selected node in a direction' \
'join-window:Join the selected window with neighbor in a direction' \
'toggle-stack:Toggle stacked state for the selected container' \
'toggle-orientation:Global orientation toggle that works consistently across layout modes (and between splits/stacks)' \
'unjoin:Unjoin previously joined windows' \
'toggle-focus-float:Toggle floating on the focused selection (tree focus)' \
'adjust-master-ratio:Adjust master ratio by a delta (master/stack layout only)' \
'adjust-master-count:Adjust master count by a delta (master/stack layout only)' \
'promote-to-master:Promote the selected window into the master area (master/stack layout only)' \
'swap-master-stack:Swap the first master with the first stack window (master/stack layout only)' \
'swap-windows:Swap two windows by window id (\`WindowId { pid\: ..., idx\: ... }\`)' \
'scroll-strip:Scroll the strip by a normalized delta (scrolling layout only)' \
'snap-strip:Snap the strip to the nearest column boundary (scrolling layout only)' \
'center-selection:Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__adjust-master-count_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__adjust-master-count_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout adjust-master-count commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__adjust-master-ratio_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__adjust-master-ratio_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout adjust-master-ratio commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__ascend_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__ascend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout ascend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__center-selection_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__center-selection_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout center-selection commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__descend_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__descend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout descend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help_commands() {
    local commands; commands=(
'ascend:Move selection up the tree' \
'descend:Move selection down the tree' \
'move-node:Move the selected node in a direction' \
'join-window:Join the selected window with neighbor in a direction' \
'toggle-stack:Toggle stacked state for the selected container' \
'toggle-orientation:Global orientation toggle that works consistently across layout modes (and between splits/stacks)' \
'unjoin:Unjoin previously joined windows' \
'toggle-focus-float:Toggle floating on the focused selection (tree focus)' \
'adjust-master-ratio:Adjust master ratio by a delta (master/stack layout only)' \
'adjust-master-count:Adjust master count by a delta (master/stack layout only)' \
'promote-to-master:Promote the selected window into the master area (master/stack layout only)' \
'swap-master-stack:Swap the first master with the first stack window (master/stack layout only)' \
'swap-windows:Swap two windows by window id (\`WindowId { pid\: ..., idx\: ... }\`)' \
'scroll-strip:Scroll the strip by a normalized delta (scrolling layout only)' \
'snap-strip:Snap the strip to the nearest column boundary (scrolling layout only)' \
'center-selection:Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute layout help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust-master-count_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust-master-count_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help adjust-master-count commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust-master-ratio_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust-master-ratio_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help adjust-master-ratio commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__ascend_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__ascend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help ascend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__center-selection_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__center-selection_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help center-selection commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__descend_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__descend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help descend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__join-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__join-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help join-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__move-node_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__move-node_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help move-node commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__promote-to-master_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__promote-to-master_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help promote-to-master commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__scroll-strip_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__scroll-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help scroll-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__snap-strip_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__snap-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help snap-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap-master-stack_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap-master-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help swap-master-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap-windows_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap-windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help swap-windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle-focus-float_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle-focus-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help toggle-focus-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle-orientation_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help toggle-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle-stack_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help toggle-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__unjoin_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__unjoin_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout help unjoin commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__join-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__join-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout join-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__move-node_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__move-node_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout move-node commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__promote-to-master_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__promote-to-master_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout promote-to-master commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__scroll-strip_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__scroll-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout scroll-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__snap-strip_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__snap-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout snap-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__swap-master-stack_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__swap-master-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout swap-master-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__swap-windows_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__swap-windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout swap-windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__toggle-focus-float_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__toggle-focus-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout toggle-focus-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__toggle-orientation_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__toggle-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout toggle-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__toggle-stack_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__toggle-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout toggle-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__layout__subcmd__unjoin_commands] )) ||
_rift-cli__subcmd__execute__subcmd__layout__subcmd__unjoin_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute layout unjoin commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control_commands() {
    local commands; commands=(
'show-all:Show all workspaces in mission control' \
'show-current:Show current workspace in mission control' \
'dismiss:Dismiss mission control' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute mission-control commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__dismiss_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__dismiss_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control dismiss commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help_commands() {
    local commands; commands=(
'show-all:Show all workspaces in mission control' \
'show-current:Show current workspace in mission control' \
'dismiss:Dismiss mission control' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute mission-control help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__dismiss_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__dismiss_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control help dismiss commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__show-all_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__show-all_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control help show-all commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__show-current_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__help__subcmd__show-current_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control help show-current commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__show-all_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__show-all_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control show-all commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__show-current_commands] )) ||
_rift-cli__subcmd__execute__subcmd__mission-control__subcmd__show-current_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute mission-control show-current commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__save-and-exit_commands] )) ||
_rift-cli__subcmd__execute__subcmd__save-and-exit_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute save-and-exit commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__serialize_commands] )) ||
_rift-cli__subcmd__execute__subcmd__serialize_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute serialize commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__show-timing_commands] )) ||
_rift-cli__subcmd__execute__subcmd__show-timing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute show-timing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__toggle-space-activated_commands] )) ||
_rift-cli__subcmd__execute__subcmd__toggle-space-activated_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute toggle-space-activated commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window_commands() {
    local commands; commands=(
'next:Focus the next window' \
'prev:Focus the previous window' \
'focus:Move focus in a direction' \
'toggle-float:Toggle window floating state' \
'toggle-fullscreen:Toggle fullscreen mode (fills the whole screen, ignores outer gaps)' \
'toggle-fullscreen-within-gaps:Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)' \
'resize-grow:Grow the current window size (increments by ~5%)' \
'resize-shrink:Shrink the current window size (decrements by ~5%)' \
'resize-by:Resize the selected window by a fractional amount. - Pass a signed floating value\: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. \`0.05\` = 5%). Examples\: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%' \
'close:Close a window by window server identifier' \
'add-scratchpad:' \
'toggle-scratchpad:' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__add-scratchpad_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__add-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window add-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__close_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__close_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window close commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help_commands() {
    local commands; commands=(
'next:Focus the next window' \
'prev:Focus the previous window' \
'focus:Move focus in a direction' \
'toggle-float:Toggle window floating state' \
'toggle-fullscreen:Toggle fullscreen mode (fills the whole screen, ignores outer gaps)' \
'toggle-fullscreen-within-gaps:Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)' \
'resize-grow:Grow the current window size (increments by ~5%)' \
'resize-shrink:Shrink the current window size (decrements by ~5%)' \
'resize-by:Resize the selected window by a fractional amount. - Pass a signed floating value\: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. \`0.05\` = 5%). Examples\: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%' \
'close:Close a window by window server identifier' \
'add-scratchpad:' \
'toggle-scratchpad:' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute window help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__add-scratchpad_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__add-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help add-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__close_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__close_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help close commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__focus_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__next_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__prev_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize-by_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize-by_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help resize-by commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize-grow_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize-grow_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help resize-grow commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize-shrink_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize-shrink_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help resize-shrink commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-float_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help toggle-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-fullscreen_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-fullscreen_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help toggle-fullscreen commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-fullscreen-within-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-fullscreen-within-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help toggle-fullscreen-within-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-scratchpad_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window help toggle-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__next_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__prev_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__resize-by_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__resize-by_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window resize-by commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__resize-grow_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__resize-grow_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window resize-grow commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__resize-shrink_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__resize-shrink_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window resize-shrink commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-float_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window toggle-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window toggle-fullscreen commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen-within-gaps_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen-within-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window toggle-fullscreen-within-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-scratchpad_commands] )) ||
_rift-cli__subcmd__execute__subcmd__window__subcmd__toggle-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute window toggle-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace_commands() {
    local commands; commands=(
'next:Switch to next workspace' \
'prev:Switch to previous workspace' \
'switch:Switch to specific workspace' \
'move-window:Move current window to workspace' \
'create:Create a new workspace' \
'last:Switch to the last workspace' \
'set-layout:Set layout mode for a workspace (or active workspace when omitted)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute workspace commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__create_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__create_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace create commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help_commands() {
    local commands; commands=(
'next:Switch to next workspace' \
'prev:Switch to previous workspace' \
'switch:Switch to specific workspace' \
'move-window:Move current window to workspace' \
'create:Create a new workspace' \
'last:Switch to the last workspace' \
'set-layout:Set layout mode for a workspace (or active workspace when omitted)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli execute workspace help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__create_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__create_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help create commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__last_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__last_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help last commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__next_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__prev_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__set-layout_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__set-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help set-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__switch_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__switch_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace help switch commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__last_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__last_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace last commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__next_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__prev_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__set-layout_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__set-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace set-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__execute__subcmd__workspace__subcmd__switch_commands] )) ||
_rift-cli__subcmd__execute__subcmd__workspace__subcmd__switch_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli execute workspace switch commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help_commands] )) ||
_rift-cli__subcmd__help_commands() {
    local commands; commands=(
'query:Query information from rift' \
'execute:Execute commands in rift' \
'subscribe:Event subscription commands' \
'service:Manage the launchd service for rift' \
'verify:' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute_commands() {
    local commands; commands=(
'window:Window management commands' \
'workspace:Virtual workspace commands' \
'layout:Layout commands' \
'config:Configuration management commands' \
'mission-control:Mission control commands' \
'display:Display/mouse commands' \
'save-and-exit:Save current state and exit rift' \
'debug:Print layout tree debugging output in the running rift instance' \
'serialize:Serialize and print runtime state' \
'toggle-space-activated:Toggle whether the current space is managed by rift' \
'show-timing:Show timing metrics' \
    )
    _describe -t commands 'rift-cli help execute commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config_commands() {
    local commands; commands=(
'set-animate:Update animation settings' \
'set-animation-duration:' \
'set-animation-fps:' \
'set-animation-easing:' \
'set-mouse-follows-focus:Update mouse settings' \
'set-mouse-hides-on-focus:' \
'set-focus-follows-mouse:' \
'set-stack-offset:Update layout settings' \
'set-stack-default-orientation:Set the default stack orientation behavior. Value should be one of\: "perpendicular", "same", "horizontal", or "vertical"' \
'set-outer-gaps:' \
'set-inner-gaps:' \
'set-workspace-names:Update workspace settings' \
'set:Generic set\: set an arbitrary config key (dot-separated path) to a JSON value. Example\: rift-cli execute config set --key settings.animate --value true' \
'get:Get current config' \
'save:Save current config to file' \
'reload:Reload config from file' \
    )
    _describe -t commands 'rift-cli help execute config commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__get_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__get_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config get commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__reload_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__reload_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config reload commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__save_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__save_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config save commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animate_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animate_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-animate commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animation-duration_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animation-duration_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-animation-duration commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animation-easing_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animation-easing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-animation-easing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animation-fps_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-animation-fps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-animation-fps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-focus-follows-mouse_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-focus-follows-mouse_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-focus-follows-mouse commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-inner-gaps_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-inner-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-inner-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-mouse-follows-focus_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-mouse-follows-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-mouse-follows-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-mouse-hides-on-focus_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-mouse-hides-on-focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-mouse-hides-on-focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-outer-gaps_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-outer-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-outer-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-stack-default-orientation_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-stack-default-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-stack-default-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-stack-offset_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-stack-offset_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-stack-offset commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-workspace-names_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set-workspace-names_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute config set-workspace-names commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__debug_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__debug_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute debug commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__display_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__display_commands() {
    local commands; commands=(
'focus:Focus a display by direction, index, or UUID' \
'move-mouse-to-index:Move mouse cursor to a display by index (0-based)' \
'move-mouse-to-uuid:Move mouse cursor to a display by UUID' \
'move-window:Move a window to a display by direction, index, or UUID' \
    )
    _describe -t commands 'rift-cli help execute display commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__focus_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute display focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move-mouse-to-index_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move-mouse-to-index_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute display move-mouse-to-index commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move-mouse-to-uuid_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move-mouse-to-uuid_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute display move-mouse-to-uuid commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute display move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout_commands() {
    local commands; commands=(
'ascend:Move selection up the tree' \
'descend:Move selection down the tree' \
'move-node:Move the selected node in a direction' \
'join-window:Join the selected window with neighbor in a direction' \
'toggle-stack:Toggle stacked state for the selected container' \
'toggle-orientation:Global orientation toggle that works consistently across layout modes (and between splits/stacks)' \
'unjoin:Unjoin previously joined windows' \
'toggle-focus-float:Toggle floating on the focused selection (tree focus)' \
'adjust-master-ratio:Adjust master ratio by a delta (master/stack layout only)' \
'adjust-master-count:Adjust master count by a delta (master/stack layout only)' \
'promote-to-master:Promote the selected window into the master area (master/stack layout only)' \
'swap-master-stack:Swap the first master with the first stack window (master/stack layout only)' \
'swap-windows:Swap two windows by window id (\`WindowId { pid\: ..., idx\: ... }\`)' \
'scroll-strip:Scroll the strip by a normalized delta (scrolling layout only)' \
'snap-strip:Snap the strip to the nearest column boundary (scrolling layout only)' \
'center-selection:Toggle centering of the selected column in scrolling layout. If invoked again on the same selection, centering is removed' \
    )
    _describe -t commands 'rift-cli help execute layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust-master-count_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust-master-count_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout adjust-master-count commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust-master-ratio_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust-master-ratio_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout adjust-master-ratio commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__ascend_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__ascend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout ascend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__center-selection_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__center-selection_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout center-selection commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__descend_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__descend_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout descend commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__join-window_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__join-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout join-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__move-node_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__move-node_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout move-node commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__promote-to-master_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__promote-to-master_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout promote-to-master commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__scroll-strip_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__scroll-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout scroll-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__snap-strip_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__snap-strip_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout snap-strip commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap-master-stack_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap-master-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout swap-master-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap-windows_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap-windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout swap-windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle-focus-float_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle-focus-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout toggle-focus-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle-orientation_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle-orientation_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout toggle-orientation commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle-stack_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle-stack_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout toggle-stack commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__unjoin_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__unjoin_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute layout unjoin commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control_commands() {
    local commands; commands=(
'show-all:Show all workspaces in mission control' \
'show-current:Show current workspace in mission control' \
'dismiss:Dismiss mission control' \
    )
    _describe -t commands 'rift-cli help execute mission-control commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control__subcmd__dismiss_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control__subcmd__dismiss_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute mission-control dismiss commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control__subcmd__show-all_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control__subcmd__show-all_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute mission-control show-all commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control__subcmd__show-current_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__mission-control__subcmd__show-current_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute mission-control show-current commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__save-and-exit_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__save-and-exit_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute save-and-exit commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__serialize_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__serialize_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute serialize commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__show-timing_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__show-timing_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute show-timing commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__toggle-space-activated_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__toggle-space-activated_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute toggle-space-activated commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window_commands() {
    local commands; commands=(
'next:Focus the next window' \
'prev:Focus the previous window' \
'focus:Move focus in a direction' \
'toggle-float:Toggle window floating state' \
'toggle-fullscreen:Toggle fullscreen mode (fills the whole screen, ignores outer gaps)' \
'toggle-fullscreen-within-gaps:Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)' \
'resize-grow:Grow the current window size (increments by ~5%)' \
'resize-shrink:Shrink the current window size (decrements by ~5%)' \
'resize-by:Resize the selected window by a fractional amount. - Pass a signed floating value\: positive to grow, negative to shrink. - The value is a fraction of the current size (e.g. \`0.05\` = 5%). Examples\: rift-cli execute window resize-by --amount 0.05    # grow by 5% rift-cli execute window resize-by --amount -0.10   # shrink by 10%' \
'close:Close a window by window server identifier' \
'add-scratchpad:' \
'toggle-scratchpad:' \
    )
    _describe -t commands 'rift-cli help execute window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__add-scratchpad_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__add-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window add-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__close_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__close_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window close commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__focus_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__focus_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window focus commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__next_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__prev_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize-by_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize-by_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window resize-by commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize-grow_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize-grow_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window resize-grow commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize-shrink_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize-shrink_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window resize-shrink commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-float_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-float_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window toggle-float commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window toggle-fullscreen commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen-within-gaps_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-fullscreen-within-gaps_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window toggle-fullscreen-within-gaps commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-scratchpad_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle-scratchpad_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute window toggle-scratchpad commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace_commands() {
    local commands; commands=(
'next:Switch to next workspace' \
'prev:Switch to previous workspace' \
'switch:Switch to specific workspace' \
'move-window:Move current window to workspace' \
'create:Create a new workspace' \
'last:Switch to the last workspace' \
'set-layout:Set layout mode for a workspace (or active workspace when omitted)' \
    )
    _describe -t commands 'rift-cli help execute workspace commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__create_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__create_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace create commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__last_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__last_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace last commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__move-window_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__move-window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace move-window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__next_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__next_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace next commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__prev_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__prev_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace prev commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__set-layout_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__set-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace set-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__switch_commands] )) ||
_rift-cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__switch_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help execute workspace switch commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query_commands] )) ||
_rift-cli__subcmd__help__subcmd__query_commands() {
    local commands; commands=(
'workspaces:List virtual workspaces (optionally for a specific MacOS space)' \
'windows:List windows (optionally filtered by space)' \
'displays:List connected displays' \
'window:Get information about a specific window' \
'applications:List running applications' \
'layout:Get layout state for a space' \
'workspace-layout:Get workspace layout-engine mode(s)' \
'metrics:Get performance metrics' \
    )
    _describe -t commands 'rift-cli help query commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__applications_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__applications_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query applications commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__displays_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__displays_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query displays commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__layout_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__metrics_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__metrics_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query metrics commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__window_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__windows_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__workspace-layout_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__workspace-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query workspace-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__query__subcmd__workspaces_commands] )) ||
_rift-cli__subcmd__help__subcmd__query__subcmd__workspaces_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help query workspaces commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__service_commands] )) ||
_rift-cli__subcmd__help__subcmd__service_commands() {
    local commands; commands=(
'install:Install the per-user launchd service' \
'uninstall:Uninstall the per-user launchd service' \
'start:Start (or bootstrap) the service' \
'stop:Stop (or bootout/kill) the service' \
'restart:Restart the service (kickstart -k)' \
    )
    _describe -t commands 'rift-cli help service commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__service__subcmd__install_commands] )) ||
_rift-cli__subcmd__help__subcmd__service__subcmd__install_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help service install commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__service__subcmd__restart_commands] )) ||
_rift-cli__subcmd__help__subcmd__service__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help service restart commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__service__subcmd__start_commands] )) ||
_rift-cli__subcmd__help__subcmd__service__subcmd__start_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help service start commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__service__subcmd__stop_commands] )) ||
_rift-cli__subcmd__help__subcmd__service__subcmd__stop_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help service stop commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__service__subcmd__uninstall_commands] )) ||
_rift-cli__subcmd__help__subcmd__service__subcmd__uninstall_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help service uninstall commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__subscribe_commands] )) ||
_rift-cli__subcmd__help__subcmd__subscribe_commands() {
    local commands; commands=(
'mach:Subscribe to Mach IPC events' \
'cli:Subscribe to events via CLI command execution' \
'unsub-mach:Unsubscribe from Mach IPC events' \
'unsub-cli:Unsubscribe from CLI events' \
'list-cli:List current CLI subscriptions' \
    )
    _describe -t commands 'rift-cli help subscribe commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__subscribe__subcmd__cli_commands] )) ||
_rift-cli__subcmd__help__subcmd__subscribe__subcmd__cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help subscribe cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__subscribe__subcmd__list-cli_commands] )) ||
_rift-cli__subcmd__help__subcmd__subscribe__subcmd__list-cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help subscribe list-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__subscribe__subcmd__mach_commands] )) ||
_rift-cli__subcmd__help__subcmd__subscribe__subcmd__mach_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help subscribe mach commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__subscribe__subcmd__unsub-cli_commands] )) ||
_rift-cli__subcmd__help__subcmd__subscribe__subcmd__unsub-cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help subscribe unsub-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__subscribe__subcmd__unsub-mach_commands] )) ||
_rift-cli__subcmd__help__subcmd__subscribe__subcmd__unsub-mach_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help subscribe unsub-mach commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__help__subcmd__verify_commands] )) ||
_rift-cli__subcmd__help__subcmd__verify_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli help verify commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query_commands] )) ||
_rift-cli__subcmd__query_commands() {
    local commands; commands=(
'workspaces:List virtual workspaces (optionally for a specific MacOS space)' \
'windows:List windows (optionally filtered by space)' \
'displays:List connected displays' \
'window:Get information about a specific window' \
'applications:List running applications' \
'layout:Get layout state for a space' \
'workspace-layout:Get workspace layout-engine mode(s)' \
'metrics:Get performance metrics' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli query commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__applications_commands] )) ||
_rift-cli__subcmd__query__subcmd__applications_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query applications commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__displays_commands] )) ||
_rift-cli__subcmd__query__subcmd__displays_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query displays commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help_commands] )) ||
_rift-cli__subcmd__query__subcmd__help_commands() {
    local commands; commands=(
'workspaces:List virtual workspaces (optionally for a specific MacOS space)' \
'windows:List windows (optionally filtered by space)' \
'displays:List connected displays' \
'window:Get information about a specific window' \
'applications:List running applications' \
'layout:Get layout state for a space' \
'workspace-layout:Get workspace layout-engine mode(s)' \
'metrics:Get performance metrics' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli query help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__applications_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__applications_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help applications commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__displays_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__displays_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help displays commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__layout_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__metrics_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__metrics_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help metrics commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__window_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__windows_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__workspace-layout_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__workspace-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help workspace-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__help__subcmd__workspaces_commands] )) ||
_rift-cli__subcmd__query__subcmd__help__subcmd__workspaces_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query help workspaces commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__layout_commands] )) ||
_rift-cli__subcmd__query__subcmd__layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__metrics_commands] )) ||
_rift-cli__subcmd__query__subcmd__metrics_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query metrics commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__window_commands] )) ||
_rift-cli__subcmd__query__subcmd__window_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query window commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__windows_commands] )) ||
_rift-cli__subcmd__query__subcmd__windows_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query windows commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__workspace-layout_commands] )) ||
_rift-cli__subcmd__query__subcmd__workspace-layout_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query workspace-layout commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__query__subcmd__workspaces_commands] )) ||
_rift-cli__subcmd__query__subcmd__workspaces_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli query workspaces commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service_commands] )) ||
_rift-cli__subcmd__service_commands() {
    local commands; commands=(
'install:Install the per-user launchd service' \
'uninstall:Uninstall the per-user launchd service' \
'start:Start (or bootstrap) the service' \
'stop:Stop (or bootout/kill) the service' \
'restart:Restart the service (kickstart -k)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli service commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help_commands] )) ||
_rift-cli__subcmd__service__subcmd__help_commands() {
    local commands; commands=(
'install:Install the per-user launchd service' \
'uninstall:Uninstall the per-user launchd service' \
'start:Start (or bootstrap) the service' \
'stop:Stop (or bootout/kill) the service' \
'restart:Restart the service (kickstart -k)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli service help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__service__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help__subcmd__install_commands] )) ||
_rift-cli__subcmd__service__subcmd__help__subcmd__install_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service help install commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help__subcmd__restart_commands] )) ||
_rift-cli__subcmd__service__subcmd__help__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service help restart commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help__subcmd__start_commands] )) ||
_rift-cli__subcmd__service__subcmd__help__subcmd__start_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service help start commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help__subcmd__stop_commands] )) ||
_rift-cli__subcmd__service__subcmd__help__subcmd__stop_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service help stop commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__help__subcmd__uninstall_commands] )) ||
_rift-cli__subcmd__service__subcmd__help__subcmd__uninstall_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service help uninstall commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__install_commands] )) ||
_rift-cli__subcmd__service__subcmd__install_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service install commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__restart_commands] )) ||
_rift-cli__subcmd__service__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service restart commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__start_commands] )) ||
_rift-cli__subcmd__service__subcmd__start_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service start commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__stop_commands] )) ||
_rift-cli__subcmd__service__subcmd__stop_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service stop commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__service__subcmd__uninstall_commands] )) ||
_rift-cli__subcmd__service__subcmd__uninstall_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli service uninstall commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe_commands] )) ||
_rift-cli__subcmd__subscribe_commands() {
    local commands; commands=(
'mach:Subscribe to Mach IPC events' \
'cli:Subscribe to events via CLI command execution' \
'unsub-mach:Unsubscribe from Mach IPC events' \
'unsub-cli:Unsubscribe from CLI events' \
'list-cli:List current CLI subscriptions' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli subscribe commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__cli_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help_commands() {
    local commands; commands=(
'mach:Subscribe to Mach IPC events' \
'cli:Subscribe to events via CLI command execution' \
'unsub-mach:Unsubscribe from Mach IPC events' \
'unsub-cli:Unsubscribe from CLI events' \
'list-cli:List current CLI subscriptions' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift-cli subscribe help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help__subcmd__cli_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help__subcmd__cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe help cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help__subcmd__help_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe help help commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help__subcmd__list-cli_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help__subcmd__list-cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe help list-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help__subcmd__mach_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help__subcmd__mach_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe help mach commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help__subcmd__unsub-cli_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help__subcmd__unsub-cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe help unsub-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__help__subcmd__unsub-mach_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__help__subcmd__unsub-mach_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe help unsub-mach commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__list-cli_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__list-cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe list-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__mach_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__mach_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe mach commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__unsub-cli_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__unsub-cli_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe unsub-cli commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__subscribe__subcmd__unsub-mach_commands] )) ||
_rift-cli__subcmd__subscribe__subcmd__unsub-mach_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli subscribe unsub-mach commands' commands "$@"
}
(( $+functions[_rift-cli__subcmd__verify_commands] )) ||
_rift-cli__subcmd__verify_commands() {
    local commands; commands=()
    _describe -t commands 'rift-cli verify commands' commands "$@"
}

if [ "$funcstack[1]" = "_rift-cli" ]; then
    _rift-cli "$@"
else
    compdef _rift-cli rift-cli
fi
