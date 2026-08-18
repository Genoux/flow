//! `FLOW_DEBUG=1` - the detail that only helps while chasing something.
//!
//! Flow runs as a user unit with no terminal, so every line it prints lands in
//! the journal and stays there. The daemon's normal output is worth keeping:
//! one line per dictation plus the raw and cleaned text is exactly what you
//! want when asking "what did it hear". Device enumeration and chord-release
//! internals are not - they answer a question nobody has until the binding
//! misbehaves, and by then they are 200 dictations back.
//!
//! One env check rather than a log framework: a subscriber, an appender and a
//! rotation policy are a lot of machinery to answer "print more".

use std::sync::OnceLock;

/// Whether `FLOW_DEBUG` asks for the chatty output.
///
/// Read once, because this is checked inside the chord poll loop and an env
/// lookup there would be a syscall every few milliseconds. `0` and the empty
/// string count as off, so `FLOW_DEBUG=0` reads the way anyone would expect
/// rather than being true for being present.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| asked_for(std::env::var_os("FLOW_DEBUG").as_deref()))
}

/// Split out from [`enabled`] so it is testable: `enabled` caches a
/// process-wide value, so a test can only ever observe one of these answers.
fn asked_for(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// `eprintln!` that only prints under `FLOW_DEBUG`.
#[macro_export]
macro_rules! verbose {
    ($($arg:tt)*) => {
        if $crate::debug::enabled() {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::asked_for;
    use std::ffi::OsStr;

    #[test]
    fn only_a_meaningful_value_turns_debug_on() {
        assert!(asked_for(Some(OsStr::new("1"))));
        assert!(asked_for(Some(OsStr::new("true"))));
        assert!(!asked_for(None));
        assert!(!asked_for(Some(OsStr::new("0"))));
        assert!(!asked_for(Some(OsStr::new(""))));
    }
}
