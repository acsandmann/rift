module completions {

  export extern rift [
    --one                     # Only run the window manager on the current space
    --default-disable         # Disable new spaces by default
    --no-animate              # Disable animations
    --validate                # No-op compatibility check for the deprecated restore file path
    --restore                 # Deprecated no-op flag retained for CLI compatibility
    --record: path            # Record reactor events to the specified file path. Overwrites the file if exists
    --config: path            # Path to configuration file to use (overrides default)
    --help(-h)                # Print help (see more with '--help')
  ]

  # Manage the launchd service for rift
  export extern "rift service" [
    --help(-h)                # Print help
  ]

  # Install the per-user launchd service
  export extern "rift service install" [
    --help(-h)                # Print help
  ]

  # Uninstall the per-user launchd service
  export extern "rift service uninstall" [
    --help(-h)                # Print help
  ]

  # Start (or bootstrap) the service
  export extern "rift service start" [
    --help(-h)                # Print help
  ]

  # Stop (or bootout/kill) the service
  export extern "rift service stop" [
    --help(-h)                # Print help
  ]

  # Restart the service (kickstart -k)
  export extern "rift service restart" [
    --help(-h)                # Print help
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift service help" [
  ]

  # Install the per-user launchd service
  export extern "rift service help install" [
  ]

  # Uninstall the per-user launchd service
  export extern "rift service help uninstall" [
  ]

  # Start (or bootstrap) the service
  export extern "rift service help start" [
  ]

  # Stop (or bootout/kill) the service
  export extern "rift service help stop" [
  ]

  # Restart the service (kickstart -k)
  export extern "rift service help restart" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift service help help" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift help" [
  ]

  # Manage the launchd service for rift
  export extern "rift help service" [
  ]

  # Install the per-user launchd service
  export extern "rift help service install" [
  ]

  # Uninstall the per-user launchd service
  export extern "rift help service uninstall" [
  ]

  # Start (or bootstrap) the service
  export extern "rift help service start" [
  ]

  # Stop (or bootout/kill) the service
  export extern "rift help service stop" [
  ]

  # Restart the service (kickstart -k)
  export extern "rift help service restart" [
  ]

  # Print this message or the help of the given subcommand(s)
  export extern "rift help help" [
  ]

}

export use completions *
