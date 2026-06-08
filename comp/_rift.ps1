
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'rift' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'rift'
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
        'rift' {
            [CompletionResult]::new('--record', '--record', [CompletionResultType]::ParameterName, 'Record reactor events to the specified file path. Overwrites the file if exists')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to configuration file to use (overrides default)')
            [CompletionResult]::new('--one', '--one', [CompletionResultType]::ParameterName, 'Only run the window manager on the current space')
            [CompletionResult]::new('--default-disable', '--default-disable', [CompletionResultType]::ParameterName, 'Disable new spaces by default')
            [CompletionResult]::new('--no-animate', '--no-animate', [CompletionResultType]::ParameterName, 'Disable animations')
            [CompletionResult]::new('--validate', '--validate', [CompletionResultType]::ParameterName, 'No-op compatibility check for the deprecated restore file path')
            [CompletionResult]::new('--restore', '--restore', [CompletionResultType]::ParameterName, 'Deprecated no-op flag retained for CLI compatibility')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('service', 'service', [CompletionResultType]::ParameterValue, 'Manage the launchd service for rift')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift;service' {
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
        'rift;service;install' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift;service;uninstall' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift;service;start' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift;service;stop' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift;service;restart' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'rift;service;help' {
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Install the per-user launchd service')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Uninstall the per-user launchd service')
            [CompletionResult]::new('start', 'start', [CompletionResultType]::ParameterValue, 'Start (or bootstrap) the service')
            [CompletionResult]::new('stop', 'stop', [CompletionResultType]::ParameterValue, 'Stop (or bootout/kill) the service')
            [CompletionResult]::new('restart', 'restart', [CompletionResultType]::ParameterValue, 'Restart the service (kickstart -k)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift;service;help;install' {
            break
        }
        'rift;service;help;uninstall' {
            break
        }
        'rift;service;help;start' {
            break
        }
        'rift;service;help;stop' {
            break
        }
        'rift;service;help;restart' {
            break
        }
        'rift;service;help;help' {
            break
        }
        'rift;help' {
            [CompletionResult]::new('service', 'service', [CompletionResultType]::ParameterValue, 'Manage the launchd service for rift')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'rift;help;service' {
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Install the per-user launchd service')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Uninstall the per-user launchd service')
            [CompletionResult]::new('start', 'start', [CompletionResultType]::ParameterValue, 'Start (or bootstrap) the service')
            [CompletionResult]::new('stop', 'stop', [CompletionResultType]::ParameterValue, 'Stop (or bootout/kill) the service')
            [CompletionResult]::new('restart', 'restart', [CompletionResultType]::ParameterValue, 'Restart the service (kickstart -k)')
            break
        }
        'rift;help;service;install' {
            break
        }
        'rift;help;service;uninstall' {
            break
        }
        'rift;help;service;start' {
            break
        }
        'rift;help;service;stop' {
            break
        }
        'rift;help;service;restart' {
            break
        }
        'rift;help;help' {
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
