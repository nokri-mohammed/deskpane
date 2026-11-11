//! A small blocking client for pisugar-server's text protocol on its Unix
//! socket (https://docs.pisugar.com, "PiSugar Power Manager"): one command
//! per line, one reply line `name: value` back. The socket has no auth and
//! also takes commands that change the board, so this client can only send
//! the three fixed lines in `Query`; no input can build another command.
//! Every failure (no socket, no answer in time, a reply that is too long,
//! malformed or out of range) returns None; nothing here panics.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// where deskpane's pisugar-server setup puts the socket (docs/INSTALL.md;
// pisugar-server's own default is /tmp, which a PrivateTmp unit cannot see),
// and the variable that moves it
pub const DEFAULT_SOCKET_PATH: &str = "/run/pisugar/pisugar-server.sock";
pub const SOCKET_PATH_ENV: &str = "PISUGAR_SOCKET";
// one query (connect, send, read the reply) must finish within this
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(300);
// most bytes read for one reply; real replies are about 30 bytes
pub const MAX_REPLY_BYTES: usize = 128;
// a percent this far outside 0 to 100 is clamped (the PiSugar 2 estimates
// from the voltage and can overshoot a little); further out is junk
const PERCENT_TOLERANCE: f64 = 5.0;
// bytes read per call
const CHUNK_BYTES: usize = 64;

/// The only commands this client can send, each a fixed line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Query {
    // the charge in percent
    Battery,
    // on external power, PiSugar 2 family (deprecated on PiSugar 3)
    BatteryCharging,
    // on external power, PiSugar 3
    BatteryPowerPlugged,
}

impl Query {
    // the exact line sent, newline included
    pub fn line(&self) -> &'static str {
        match self {
            Query::Battery => "get battery\n",
            Query::BatteryCharging => "get battery_charging\n",
            Query::BatteryPowerPlugged => "get battery_power_plugged\n",
        }
    }

    // the name the reply line starts with
    pub fn reply_name(&self) -> &'static str {
        match self {
            Query::Battery => "battery",
            Query::BatteryCharging => "battery_charging",
            Query::BatteryPowerPlugged => "battery_power_plugged",
        }
    }
}

/// Asks one pisugar-server socket for values, one connection per query.
pub struct PiSugarClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl PiSugarClient {
    /*
     * a client for the socket at socket_path; nothing is opened yet
     *
     * socket_path: e.g. /tmp/pisugar-server.sock
     * timeout: the whole budget of one query, e.g. QUERY_TIMEOUT
     */
    pub fn new(socket_path: &Path, timeout: Duration) -> Self {
        PiSugarClient {
            socket_path: socket_path.to_path_buf(),
            timeout: timeout,
        }
    }

    // the socket this client asks, for log lines
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    // the deadline of a budget that starts now: share it across the queries of one cycle
    pub fn deadline(&self) -> Instant {
        Instant::now() + self.timeout
    }

    /*
     * sends one fixed query and returns the value of its reply `<name>: <value>`
     *
     * query: which value, e.g. Query::Battery
     * deadline: when this query must be over, from deadline(); one deadline is
     *           shared by every query of a report cycle, and once it has passed
     *           nothing is sent
     * returns: the value text, trimmed, or None on any failure
     */
    pub fn ask(&self, query: Query, deadline: Instant) -> Option<String> {
        // a budget already spent (a wedged server earlier this cycle) sends nothing
        let remaining = time_left(deadline);
        match remaining {
            Some(_) => {}
            None => return None,
        }

        /* a missing socket or a stopped server fails here. connect() has no
         * timeout (review LOW 4, left as is): on a Unix socket it does not
         * wait for the server to accept, it only queues in the kernel's
         * backlog, and a full backlog fails at once (EAGAIN) */
        let connected = UnixStream::connect(&self.socket_path);
        let mut stream = match connected {
            Ok(s) => s,
            Err(_) => return None,
        };

        // send the command within the budget
        let remaining = time_left(deadline);
        let remaining = match remaining {
            Some(d) => d,
            None => return None,
        };
        let write_timeout_set = stream.set_write_timeout(Some(remaining));
        match write_timeout_set {
            Ok(()) => {}
            Err(_) => return None,
        }
        let written = stream.write_all(query.line().as_bytes());
        match written {
            Ok(()) => {}
            Err(_) => return None,
        }

        read_reply(&mut stream, query.reply_name(), deadline)
    }

    // the charge in percent, 0 to 100, within deadline (see ask)
    pub fn battery_pct(&self, deadline: Instant) -> Option<f64> {
        let value = self.ask(Query::Battery, deadline);
        match value {
            Some(text) => parse_percent(&text),
            None => None,
        }
    }

