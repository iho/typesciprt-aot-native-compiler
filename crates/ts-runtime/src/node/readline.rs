//! Node.js `readline` module — synchronous stdin reading.

use crate::value::TsVal;
use super::new_string;
use std::io::{self, BufRead, Write};

/// Print a prompt and read a line from stdin synchronously.
/// ts_readline_question(prompt) -> string
#[no_mangle]
pub unsafe extern "C" fn ts_readline_question(prompt_val: TsVal) -> TsVal {
    if let Some(s) = super::val_to_string(prompt_val) {
        print!("{}", s);
        let _ = io::stdout().flush();
    }
    let mut line = String::new();
    let stdin = io::stdin();
    let _ = stdin.lock().read_line(&mut line);
    // Strip the trailing newline.
    if line.ends_with('\n') { line.pop(); }
    if line.ends_with('\r') { line.pop(); }
    new_string(&line)
}

/// Read a single line from stdin (no prompt). Returns empty string on EOF.
/// ts_readline_read_line() -> string
#[no_mangle]
pub unsafe extern "C" fn ts_readline_read_line() -> TsVal {
    let mut line = String::new();
    let stdin = io::stdin();
    match stdin.lock().read_line(&mut line) {
        Ok(0) => new_string(""),   // EOF
        Ok(_) => {
            if line.ends_with('\n') { line.pop(); }
            if line.ends_with('\r') { line.pop(); }
            new_string(&line)
        }
        Err(_) => new_string(""),
    }
}
