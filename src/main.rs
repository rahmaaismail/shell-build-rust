#[allow(unused_imports)]
use std::io::Write;
use std::env;
use std::path::Path;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::fs::{File, OpenOptions};
use std::cell::RefCell;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, Editor};
use rustyline_derive::Helper;

const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd"];

fn longest_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = &strings[0];
    let mut lcp_len = first.len();
    for s in &strings[1..] {
        lcp_len = lcp_len.min(s.len());
        lcp_len = first.chars().zip(s.chars())
            .take(lcp_len)
            .take_while(|(a, b)| a == b)
            .count();
    }
    first[..lcp_len].to_string()
}

#[derive(Helper)]
struct ShellHelper {
    last_prefix: RefCell<String>,
    tab_count: RefCell<usize>,
}

impl ShellHelper {
    fn new() -> Self {
        ShellHelper {
            last_prefix: RefCell::new(String::new()),
            tab_count: RefCell::new(0),
        }
    }

    fn get_matches(&self, prefix: &str) -> Vec<String> {
        let mut matches = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for &builtin in BUILTINS {
            if builtin.starts_with(prefix) {
                matches.push(builtin.to_string());
                seen.insert(builtin.to_string());
            }
        }

        if let Ok(path_var) = env::var("PATH") {
            for dir in path_var.split(':') {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with(prefix) && !seen.contains(&name) {
                            if let Ok(metadata) = entry.metadata() {
                                if metadata.permissions().mode() & 0o111 != 0 {
                                    seen.insert(name.clone());
                                    matches.push(name);
                                }
                            }
                        }
                    }
                }
            }
        }

        matches.sort();
        matches
    }
}

impl Completer for ShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let prefix = &line[..pos];
        let matches = self.get_matches(prefix);

        if matches.is_empty() {
            return Ok((0, vec![]));
        }

        if matches.len() == 1 {
            *self.last_prefix.borrow_mut() = String::new();
            *self.tab_count.borrow_mut() = 0;
            return Ok((0, vec![Pair {
                display: matches[0].clone(),
                replacement: format!("{} ", matches[0]),
            }]));
        }

        // find longest common prefix of all matches
        let lcp = longest_common_prefix(&matches);

        // if LCP is longer than what user typed, complete to LCP (no trailing space)
        if lcp.len() > prefix.len() {
            *self.last_prefix.borrow_mut() = String::new();
            *self.tab_count.borrow_mut() = 0;
            return Ok((0, vec![Pair {
                display: lcp.clone(),
                replacement: lcp,
            }]));
        }

        // LCP == prefix, multiple matches — bell then show all
        let current_prefix = prefix.to_string();
        let is_same_prefix = *self.last_prefix.borrow() == current_prefix;

        if is_same_prefix {
            *self.tab_count.borrow_mut() += 1;
        } else {
            *self.last_prefix.borrow_mut() = current_prefix;
            *self.tab_count.borrow_mut() = 1;
        }

        let count = *self.tab_count.borrow();

        if count == 1 {
            print!("\x07");
            std::io::stdout().flush().unwrap();
            return Ok((0, vec![]));
        } else {
            *self.tab_count.borrow_mut() = 0;
            println!();
            println!("{}", matches.join("  "));
            print!("$ {}", prefix);
            std::io::stdout().flush().unwrap();
            return Ok((0, vec![]));
        }
    }
}

impl Hinter for ShellHelper {
    type Hint = String;
}
impl Highlighter for ShellHelper {}
impl Validator for ShellHelper {}

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

fn extract_redirect(parts: &[String]) -> (Vec<String>, Option<(String, bool)>, Option<(String, bool)>) {
    let mut args = Vec::new();
    let mut stdout_file = None;
    let mut stderr_file = None;
    let mut i = 0;

    while i < parts.len() {
        if parts[i] == ">>" || parts[i] == "1>>" {
            if i + 1 < parts.len() {
                stdout_file = Some((parts[i + 1].clone(), true));
                i += 2;
            }
        } else if parts[i] == ">" || parts[i] == "1>" {
            if i + 1 < parts.len() {
                stdout_file = Some((parts[i + 1].clone(), false));
                i += 2;
            }
        } else if parts[i] == "2>>" {
            if i + 1 < parts.len() {
                stderr_file = Some((parts[i + 1].clone(), true));
                i += 2;
            }
        } else if parts[i] == "2>" {
            if i + 1 < parts.len() {
                stderr_file = Some((parts[i + 1].clone(), false));
                i += 2;
            }
        } else {
            args.push(parts[i].clone());
            i += 1;
        }
    }

    (args, stdout_file, stderr_file)
}

fn open_file(file: &str, append: bool) -> File {
    OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(file)
        .unwrap()
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
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();

    let mut rl = Editor::with_config(config).unwrap();
    rl.set_helper(Some(ShellHelper::new()));

    loop {
        let readline = rl.readline("$ ");
        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                let parts = parse_args(input);
                if parts.is_empty() {
                    continue;
                }

                let (parts, stdout_redirect, stderr_redirect) = extract_redirect(&parts);
                if parts.is_empty() {
                    continue;
                }

                if let Some((ref file, append)) = stderr_redirect {
                    open_file(file, append);
                }

                let command = &parts[0];
                let args = &parts[1..];

                if command == "exit" {
                    break;
                } else if command == "echo" {
                    let output = args.join(" ");
                    if let Some((ref file, append)) = stdout_redirect {
                        let mut f = open_file(file, append);
                        writeln!(f, "{}", output).unwrap();
                    } else {
                        println!("{}", output);
                    }
                } else if command == "type" {
                    if let Some(arg) = args.first() {
                        let result = if BUILTINS.contains(&arg.as_str()) {
                            format!("{} is a shell builtin", arg)
                        } else if let Some(path) = find_in_path(arg) {
                            format!("{} is {}", arg, path)
                        } else {
                            format!("{}: not found", arg)
                        };
                        if let Some((ref file, append)) = stdout_redirect {
                            let mut f = open_file(file, append);
                            writeln!(f, "{}", result).unwrap();
                        } else {
                            println!("{}", result);
                        }
                    }
                } else if command == "pwd" {
                    let cwd = env::current_dir().unwrap();
                    let output = cwd.display().to_string();
                    if let Some((ref file, append)) = stdout_redirect {
                        let mut f = open_file(file, append);
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
                    let mut cmd = Command::new(command);
                    cmd.args(args);

                    if let Some((ref file, append)) = stdout_redirect {
                        cmd.stdout(Stdio::from(open_file(file, append)));
                    }
                    if let Some((ref file, append)) = stderr_redirect {
                        cmd.stderr(Stdio::from(open_file(file, append)));
                    }

                    cmd.status().unwrap();
                } else {
                    println!("{}: command not found", command);
                }
            }
            Err(ReadlineError::Eof) => break,
            Err(_) => break,
        }
    }
}