    /*
     * true while the board is on external power, from one of the two plug
     * queries (BatteryPowerPlugged on a PiSugar 3, BatteryCharging before)
     *
     * query: Query::BatteryPowerPlugged or Query::BatteryCharging
     * deadline: as for ask
     * returns: None on any failure, and for Query::Battery
     */
    pub fn plugged(&self, query: Query, deadline: Instant) -> Option<bool> {
        if query == Query::Battery {
            return None;
        }
        let value = self.ask(query, deadline);
        match value {
            Some(text) => parse_bool(&text),
            None => None,
        }
    }
}

/*
 * the time left until deadline, or None once it has passed (a zero
 * timeout would mean "wait forever" to the socket calls)
 */
fn time_left(deadline: Instant) -> Option<Duration> {
    let now = Instant::now();
    if now >= deadline {
        return None;
    }
    Some(deadline - now)
}

/*
 * reads lines until one is the reply for name, the budget runs out, or
 * more than MAX_REPLY_BYTES arrived. Other lines (pisugar-server also
 * writes button events such as "single" to its clients) are skipped.
 *
 * stream: the connected socket, the command already sent
 * name: the value name asked for
 * deadline: when the whole query must be over
 */
fn read_reply(stream: &mut UnixStream, name: &str, deadline: Instant) -> Option<String> {
    let mut pending: Vec<u8> = Vec::new();
    let mut total = 0;
    let mut chunk = [0u8; CHUNK_BYTES];

    loop {
        let remaining = time_left(deadline);
        let remaining = match remaining {
            Some(d) => d,
            None => return None,
        };
        let read_timeout_set = stream.set_read_timeout(Some(remaining));
        match read_timeout_set {
            Ok(()) => {}
            Err(_) => return None,
        }

        let read = stream.read(&mut chunk);
        let len = match read {
            Ok(n) => n,
            Err(e) => {
                // a signal arrived mid-read: try again within the budget
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                // timed out or broken
                return None;
            }
        };

        // the server closed: what is left may be a last line without a newline
        if len == 0 {
            return parse_reply_bytes(name, &pending);
        }

        total += len;
        if total > MAX_REPLY_BYTES {
            return None;
        }
        pending.extend_from_slice(&chunk[..len]);

        // look at every complete line received so far
        loop {
            let mut newline_at: Option<usize> = None;
            let mut index = 0;
            while index < pending.len() {
                if pending[index] == b'\n' {
                    newline_at = Some(index);
                    break;
                }
                index += 1;
            }
            let end = match newline_at {
                Some(i) => i,
                None => break,
            };

            let line: Vec<u8> = pending[..end].to_vec();
            pending.drain(..end + 1);
            let parsed = parse_reply_bytes(name, &line);
            match parsed {
                Some(value) => return Some(value),
                None => {}
            }
        }
    }
}

// parse_reply on raw bytes; bytes that are not UTF-8 are no reply
fn parse_reply_bytes(name: &str, line: &[u8]) -> Option<String> {
    let decoded = std::str::from_utf8(line);
    match decoded {
        Ok(text) => parse_reply(name, text),
        Err(_) => None,
    }
}

/*
 * the value of one reply line, e.g. ("battery", "battery: 87.5") -> "87.5"
 *
 * name: the value name asked for
 * line: one line, with or without its \r\n
 * returns: the value text with spaces trimmed, or None when the line is for
 *          another name, has no colon, or has an empty value
 */
pub fn parse_reply(name: &str, line: &str) -> Option<String> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let split = trimmed.split_once(':');
    let (key, value) = match split {
        Some(pair) => pair,
        None => return None,
    };
    if key.trim() != name {
        return None;
    }
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

/*
 * a percent from a value text, e.g. "87.53" -> 87.53
 *
 * returns: the number clamped to 0 to 100 when it is finite and at most
 *          PERCENT_TOLERANCE outside that range ("100.4" -> 100); None for
 *          anything else ("NaN", "inf", "120", "87%")
 */
pub fn parse_percent(value: &str) -> Option<f64> {
    let parsed = value.parse::<f64>();
    let number = match parsed {
        Ok(n) => n,
        Err(_) => return None,
    };
    if !number.is_finite() {
        return None;
    }
    if number < -PERCENT_TOLERANCE || number > 100.0 + PERCENT_TOLERANCE {
        return None;
    }
    Some(number.clamp(0.0, 100.0))
}

