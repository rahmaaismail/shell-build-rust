#[allow(unused_imports)]
use std::io::Write;
use std::env;
use std::path::Path;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::fs::{File, OpenOptions};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, Editor};
use rustyline_derive::Helper;

extern crate libc;

const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd", "complete", "jobs", "history", "declare"];

fn longest_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() { return String::new(); }
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

fn reap_jobs(bg_jobs: &mut Vec<(usize, std::process::Child, String)>) {
    let mut done = Vec::new();
    for (i, (_, child, _)) in bg_jobs.iter_mut().enumerate() {
        if let Ok(Some(_)) = child.try_wait() { done.push(i); }
    }
    let total = bg_jobs.len();
    for &i in &done {
        let (job_num, _, cmd_str) = &bg_jobs[i];
        let marker = if i + 1 == total { "+" } else if total >= 2 && i + 1 == total - 1 { "-" } else { " " };
        println!("[{}]{}  Done                    {}", job_num, marker, cmd_str);
    }
    for i in done.into_iter().rev() { bg_jobs.remove(i); }
}

fn split_pipeline(input: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;

    for c in input.chars() {
        match c {
            '\'' if !in_double => { in_single = !in_single; current.push(c); }
            '"' if !in_single => { in_double = !in_double; current.push(c); }
            '|' if !in_single && !in_double => {
                segments.push(current.trim().to_string());
                current = String::new();
            }
            _ => { current.push(c); }
        }
    }
    segments.push(current.trim().to_string());
    segments
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

fn run_pipeline(segments: &[String]) {
    if segments.is_empty() { return; }

    let mut pipes: Vec<(i32, i32)> = Vec::new();
    for _ in 0..segments.len() - 1 {
        let mut fds = [0i32; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()); }
        pipes.push((fds[0], fds[1]));
    }

    let mut pids: Vec<libc::pid_t> = Vec::new();

    for (i, seg) in segments.iter().enumerate() {
        let parts = parse_args(seg);
        if parts.is_empty() { continue; }
        let command = parts[0].clone();
        let args = parts[1..].to_vec();

        let is_last = i == segments.len() - 1;
        let stdin_fd = if i > 0 { pipes[i-1].0 } else { -1 };
        let stdout_fd = if !is_last { pipes[i].1 } else { -1 };

        let pid = unsafe { libc::fork() };
        if pid == 0 {
            if stdin_fd != -1 { unsafe { libc::dup2(stdin_fd, 0); } }
            if stdout_fd != -1 { unsafe { libc::dup2(stdout_fd, 1); } }
            for &(r, w) in &pipes {
                unsafe { libc::close(r); libc::close(w); }
            }
            if !is_last {
                let dev_null = std::ffi::CString::new("/dev/null").unwrap();
                let null_fd = unsafe { libc::open(dev_null.as_ptr(), libc::O_WRONLY) };
                if null_fd >= 0 { unsafe { libc::dup2(null_fd, 2); libc::close(null_fd); } }
            }

            if BUILTINS.contains(&command.as_str()) {
                let output = match command.as_str() {
                    "echo" => format!("{}\n", args.join(" ")),
                    "pwd" => format!("{}\n", env::current_dir().unwrap().display()),
                    "type" => {
                        if let Some(arg) = args.first() {
                            if BUILTINS.contains(&arg.as_str()) {
                                format!("{} is a shell builtin\n", arg)
                            } else if let Some(path) = find_in_path(arg) {
                                format!("{} is {}\n", arg, path)
                            } else {
                                format!("{}: not found\n", arg)
                            }
                        } else { String::new() }
                    }
                    _ => String::new(),
                };
                if !output.is_empty() {
                    print!("{}", output);
                    std::io::stdout().flush().unwrap();
                }
                unsafe { libc::_exit(0); }
            } else {
                if let Some(path) = find_in_path(&command) {
                    let c_path = std::ffi::CString::new(path).unwrap();
                    let mut c_args: Vec<std::ffi::CString> = vec![
                        std::ffi::CString::new(command.as_str()).unwrap()
                    ];
                    for arg in &args {
                        c_args.push(std::ffi::CString::new(arg.as_str()).unwrap());
                    }
                    let mut c_ptrs: Vec<*const libc::c_char> = c_args.iter().map(|s| s.as_ptr()).collect();
                    c_ptrs.push(std::ptr::null());
                    unsafe { libc::execv(c_path.as_ptr(), c_ptrs.as_ptr()); }
                } else {
                    eprintln!("{}: command not found", command);
                }
                unsafe { libc::_exit(127); }
            }
        } else {
            pids.push(pid);
        }
    }

    for (r, w) in pipes {
        unsafe { libc::close(r); libc::close(w); }
    }

    for pid in pids {
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0); }
    }
}

