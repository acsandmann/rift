#compdef rift

autoload -U is-at-least

_rift() {
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
'--record=[Record reactor events to the specified file path. Overwrites the file if exists]:RECORD:_files' \
'--config=[Path to configuration file to use (overrides default)]:PATH:_files' \
'--one[Only run the window manager on the current space]' \
'--default-disable[Disable new spaces by default]' \
'--no-animate[Disable animations]' \
'--validate[No-op compatibility check for the deprecated restore file path]' \
'--restore[Deprecated no-op flag retained for CLI compatibility]' \
'-h[Print help (see more with '\''--help'\'')]' \
'--help[Print help (see more with '\''--help'\'')]' \
":: :_rift_commands" \
"*::: :->rift-wm" \
&& ret=0
    case $state in
    (rift-wm)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-command-$line[1]:"
        case $line[1] in
            (service)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
":: :_rift__subcmd__service_commands" \
"*::: :->service" \
&& ret=0

    case $state in
    (service)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-service-command-$line[1]:"
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
":: :_rift__subcmd__service__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-service-help-command-$line[1]:"
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
(help)
_arguments "${_arguments_options[@]}" : \
":: :_rift__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-help-command-$line[1]:"
        case $line[1] in
            (service)
_arguments "${_arguments_options[@]}" : \
":: :_rift__subcmd__help__subcmd__service_commands" \
"*::: :->service" \
&& ret=0

    case $state in
    (service)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:rift-help-service-command-$line[1]:"
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

(( $+functions[_rift_commands] )) ||
_rift_commands() {
    local commands; commands=(
'service:Manage the launchd service for rift' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift commands' commands "$@"
}
(( $+functions[_rift__subcmd__help_commands] )) ||
_rift__subcmd__help_commands() {
    local commands; commands=(
'service:Manage the launchd service for rift' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift help commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__help_commands] )) ||
_rift__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift help help commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__service_commands] )) ||
_rift__subcmd__help__subcmd__service_commands() {
    local commands; commands=(
'install:Install the per-user launchd service' \
'uninstall:Uninstall the per-user launchd service' \
'start:Start (or bootstrap) the service' \
'stop:Stop (or bootout/kill) the service' \
'restart:Restart the service (kickstart -k)' \
    )
    _describe -t commands 'rift help service commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__service__subcmd__install_commands] )) ||
_rift__subcmd__help__subcmd__service__subcmd__install_commands() {
    local commands; commands=()
    _describe -t commands 'rift help service install commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__service__subcmd__restart_commands] )) ||
_rift__subcmd__help__subcmd__service__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'rift help service restart commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__service__subcmd__start_commands] )) ||
_rift__subcmd__help__subcmd__service__subcmd__start_commands() {
    local commands; commands=()
    _describe -t commands 'rift help service start commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__service__subcmd__stop_commands] )) ||
_rift__subcmd__help__subcmd__service__subcmd__stop_commands() {
    local commands; commands=()
    _describe -t commands 'rift help service stop commands' commands "$@"
}
(( $+functions[_rift__subcmd__help__subcmd__service__subcmd__uninstall_commands] )) ||
_rift__subcmd__help__subcmd__service__subcmd__uninstall_commands() {
    local commands; commands=()
    _describe -t commands 'rift help service uninstall commands' commands "$@"
}
(( $+functions[_rift__subcmd__service_commands] )) ||
_rift__subcmd__service_commands() {
    local commands; commands=(
'install:Install the per-user launchd service' \
'uninstall:Uninstall the per-user launchd service' \
'start:Start (or bootstrap) the service' \
'stop:Stop (or bootout/kill) the service' \
'restart:Restart the service (kickstart -k)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift service commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help_commands] )) ||
_rift__subcmd__service__subcmd__help_commands() {
    local commands; commands=(
'install:Install the per-user launchd service' \
'uninstall:Uninstall the per-user launchd service' \
'start:Start (or bootstrap) the service' \
'stop:Stop (or bootout/kill) the service' \
'restart:Restart the service (kickstart -k)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'rift service help commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help__subcmd__help_commands] )) ||
_rift__subcmd__service__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'rift service help help commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help__subcmd__install_commands] )) ||
_rift__subcmd__service__subcmd__help__subcmd__install_commands() {
    local commands; commands=()
    _describe -t commands 'rift service help install commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help__subcmd__restart_commands] )) ||
_rift__subcmd__service__subcmd__help__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'rift service help restart commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help__subcmd__start_commands] )) ||
_rift__subcmd__service__subcmd__help__subcmd__start_commands() {
    local commands; commands=()
    _describe -t commands 'rift service help start commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help__subcmd__stop_commands] )) ||
_rift__subcmd__service__subcmd__help__subcmd__stop_commands() {
    local commands; commands=()
    _describe -t commands 'rift service help stop commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__help__subcmd__uninstall_commands] )) ||
_rift__subcmd__service__subcmd__help__subcmd__uninstall_commands() {
    local commands; commands=()
    _describe -t commands 'rift service help uninstall commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__install_commands] )) ||
_rift__subcmd__service__subcmd__install_commands() {
    local commands; commands=()
    _describe -t commands 'rift service install commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__restart_commands] )) ||
_rift__subcmd__service__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'rift service restart commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__start_commands] )) ||
_rift__subcmd__service__subcmd__start_commands() {
    local commands; commands=()
    _describe -t commands 'rift service start commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__stop_commands] )) ||
_rift__subcmd__service__subcmd__stop_commands() {
    local commands; commands=()
    _describe -t commands 'rift service stop commands' commands "$@"
}
(( $+functions[_rift__subcmd__service__subcmd__uninstall_commands] )) ||
_rift__subcmd__service__subcmd__uninstall_commands() {
    local commands; commands=()
    _describe -t commands 'rift service uninstall commands' commands "$@"
}

if [ "$funcstack[1]" = "_rift" ]; then
    _rift "$@"
else
    compdef _rift rift
fi