// "true" or "false" exactly, anything else is None
pub fn parse_bool(value: &str) -> Option<bool> {
    if value == "true" {
        return Some(true);
    }
    if value == "false" {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    // every query there is; a new variant must be added here and to the test
    const ALL_QUERIES: [Query; 3] = [
        Query::Battery,
        Query::BatteryCharging,
        Query::BatteryPowerPlugged,
    ];

    // what the fake server does on one connection: wait, then write these bytes
    struct Answer {
        delay: Duration,
        bytes: Vec<u8>,
    }

    fn answer(text: &str) -> Answer {
        Answer {
            delay: Duration::ZERO,
            bytes: text.as_bytes().to_vec(),
        }
    }

    // deletes the socket file when the test ends, even when it fails
    struct SocketFile {
        path: PathBuf,
    }

    impl Drop for SocketFile {
        fn drop(&mut self) {
            let removed = std::fs::remove_file(&self.path);
            match removed {
                Ok(_) => {}
                Err(_) => {}
            }
        }
    }

    // a short, unique socket path for one test
    fn socket_path(name: &str) -> PathBuf {
        PathBuf::from(format!(
            "/tmp/phd-test-{}-{}.sock",
            std::process::id(),
            name
        ))
    }

    /*
     * a fake pisugar-server: accepts one connection per answer, reads the
     * command line, then answers; the handle returns the commands received
     */
    fn serve(name: &str, answers: Vec<Answer>) -> (SocketFile, thread::JoinHandle<Vec<String>>) {
        let path = socket_path(name);
        let file = SocketFile { path: path.clone() };
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let handle = thread::spawn(move || {
            let mut commands: Vec<String> = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut received: Vec<u8> = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    let n = stream.read(&mut byte).unwrap();
                    if n == 0 || byte[0] == b'\n' {
                        break;
                    }
                    received.push(byte[0]);
                }
                commands.push(String::from_utf8(received).unwrap());
                thread::sleep(answer.delay);
                // the client may have given up already: ignore a broken pipe
                let _ = stream.write_all(&answer.bytes);
            }
            commands
        });
        (file, handle)
    }

    fn client(file: &SocketFile) -> PiSugarClient {
        PiSugarClient::new(&file.path, QUERY_TIMEOUT)
    }

    // ---- queries against a fake server ----

    #[test]
    fn reads_the_battery_percent() {
        let (file, handle) = serve("pct", vec![answer("battery: 87.53\n")]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            Some(87.53)
        );
        assert_eq!(handle.join().unwrap(), vec!["get battery".to_string()]);
    }

    #[test]
    fn reads_both_plug_values() {
        let (file, handle) = serve(
            "plug",
            vec![
                answer("battery_power_plugged: true\n"),
                answer("battery_charging: false\r\n"),
            ],
        );
        let pisugar = client(&file);
        assert_eq!(
            pisugar.plugged(Query::BatteryPowerPlugged, pisugar.deadline()),
            Some(true)
        );
        assert_eq!(
            pisugar.plugged(Query::BatteryCharging, pisugar.deadline()),
            Some(false)
        );
        assert_eq!(
            handle.join().unwrap(),
            vec![
                "get battery_power_plugged".to_string(),
                "get battery_charging".to_string()
            ]
        );
    }

    #[test]
    fn plugged_refuses_the_percent_query() {
        let pisugar = PiSugarClient::new(&socket_path("noplug"), QUERY_TIMEOUT);
        assert_eq!(pisugar.plugged(Query::Battery, pisugar.deadline()), None);
    }

    #[test]
    fn missing_socket_is_none() {
        let path = socket_path("missing");
        let _ = std::fs::remove_file(&path);
        let pisugar = PiSugarClient::new(&path, QUERY_TIMEOUT);
        assert_eq!(pisugar.battery_pct(Instant::now() + QUERY_TIMEOUT), None);
    }

    #[test]
    fn silent_server_times_out_within_the_budget() {
        let silent = Answer {
            delay: Duration::from_millis(800),
            bytes: b"battery: 50\n".to_vec(),
        };
        let (file, handle) = serve("silent", vec![silent]);
        let started = Instant::now();
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            None
        );
        let took = started.elapsed();
        assert!(took < Duration::from_millis(600), "took {took:?}");
        handle.join().unwrap();
    }

    #[test]
    fn slow_server_inside_the_budget_is_read() {
        let slow = Answer {
            delay: Duration::from_millis(100),
            bytes: b"battery: 42\n".to_vec(),
        };
        let (file, handle) = serve("slow", vec![slow]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            Some(42.0)
        );
        handle.join().unwrap();
    }

    #[test]
    fn closed_without_a_reply_is_none() {
        let (file, handle) = serve("closed", vec![answer("")]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            None
        );
        handle.join().unwrap();
    }

    #[test]
    fn last_line_without_newline_is_read() {
        let (file, handle) = serve("eof", vec![answer("battery: 64")]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            Some(64.0)
        );
        handle.join().unwrap();
    }

    #[test]
    fn event_lines_before_the_reply_are_skipped() {
        let (file, handle) = serve("event", vec![answer("single\nbattery: 50\n")]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            Some(50.0)
        );
        handle.join().unwrap();
    }

    #[test]
    fn garbage_replies_are_none() {
        let garbage = [
            "battery: abc\n",
            "battery 87\n",
            "battery:\n",
            "model: PiSugar 3\n",
            "\u{1b}[2Jbattery: 50\n",
        ];
        let mut index = 0;
        while index < garbage.len() {
            let text = garbage[index];
            let (file, handle) = serve(&format!("junk{index}"), vec![answer(text)]);
            assert_eq!(
                client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
                None,
                "{text:?}"
            );
            handle.join().unwrap();
            index += 1;
        }

        let not_utf8 = Answer {
            delay: Duration::ZERO,
            bytes: vec![0xff, 0xfe, b'\n'],
        };
        let (file, handle) = serve("utf8", vec![not_utf8]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            None
        );
        handle.join().unwrap();
    }

    #[test]
    fn out_of_range_replies_are_none() {
        let out_of_range = [
            "battery: 120\n",
            "battery: -6\n",
            "battery: NaN\n",
            "battery: inf\n",
        ];
        let mut index = 0;
        while index < out_of_range.len() {
            let text = out_of_range[index];
            let (file, handle) = serve(&format!("range{index}"), vec![answer(text)]);
            assert_eq!(
                client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
                None,
                "{text:?}"
            );
            handle.join().unwrap();
            index += 1;
        }
    }

    #[test]
    fn over_long_reply_is_none() {
        let long = format!("battery: 5{}\n", "0".repeat(MAX_REPLY_BYTES));
        let (file, handle) = serve("long", vec![answer(&long)]);
        assert_eq!(
            client(&file).battery_pct(Instant::now() + QUERY_TIMEOUT),
            None
        );
        handle.join().unwrap();
    }

    #[test]
    fn only_the_three_fixed_lines_can_ever_be_sent() {
        /* Query is the only input to ask(), and line() maps each variant to
         * a constant; this list must stay exactly these three get lines */
        let mut lines: Vec<&str> = Vec::new();
        for query in ALL_QUERIES {
            lines.push(query.line());
        }
        assert_eq!(
            lines,
            vec![
                "get battery\n",
                "get battery_charging\n",
                "get battery_power_plugged\n"
            ]
        );

        // and on the wire each query sends its line and nothing else
        let (file, handle) = serve(
            "fixed",
            vec![
                answer("battery: 1\n"),
                answer("battery_charging: true\n"),
                answer("battery_power_plugged: true\n"),
            ],
        );
        let pisugar = client(&file);
        for query in ALL_QUERIES {
            assert!(pisugar.ask(query, pisugar.deadline()).is_some());
        }
        let received = handle.join().unwrap();
        assert_eq!(
            received,
            vec![
                "get battery".to_string(),
                "get battery_charging".to_string(),
                "get battery_power_plugged".to_string()
            ]
        );
    }

    // ---- pure parsing ----

    #[test]
    fn parse_reply_takes_the_value_of_the_named_line() {
        assert_eq!(
            parse_reply("battery", "battery: 87.5\r\n"),
            Some("87.5".to_string())
        );
        assert_eq!(
            parse_reply("model", "model: PiSugar 2 Pro"),
            Some("PiSugar 2 Pro".to_string())
        );
        assert_eq!(parse_reply("battery", "battery_v: 4.1"), None);
        assert_eq!(parse_reply("battery", "battery"), None);
        assert_eq!(parse_reply("battery", "battery:   "), None);
    }

    #[test]
    fn parse_percent_clamps_near_misses_and_refuses_the_rest() {
        assert_eq!(parse_percent("0"), Some(0.0));
        assert_eq!(parse_percent("100"), Some(100.0));
        assert_eq!(parse_percent("19.9"), Some(19.9));
        assert_eq!(parse_percent("100.4"), Some(100.0));
        assert_eq!(parse_percent("-0.5"), Some(0.0));
        assert_eq!(parse_percent("105"), Some(100.0));
        assert_eq!(parse_percent("105.1"), None);
        assert_eq!(parse_percent("-5.1"), None);
        assert_eq!(parse_percent("NaN"), None);
        assert_eq!(parse_percent("-inf"), None);
        assert_eq!(parse_percent("87%"), None);
        assert_eq!(parse_percent(""), None);
    }

    #[test]
    fn parse_bool_is_exact() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("True"), None);
        assert_eq!(parse_bool("1"), None);
    }
}