#[derive(Helper)]
struct ShellHelper {
    last_prefix: RefCell<String>,
    tab_count: RefCell<usize>,
    completions: Rc<RefCell<HashMap<String, String>>>,
}

impl ShellHelper {
    fn new(completions: Rc<RefCell<HashMap<String, String>>>) -> Self {
        ShellHelper {
            last_prefix: RefCell::new(String::new()),
            tab_count: RefCell::new(0),
            completions,
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

        if prefix.contains(' ') {
            let parts: Vec<&str> = prefix.splitn(2, ' ').collect();
            let cmd_name = parts[0];
            let arg_prefix = if prefix.ends_with(' ') { "" } else {
                prefix.split(' ').last().unwrap_or("")
            };

            let completer_path = self.completions.borrow().get(cmd_name).cloned();
            if let Some(script_path) = completer_path {
                let all_parts: Vec<&str> = prefix.split_whitespace().collect();
                let current_word = if prefix.ends_with(' ') { "" } else {
                    all_parts.last().copied().unwrap_or("")
                };
                let prev_word = if prefix.ends_with(' ') {
                    all_parts.last().copied().unwrap_or("")
                } else if all_parts.len() >= 2 {
                    all_parts[all_parts.len() - 2]
                } else {
                    ""
                };

                let output = Command::new(&script_path)
                    .arg(cmd_name)
                    .arg(current_word)
                    .arg(prev_word)
                    .env("COMP_LINE", prefix)
                    .env("COMP_POINT", prefix.len().to_string())
                    .output();

                if let Ok(output) = output {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let mut candidates: Vec<&str> = stdout.lines().collect();
                    candidates.sort();

                    if candidates.len() == 1 {
                        let before_arg = &prefix[..prefix.len() - current_word.len()];
                        return Ok((0, vec![Pair {
                            display: candidates[0].to_string(),
                            replacement: format!("{}{} ", before_arg, candidates[0]),
                        }]));
                    }

                    if candidates.len() > 1 {
                        let candidate_strings: Vec<String> = candidates.iter().map(|s| s.to_string()).collect();
                        let lcp = longest_common_prefix(&candidate_strings);

                        if lcp.len() > current_word.len() {
                            let before_arg = &prefix[..prefix.len() - current_word.len()];
                            *self.last_prefix.borrow_mut() = String::new();
                            *self.tab_count.borrow_mut() = 0;
                            return Ok((0, vec![Pair {
                                display: lcp.clone(),
                                replacement: format!("{}{}", before_arg, lcp),
                            }]));
                        }

                        let current_prefix = prefix.to_string();
                        let is_same_prefix = *self.last_prefix.borrow() == current_prefix;
                        if is_same_prefix { *self.tab_count.borrow_mut() += 1; }
                        else { *self.last_prefix.borrow_mut() = current_prefix; *self.tab_count.borrow_mut() = 1; }
                        let count = *self.tab_count.borrow();
                        if count == 1 {
                            print!("\x07"); std::io::stdout().flush().unwrap();
                            return Ok((0, vec![]));
                        } else {
                            *self.tab_count.borrow_mut() = 0;
                            println!(); println!("{}", candidates.join("  "));
                            print!("$ {}", prefix); std::io::stdout().flush().unwrap();
                            return Ok((0, vec![]));
                        }
                    }
                }
                return Ok((0, vec![]));
            }

            let file_prefix = arg_prefix;
            let mut file_matches: Vec<(String, bool)> = Vec::new();

            let (dir, name_prefix) = if let Some(slash_pos) = file_prefix.rfind('/') {
                (&file_prefix[..slash_pos + 1], &file_prefix[slash_pos + 1..])
            } else {
                ("", file_prefix)
            };

            let read_dir_path = if dir.is_empty() { "." } else { dir };

            if let Ok(entries) = std::fs::read_dir(read_dir_path) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(name_prefix) {
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        file_matches.push((format!("{}{}", dir, name), is_dir));
                    }
                }
            }

            file_matches.sort_by(|a, b| a.0.cmp(&b.0));

            if file_matches.is_empty() { return Ok((0, vec![])); }

            if file_matches.len() == 1 {
                let (matched_path, is_dir) = &file_matches[0];
                let cmd_and_space = &prefix[..prefix.len() - file_prefix.len()];
                let suffix = if *is_dir { "/" } else { " " };
                *self.last_prefix.borrow_mut() = String::new();
                *self.tab_count.borrow_mut() = 0;
                return Ok((0, vec![Pair {
                    display: matched_path.clone(),
                    replacement: format!("{}{}{}", cmd_and_space, matched_path, suffix),
                }]));
            }

            let names: Vec<String> = file_matches.iter().map(|(n, _)| n.clone()).collect();
            let lcp = longest_common_prefix(&names);

            if lcp.len() > file_prefix.len() {
                let cmd_and_space = &prefix[..prefix.len() - file_prefix.len()];
                *self.last_prefix.borrow_mut() = String::new();
                *self.tab_count.borrow_mut() = 0;
                return Ok((0, vec![Pair {
                    display: lcp.clone(),
                    replacement: format!("{}{}", cmd_and_space, lcp),
                }]));
            }

            let current_prefix = prefix.to_string();
            let is_same_prefix = *self.last_prefix.borrow() == current_prefix;
            if is_same_prefix { *self.tab_count.borrow_mut() += 1; }
            else { *self.last_prefix.borrow_mut() = current_prefix; *self.tab_count.borrow_mut() = 1; }
            let count = *self.tab_count.borrow();
            if count == 1 {
                print!("\x07"); std::io::stdout().flush().unwrap();
                return Ok((0, vec![]));
            } else {
                *self.tab_count.borrow_mut() = 0;
                println!();
                let display: Vec<String> = file_matches.iter()
                    .map(|(name, is_dir)| if *is_dir { format!("{}/", name) } else { name.clone() })
                    .collect();
                println!("{}", display.join("  "));
                print!("$ {}", prefix); std::io::stdout().flush().unwrap();
                return Ok((0, vec![]));
            }
        }

