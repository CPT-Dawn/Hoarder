//! `hoarder pause [DURATION]`, `hoarder resume` and `hoarder status`: the
//! Daemon's recording state, through its socket (PLAN.md §5.1, §10).

use hoarder_ipc::{Recording, State, Status};

use crate::Exit;
use crate::history::{answer, connect, print_json};

type Fail = (Exit, String);

/// Seconds in a DURATION: a number with an optional unit (`s`, `m`, `h`,
/// `d`; seconds without one), or several run together, as in `1h30m`.
pub fn parse_duration(arg: &str) -> Result<u64, String> {
    let bad = || format!("{arg:?} is not a duration like 90, 30s, 5m, 2h or 1h30m");
    let mut total: u64 = 0;
    let mut rest = arg.trim();
    if rest.is_empty() {
        return Err(bad());
    }
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let n: u64 = rest[..digits].parse().map_err(|_| bad())?;
        rest = &rest[digits..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let unit = match &rest[..unit_len] {
            "" | "s" => 1,
            "m" => 60,
            "h" => 60 * 60,
            "d" => 24 * 60 * 60,
            _ => return Err(bad()),
        };
        rest = &rest[unit_len..];
        total = n
            .checked_mul(unit)
            .and_then(|s| total.checked_add(s))
            .ok_or_else(bad)?;
    }
    Ok(total)
}

/// `hoarder pause [DURATION]`.
pub fn pause(secs: Option<u64>, json: bool) -> Result<(), Fail> {
    let mut daemon = connect("it is what records")?;
    let recording = answer(daemon.pause(secs))?;
    report(&recording, json)
}

/// `hoarder resume`.
pub fn resume(json: bool) -> Result<(), Fail> {
    let mut daemon = connect("it is what records")?;
    let recording = answer(daemon.resume())?;
    report(&recording, json)
}

/// `hoarder status [--json]`.
pub fn status(json: bool) -> Result<(), Fail> {
    let mut daemon = connect("only it knows its state")?;
    let status: Status = answer(daemon.status())?;
    if json {
        return print_json(&status);
    }
    println!("daemon:     {} (pid {})", status.daemon, status.pid);
    println!("recording:  {}", describe(&status.recording));
    println!("persist:    {}", if status.persist { "on" } else { "off" });
    let owner = serde_json::to_value(status.clipboard.owner).unwrap_or_default();
    println!("clipboard:  owned by {}", owner.as_str().unwrap_or("?"));
    Ok(())
}

fn report(recording: &Recording, json: bool) -> Result<(), Fail> {
    if json {
        print_json(recording)
    } else {
        println!("{}", describe(recording));
        Ok(())
    }
}

fn describe(recording: &Recording) -> String {
    match (recording.state, &recording.until) {
        (State::Active, _) => "active".into(),
        (State::Paused, Some(until)) => format!("paused until {until}"),
        (State::Paused, None) => "paused, until `hoarder resume`".into(),
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn durations() {
        for (arg, secs) in [
            ("90", 90),
            ("30s", 30),
            ("5m", 300),
            ("2h", 7200),
            ("1d", 86400),
            ("1h30m", 5400),
        ] {
            assert_eq!(parse_duration(arg), Ok(secs), "{arg}");
        }
        for bad in ["", "soon", "5x", "m", "-5", "1.5h"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }
}
