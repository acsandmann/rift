# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_rift_global_optspecs
	string join \n one default-disable no-animate validate restore record= config= h/help
end

function __fish_rift_needs_command
	# Figure out if the current invocation already has a command.
	set -l cmd (commandline -opc)
	set -e cmd[1]
	argparse -s (__fish_rift_global_optspecs) -- $cmd 2>/dev/null
	or return
	if set -q argv[1]
		# Also print the command, so this can be used to figure out what it is.
		echo $argv[1]
		return 1
	end
	return 0
end

function __fish_rift_using_subcommand
	set -l cmd (__fish_rift_needs_command)
	test -z "$cmd"
	and return 1
	contains -- $cmd[1] $argv
end

complete -c rift -n "__fish_rift_needs_command" -l record -d 'Record reactor events to the specified file path. Overwrites the file if exists' -r -F
complete -c rift -n "__fish_rift_needs_command" -l config -d 'Path to configuration file to use (overrides default)' -r -F
complete -c rift -n "__fish_rift_needs_command" -l one -d 'Only run the window manager on the current space'
complete -c rift -n "__fish_rift_needs_command" -l default-disable -d 'Disable new spaces by default'
complete -c rift -n "__fish_rift_needs_command" -l no-animate -d 'Disable animations'
complete -c rift -n "__fish_rift_needs_command" -l validate -d 'No-op compatibility check for the deprecated restore file path'
complete -c rift -n "__fish_rift_needs_command" -l restore -d 'Deprecated no-op flag retained for CLI compatibility'
complete -c rift -n "__fish_rift_needs_command" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c rift -n "__fish_rift_needs_command" -f -a "service" -d 'Manage the launchd service for rift'
complete -c rift -n "__fish_rift_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -s h -l help -d 'Print help'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -f -a "install" -d 'Install the per-user launchd service'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -f -a "uninstall" -d 'Uninstall the per-user launchd service'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -f -a "start" -d 'Start (or bootstrap) the service'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -f -a "stop" -d 'Stop (or bootout/kill) the service'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -f -a "restart" -d 'Restart the service (kickstart -k)'
complete -c rift -n "__fish_rift_using_subcommand service; and not __fish_seen_subcommand_from install uninstall start stop restart help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from install" -s h -l help -d 'Print help'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from uninstall" -s h -l help -d 'Print help'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from start" -s h -l help -d 'Print help'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from stop" -s h -l help -d 'Print help'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from restart" -s h -l help -d 'Print help'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from help" -f -a "install" -d 'Install the per-user launchd service'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from help" -f -a "uninstall" -d 'Uninstall the per-user launchd service'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from help" -f -a "start" -d 'Start (or bootstrap) the service'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from help" -f -a "stop" -d 'Stop (or bootout/kill) the service'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from help" -f -a "restart" -d 'Restart the service (kickstart -k)'
complete -c rift -n "__fish_rift_using_subcommand service; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c rift -n "__fish_rift_using_subcommand help; and not __fish_seen_subcommand_from service help" -f -a "service" -d 'Manage the launchd service for rift'
complete -c rift -n "__fish_rift_using_subcommand help; and not __fish_seen_subcommand_from service help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c rift -n "__fish_rift_using_subcommand help; and __fish_seen_subcommand_from service" -f -a "install" -d 'Install the per-user launchd service'
complete -c rift -n "__fish_rift_using_subcommand help; and __fish_seen_subcommand_from service" -f -a "uninstall" -d 'Uninstall the per-user launchd service'
complete -c rift -n "__fish_rift_using_subcommand help; and __fish_seen_subcommand_from service" -f -a "start" -d 'Start (or bootstrap) the service'
complete -c rift -n "__fish_rift_using_subcommand help; and __fish_seen_subcommand_from service" -f -a "stop" -d 'Stop (or bootout/kill) the service'
complete -c rift -n "__fish_rift_using_subcommand help; and __fish_seen_subcommand_from service" -f -a "restart" -d 'Restart the service (kickstart -k)'