        let matches = self.get_matches(prefix);
        if matches.is_empty() { return Ok((0, vec![])); }

        if matches.len() == 1 {
            *self.last_prefix.borrow_mut() = String::new();
            *self.tab_count.borrow_mut() = 0;
            return Ok((0, vec![Pair { display: matches[0].clone(), replacement: format!("{} ", matches[0]) }]));
        }

        let lcp = longest_common_prefix(&matches);
        if lcp.len() > prefix.len() {
            *self.last_prefix.borrow_mut() = String::new();
            *self.tab_count.borrow_mut() = 0;
            return Ok((0, vec![Pair { display: lcp.clone(), replacement: lcp }]));
        }

        let current_prefix = prefix.to_string();
        let is_same_prefix = *self.last_prefix.borrow() == current_prefix;
        if is_same_prefix { *self.tab_count.borrow_mut() += 1; }
        else { *self.last_prefix.borrow_mut() = current_prefix; *self.tab_count.borrow_mut() = 1; }
        let count = *self.tab_count.borrow();
        if count == 1 {
            print!("\x07"); std::io::stdout().flush().unwrap();
            return Ok((0, vec![]));
        } else {
            *self.tab_count.borrow_mut() = 0;
            println!(); println!("{}", matches.join("  "));
            print!("$ {}", prefix); std::io::stdout().flush().unwrap();
            return Ok((0, vec![]));
        }
    }
}

