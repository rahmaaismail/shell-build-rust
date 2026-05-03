#[allow(unused_imports)]
use std::io::{self, Write};
use std::env;
use std::path::Path;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

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
        let parts: Vec<&str> = input.split_whitespace().collect();
        
        if parts.is_empty() {
            continue;
        }

        let command = parts[0];
        let args = &parts[1..];

        if command == "exit" {
            break;
        } else if command == "echo" {
            println!("{}", args.join(" "));
        } else if command == "type" {
            if let Some(arg) = args.first() {
                if builtins.contains(arg) {
                    println!("{} is a shell builtin", arg);
                } else if let Some(path) = find_in_path(arg) {
                    println!("{} is {}", arg, path);
                } else {
                    println!("{}: not found", arg);
                }
            }
        } else if command == "pwd" {
            let cwd = env::current_dir().unwrap();
            println!("{}", cwd.display());
        } else if command == "cd" {
            if let Some(dir) = args.first() {
                let target = if *dir == "~" {
                    env:: var("HOME").unwrap_or_default()
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
            Command::new(command)
                .args(args)
                .status()
                .unwrap();
        } else {
            println!("{}: command not found", command);
        }
    }
}