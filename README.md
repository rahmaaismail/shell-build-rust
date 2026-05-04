# Shell in Rust

A POSIX-ish shell implementation written in Rust, built by following the [CodeCrafters](https://codecrafters.io) "Build Your Own Shell" challenge. This was my first time writing Rust, so it was a great way to learn the language while building something real.

I completed all stages and extensions of the challenge, which put me in the top ~20% of users who attempt it.

## Stages Completed

**Base**
- Print a prompt
- Handle invalid commands
- Implement a REPL
- exit, echo, type builtins
- Locate and run executable files

**Extensions**
- Navigation — `pwd`, `cd` (absolute, relative, and `~`)
- Quoting — single quotes, double quotes, backslash escaping
- Redirection — stdout/stderr redirect and append (`>`, `>>`, `2>`, `2>>`)
- Command Completion — builtin and executable tab completion
- Filename Completion — file, directory, nested, and partial completions
- Programmable Completion — `complete -C` with argument and environment passing
- Background Jobs — `&`, `jobs`, job numbering, reaping
- Pipelines — multi-command pipelines with builtins
- History — `history` builtin, up/down arrow navigation, file persistence (`HISTFILE`)
- Parameter Expansion — `declare`, `$VAR`, `${VAR}`, empty variable handling

## Running It

Make sure you have [Rust installed](https://rustup.rs), then:

```bash
cargo build
cargo run
```

To use history persistence across sessions, set `HISTFILE` before running:

```bash
HISTFILE=~/.my_shell_history cargo run
```

## What I Learned

Coming into this with no prior Rust experience, I picked up a lot along the way — ownership and borrowing, iterators, pattern matching, `RefCell`/`Rc` for shared mutable state, unsafe FFI with `libc` for `fork`/`exec`/`pipe`, and working with the `rustyline` crate for readline-style input. Building a real project from scratch turned out to be a much better way to learn than just reading docs.