impl Hinter for ShellHelper { type Hint = String; }
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
                if let Some(next) = chars.next() { current.push(next); }
            }
            '\\' if in_double_quote => {
                if let Some(next) = chars.next() {
                    if next == '"' || next == '\\' { current.push(next); }
                    else { current.push('\\'); current.push(next); }
                }
            }
            '\'' if !in_single_quote && !in_double_quote => { in_single_quote = true; }
            '\'' if in_single_quote => { in_single_quote = false; }
            '"' if !in_single_quote && !in_double_quote => { in_double_quote = true; }
            '"' if in_double_quote => { in_double_quote = false; }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() { args.push(current.clone()); current.clear(); }
            }
            _ => { current.push(c); }
        }
    }
    if !current.is_empty() { args.push(current); }
    args
}

fn extract_redirect(parts: &[String]) -> (Vec<String>, Option<(String, bool)>, Option<(String, bool)>) {
    let mut args = Vec::new();
    let mut stdout_file = None;
    let mut stderr_file = None;
    let mut i = 0;

    while i < parts.len() {
        if parts[i] == ">>" || parts[i] == "1>>" {
            if i + 1 < parts.len() { stdout_file = Some((parts[i+1].clone(), true)); i += 2; }
        } else if parts[i] == ">" || parts[i] == "1>" {
            if i + 1 < parts.len() { stdout_file = Some((parts[i+1].clone(), false)); i += 2; }
        } else if parts[i] == "2>>" {
            if i + 1 < parts.len() { stderr_file = Some((parts[i+1].clone(), true)); i += 2; }
        } else if parts[i] == "2>" {
            if i + 1 < parts.len() { stderr_file = Some((parts[i+1].clone(), false)); i += 2; }
        } else {
            args.push(parts[i].clone()); i += 1;
        }
    }
    (args, stdout_file, stderr_file)
}

fn open_file(file: &str, append: bool) -> File {
    OpenOptions::new().write(true).create(true).append(append).truncate(!append).open(file).unwrap()
}

fn save_history(history: &[String], histfile: &str) {
    if let Ok(mut f) = File::create(histfile) {
        for cmd in history {
            let _ = writeln!(f, "{}", cmd);
        }
    }
}

