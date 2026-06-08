
use builtin;
use str;

set edit:completion:arg-completer[rift] = {|@words|
    fn spaces {|n|
        builtin:repeat $n ' ' | str:join ''
    }
    fn cand {|text desc|
        edit:complex-candidate $text &display=$text' '(spaces (- 14 (wcswidth $text)))$desc
    }
    var command = 'rift'
    for word $words[1..-1] {
        if (str:has-prefix $word '-') {
            break
        }
        set command = $command';'$word
    }
    var completions = [
        &'rift'= {
            cand --record 'Record reactor events to the specified file path. Overwrites the file if exists'
            cand --config 'Path to configuration file to use (overrides default)'
            cand --one 'Only run the window manager on the current space'
            cand --default-disable 'Disable new spaces by default'
            cand --no-animate 'Disable animations'
            cand --validate 'No-op compatibility check for the deprecated restore file path'
            cand --restore 'Deprecated no-op flag retained for CLI compatibility'
            cand -h 'Print help (see more with ''--help'')'
            cand --help 'Print help (see more with ''--help'')'
            cand service 'Manage the launchd service for rift'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift;service'= {
            cand -h 'Print help'
            cand --help 'Print help'
            cand install 'Install the per-user launchd service'
            cand uninstall 'Uninstall the per-user launchd service'
            cand start 'Start (or bootstrap) the service'
            cand stop 'Stop (or bootout/kill) the service'
            cand restart 'Restart the service (kickstart -k)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift;service;install'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift;service;uninstall'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift;service;start'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift;service;stop'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift;service;restart'= {
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'rift;service;help'= {
            cand install 'Install the per-user launchd service'
            cand uninstall 'Uninstall the per-user launchd service'
            cand start 'Start (or bootstrap) the service'
            cand stop 'Stop (or bootout/kill) the service'
            cand restart 'Restart the service (kickstart -k)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift;service;help;install'= {
        }
        &'rift;service;help;uninstall'= {
        }
        &'rift;service;help;start'= {
        }
        &'rift;service;help;stop'= {
        }
        &'rift;service;help;restart'= {
        }
        &'rift;service;help;help'= {
        }
        &'rift;help'= {
            cand service 'Manage the launchd service for rift'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'rift;help;service'= {
            cand install 'Install the per-user launchd service'
            cand uninstall 'Uninstall the per-user launchd service'
            cand start 'Start (or bootstrap) the service'
            cand stop 'Stop (or bootout/kill) the service'
            cand restart 'Restart the service (kickstart -k)'
        }
        &'rift;help;service;install'= {
        }
        &'rift;help;service;uninstall'= {
        }
        &'rift;help;service;start'= {
        }
        &'rift;help;service;stop'= {
        }
        &'rift;help;service;restart'= {
        }
        &'rift;help;help'= {
        }
    ]
    $completions[$command]
}
