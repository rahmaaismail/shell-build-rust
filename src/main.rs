#[allow(unused_imports)]
use std::io::{self, Write};
use std::env;
use std::path::Path;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::fs::File;

fn parse_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single_quote && !in_double_quote => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\\' if in_double_quote => {
                if let Some(next) = chars.next() {
                    if next == '"' || next == '\\' {
                        current.push(next);
                    } else {
                        current.push('\\');
                        current.push(next);
                    }
                }
            }
            '\'' if !in_single_quote && !in_double_quote => {
                in_single_quote = true;
            }
            '\'' if in_single_quote => {
                in_single_quote = false;
            }
            '"' if !in_single_quote && !in_double_quote => {
                in_double_quote = true;
            }
            '"' if in_double_quote => {
                in_double_quote = false;
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

// returns (args_without_redirect, Option<output_file>)
fn extract_redirect(parts: &[String]) -> (Vec<String>, Option<String>) {
    let mut args = Vec::new();
    let mut output_file = None;
    let mut i = 0;

    while i < parts.len() {
        if parts[i] == ">" || parts[i] == "1>" {
            if i + 1 < parts.len() {
                output_file = Some(parts[i + 1].clone());
                i += 2;
            }
        } else {
            args.push(parts[i].clone());
            i += 1;
        }
    }

    (args, output_file)
}

fn find_in_path(command: &str) -> Option<String> {
    let path_var = env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let full_path = format!("{}/{}", dir, command);
        let path = Path::new(&full_path);
        if path.exists() {
            if let Ok(metadata) = path.metadata() {
                if metadata.permissions().mode() & 0o111 != 0 {
                    return Some(full_path);
                }
            }
        }
    }
    None
}

fn main() {
    let builtins = vec!["echo", "exit", "type", "pwd", "cd"];

    loop {
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();

        let parts = parse_args(input);
        if parts.is_empty() {
            continue;
        }

        let (parts, output_file) = extract_redirect(&parts);
        if parts.is_empty() {
            continue;
        }

        let command = &parts[0];
        let args = &parts[1..];

        if command == "exit" {
            break;
        } else if command == "echo" {
            let output = args.join(" ");
            if let Some(ref file) = output_file {
                let mut f = File::create(file).unwrap();
                writeln!(f, "{}", output).unwrap();
            } else {
                println!("{}", output);
            }
        } else if command == "type" {
            if let Some(arg) = args.first() {
                let result = if builtins.contains(&arg.as_str()) {
                    format!("{} is a shell builtin", arg)
                } else if let Some(path) = find_in_path(arg) {
                    format!("{} is {}", arg, path)
                } else {
                    format!("{}: not found", arg)
                };
                if let Some(ref file) = output_file {
                    let mut f = File::create(file).unwrap();
                    writeln!(f, "{}", result).unwrap();
                } else {
                    println!("{}", result);
                }
            }
        } else if command == "pwd" {
            let cwd = env::current_dir().unwrap();
            let output = cwd.display().to_string();
            if let Some(ref file) = output_file {
                let mut f = File::create(file).unwrap();
                writeln!(f, "{}", output).unwrap();
            } else {
                println!("{}", output);
            }
        } else if command == "cd" {
            if let Some(dir) = args.first() {
                let target = if dir == "~" {
                    env::var("HOME").unwrap_or_default()
                } else {
                    dir.to_string()
                };
                let path = Path::new(&target);
                if path.exists() {
                    env::set_current_dir(path).unwrap();
                } else {
                    println!("cd: {}: No such file or directory", dir);
                }
            }
        } else if let Some(_path) = find_in_path(command) {
            if let Some(ref file) = output_file {
                let f = File::create(file).unwrap();
                Command::new(command)
                    .args(args)
                    .stdout(Stdio::from(f))
                    .status()
                    .unwrap();
            } else {
                Command::new(command)
                    .args(args)
                    .status()
                    .unwrap();
            }
        } else {
            println!("{}: command not found", command);
        }
    }
}