fn expand_vars(s: &str, shell_vars: &HashMap<String, String>) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            if chars.peek() == Some(&'{') {
                // ${VAR} form
                chars.next(); // consume '{'
                let mut name = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc == '}' {
                        chars.next(); // consume '}'
                        break;
                    }
                    name.push(nc);
                    chars.next();
                }
                if let Some(val) = shell_vars.get(&name) {
                    result.push_str(val);
                } else if let Ok(val) = env::var(&name) {
                    result.push_str(&val);
                }
                // if not found, expands to empty string
            } else {
                // $VAR form
                let mut name = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc.is_alphanumeric() || nc == '_' {
                        name.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    result.push('$');
                } else if let Some(val) = shell_vars.get(&name) {
                    result.push_str(val);
                } else if let Ok(val) = env::var(&name) {
                    result.push_str(&val);
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn main() {
    let config = Config::builder().completion_type(CompletionType::List).build();
    let completions: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
    let mut rl = Editor::with_config(config).unwrap();
    rl.set_helper(Some(ShellHelper::new(Rc::clone(&completions))));
    let mut bg_jobs: Vec<(usize, std::process::Child, String)> = Vec::new();
    let mut history: Vec<String> = Vec::new();
    let mut history_saved_count: usize = 0;
    let mut shell_vars: HashMap<String, String> = HashMap::new();

    // Load history from HISTFILE on startup
    if let Ok(histfile) = env::var("HISTFILE") {
        if let Ok(contents) = std::fs::read_to_string(&histfile) {
            for line in contents.lines() {
                if !line.is_empty() {
                    history.push(line.to_string());
                    let _ = rl.add_history_entry(line);
                }
            }
            history_saved_count = history.len();
        }
    }

    loop {
        reap_jobs(&mut bg_jobs);

        let readline = rl.readline("$ ");
        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() { continue; }

                history.push(input.to_string());
                let _ = rl.add_history_entry(input);

                let segments = split_pipeline(input);
                if segments.len() > 1 {
                    run_pipeline(&segments);
                    continue;
                }

                let parts = parse_args(input);
                if parts.is_empty() { continue; }

                let background = parts.last().map(|s| s.as_str()) == Some("&");
                let parts: Vec<String> = if background { parts[..parts.len()-1].to_vec() } else { parts };
                if parts.is_empty() { continue; }

                let (parts, stdout_redirect, stderr_redirect) = extract_redirect(&parts);
                if parts.is_empty() { continue; }

                if let Some((ref file, append)) = stderr_redirect { open_file(file, append); }

                let command = &parts[0];
                let args = &parts[1..];

                // expand variables in command and args
                let command = expand_vars(command, &shell_vars);
                let command = command.as_str();
                let expanded_args: Vec<String> = args.iter().map(|a| expand_vars(a, &shell_vars)).collect();
                let args = expanded_args.as_slice();

                if command == "exit" {
                    if let Ok(histfile) = env::var("HISTFILE") {
                        save_history(&history, &histfile);
                    }
                    break;
                } else if command == "echo" {
                    let output = args.join(" ");
                    if let Some((ref file, append)) = stdout_redirect {
                        let mut f = open_file(file, append); writeln!(f, "{}", output).unwrap();
                    } else { println!("{}", output); }
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
                            let mut f = open_file(file, append); writeln!(f, "{}", result).unwrap();
                        } else { println!("{}", result); }
                    }
                } else if command == "pwd" {
                    let cwd = env::current_dir().unwrap();
                    let output = cwd.display().to_string();
                    if let Some((ref file, append)) = stdout_redirect {
                        let mut f = open_file(file, append); writeln!(f, "{}", output).unwrap();
                    } else { println!("{}", output); }
                } else if command == "cd" {
                    if let Some(dir) = args.first() {
                        let target = if dir == "~" { env::var("HOME").unwrap_or_default() } else { dir.to_string() };
                        let path = Path::new(&target);
                        if path.exists() { env::set_current_dir(path).unwrap(); }
                        else { println!("cd: {}: No such file or directory", dir); }
                    }
                } else if command == "jobs" {
                    let mut done = Vec::new();
                    for (i, (_, child, _)) in bg_jobs.iter_mut().enumerate() {
                        if let Ok(Some(_)) = child.try_wait() { done.push(i); }
                    }
                    let total = bg_jobs.len();
                    for (i, (job_num, _, cmd_str)) in bg_jobs.iter().enumerate() {
                        let marker = if i+1==total {"+"} else if total>=2&&i+1==total-1 {"-"} else {" "};
                        if done.contains(&i) { println!("[{}]{}  Done                    {}", job_num, marker, cmd_str); }
                        else { println!("[{}]{}  Running                 {} &", job_num, marker, cmd_str); }
                    }
                    for i in done.into_iter().rev() { bg_jobs.remove(i); }
                } else if command == "history" {
                    if args.first().map(|s| s.as_str()) == Some("-r") {
                        if let Some(path) = args.get(1) {
                            if let Ok(contents) = std::fs::read_to_string(path) {
                                for line in contents.lines() {
                                    if !line.is_empty() {
                                        history.push(line.to_string());
                                        let _ = rl.add_history_entry(line);
                                    }
                                }
                            }
                        }
                    } else if args.first().map(|s| s.as_str()) == Some("-w") {
                        if let Some(path) = args.get(1) {
                            let mut f = File::create(path).unwrap();
                            for cmd in &history {
                                writeln!(f, "{}", cmd).unwrap();
                            }
                        }
                    } else if args.first().map(|s| s.as_str()) == Some("-a") {
                        if let Some(path) = args.get(1) {
                            let mut f = OpenOptions::new().create(true).append(true).open(path).unwrap();
                            for cmd in &history[history_saved_count..] {
                                writeln!(f, "{}", cmd).unwrap();
                            }
                            history_saved_count = history.len();
                        }
                    } else {
                        let total = history.len();
                        let start = if let Some(n_str) = args.first() {
                            if let Ok(n) = n_str.parse::<usize>() {
                                total.saturating_sub(n)
                            } else { 0 }
                        } else { 0 };
                        for (i, cmd) in history[start..].iter().enumerate() {
                            println!("{:>4}  {}", start + i + 1, cmd);
                        }
                    }
                } else if command == "complete" {
                    if args.first().map(|s| s.as_str()) == Some("-p") {
                        if let Some(cmd_name) = args.get(1) {
                            if let Some(path) = completions.borrow().get(cmd_name.as_str()) {
                                println!("complete -C '{}' {}", path, cmd_name);
                            } else { println!("complete: {}: no completion specification", cmd_name); }
                        }
                    } else if args.first().map(|s| s.as_str()) == Some("-C") {
                        if let (Some(path), Some(cmd_name)) = (args.get(1), args.get(2)) {
                            completions.borrow_mut().insert(cmd_name.clone(), path.clone());
                        }
                    } else if args.first().map(|s| s.as_str()) == Some("-r") {
                        if let Some(cmd_name) = args.get(1) { completions.borrow_mut().remove(cmd_name.as_str()); }
                    }
                } else if command == "declare" {
                    if args.first().map(|s| s.as_str()) == Some("-p") {
                        if let Some(var_name) = args.get(1) {
                            if let Some(value) = shell_vars.get(var_name.as_str()) {
                                println!("declare -- {}=\"{}\"", var_name, value);
                            } else {
                                eprintln!("declare: {}: not found", var_name);
                            }
                        }
                    } else {
                        for arg in args {
                            if let Some(eq_pos) = arg.find('=') {
                                let name = &arg[..eq_pos];
                                let valid = {
                                    let mut chars = name.chars();
                                    chars.next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
                                        && chars.all(|c| c.is_alphanumeric() || c == '_')
                                };
                                if valid {
                                    let value = &arg[eq_pos + 1..];
                                    shell_vars.insert(name.to_string(), value.to_string());
                                } else {
                                    eprintln!("declare: `{}': not a valid identifier", arg);
                                }
                            }
                        }
                    }
                } else if let Some(_path) = find_in_path(command) {
                    let mut cmd = Command::new(command);
                    cmd.args(args);
                    if let Some((ref file, append)) = stdout_redirect { cmd.stdout(Stdio::from(open_file(file, append))); }
                    if let Some((ref file, append)) = stderr_redirect { cmd.stderr(Stdio::from(open_file(file, append))); }
                    if background {
                        let child = cmd.spawn().unwrap();
                        let pid = child.id();
                        let job_num = {
                            let used: std::collections::HashSet<usize> = bg_jobs.iter().map(|(n,_,_)| *n).collect();
                            (1..).find(|n| !used.contains(n)).unwrap()
                        };
                        let cmd_str = format!("{} {}", command, args.join(" ")).trim().to_string();
                        println!("[{}] {}", job_num, pid);
                        bg_jobs.push((job_num, child, cmd_str));
                    } else { cmd.status().unwrap(); }
                } else {
                    println!("{}: command not found", command);
                }
            }
            Err(ReadlineError::Eof) => {
                if let Ok(histfile) = env::var("HISTFILE") {
                    save_history(&history, &histfile);
                }
                break;
            }
            Err(_) => break,
        }
    }
}