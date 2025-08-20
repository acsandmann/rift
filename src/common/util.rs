use tracing::{error, trace};

pub fn parse_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current_part = String::new();
    let mut in_quotes = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                if in_quotes {
                    in_quotes = false;
                } else {
                    in_quotes = true;
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current_part.is_empty() {
                    parts.push(current_part.clone());
                    current_part.clear();
                }
            }
            '\\' if in_quotes => {
                if let Some(next_ch) = chars.next() {
                    match next_ch {
                        'n' => current_part.push('\n'),
                        't' => current_part.push('\t'),
                        'r' => current_part.push('\r'),
                        '\\' => current_part.push('\\'),
                        '\'' => current_part.push('\''),
                        '"' => current_part.push('"'),
                        _ => {
                            current_part.push('\\');
                            current_part.push(next_ch);
                        }
                    }
                } else {
                    current_part.push('\\');
                }
            }
            _ => {
                current_part.push(ch);
            }
        }
    }

    if !current_part.is_empty() {
        parts.push(current_part);
    }

    parts
}

pub fn execute_startup_commands(commands: &[String]) {
    if commands.is_empty() {
        return;
    }

    trace!("Executing {} startup commands", commands.len());

    for (i, command) in commands.iter().enumerate() {
        trace!("Executing startup command {}: {}", i + 1, command);

        let parts = parse_command(command);
        if parts.is_empty() {
            error!("Empty startup command at index {}", i);
            continue;
        }

        let (cmd, args) = parts.split_first().unwrap();

        let cmd_owned = cmd.to_string();
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let command_str = command.clone();

        std::thread::spawn(move || {
            let output = std::process::Command::new(&cmd_owned).args(&args_owned).output();

            match output {
                Ok(output) => {
                    if output.status.success() {
                        trace!("Startup command completed successfully: {}", command_str);
                    } else {
                        error!(
                            "Startup command failed with status {}: {}",
                            output.status, command_str
                        );
                        if !output.stderr.is_empty() {
                            error!("stderr: {}", String::from_utf8_lossy(&output.stderr));
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to execute startup command '{}': {}", command_str, e);
                }
            }
        });
    }
}
