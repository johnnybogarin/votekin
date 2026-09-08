use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};

use votekin_core::{Vote, v2};

use crate::config::{Config, random_secret};

const MAX_CONNECTIONS: usize = 32;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_INTERVAL: Duration = Duration::from_secs(5);

pub enum Event {
    Accepted(Vote),
    Failure { reason: String, suppressed: usize },
}

pub struct Listener {
    socket: TcpListener,
    connections: Vec<Connection<TcpStream>>,
    token: String,
    next_failure_log: Instant,
    suppressed: usize,
}

impl Listener {
    pub fn bind(config: Config) -> Result<Self, String> {
        let socket = TcpListener::bind(config.address())
            .map_err(|error| format!("Cannot bind VoteKin to {}: {error}", config.address()))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| format!("Cannot configure VoteKin listener: {error}"))?;
        Ok(Self {
            socket,
            connections: Vec::new(),
            token: config.token,
            next_failure_log: Instant::now(),
            suppressed: 0,
        })
    }

    pub fn poll(&mut self) -> Vec<Event> {
        let now = Instant::now();
        let mut events = Vec::new();
        for _ in 0..8 {
            match self.socket.accept() {
                Ok((stream, _)) => {
                    if self.connections.len() >= MAX_CONNECTIONS {
                        self.failure("Connection limit reached".into(), now, &mut events);
                        continue;
                    }
                    if stream.set_nonblocking(true).is_err() {
                        self.failure("Cannot configure incoming socket".into(), now, &mut events);
                        continue;
                    }
                    match random_secret() {
                        Ok(challenge) => self
                            .connections
                            .push(Connection::new(stream, challenge, now)),
                        Err(error) => self.failure(error, now, &mut events),
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.failure("Cannot accept incoming connection".into(), now, &mut events);
                    break;
                }
            }
        }
        for index in 0..self.connections.len() {
            match self.connections[index].poll(&self.token, now) {
                Outcome::Accepted(vote) => events.push(Event::Accepted(vote)),
                Outcome::Rejected(reason) => self.failure(reason, now, &mut events),
                Outcome::Pending => {}
            }
        }
        self.connections
            .retain(|connection| connection.stage != Stage::Closed);
        events
    }

    fn failure(&mut self, reason: String, now: Instant, events: &mut Vec<Event>) {
        if now >= self.next_failure_log {
            events.push(Event::Failure {
                reason,
                suppressed: self.suppressed,
            });
            self.suppressed = 0;
            self.next_failure_log = now + LOG_INTERVAL;
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
        }
    }
}

#[derive(PartialEq, Eq)]
enum Stage {
    Greeting,
    Reading,
    Reply,
    Closed,
}

enum Outcome {
    Pending,
    Accepted(Vote),
    Rejected(String),
}

struct Connection<S> {
    stream: S,
    challenge: String,
    deadline: Instant,
    stage: Stage,
    input: Vec<u8>,
    expected: usize,
    output: Vec<u8>,
    written: usize,
}

impl<S: Read + Write> Connection<S> {
    fn new(stream: S, challenge: String, now: Instant) -> Self {
        let output = format!("VOTIFIER 2 {challenge}\n").into_bytes();
        Self {
            stream,
            challenge,
            deadline: now + CONNECTION_TIMEOUT,
            stage: Stage::Greeting,
            input: Vec::with_capacity(4),
            expected: 4,
            output,
            written: 0,
        }
    }

    fn poll(&mut self, token: &str, now: Instant) -> Outcome {
        if self.stage == Stage::Closed {
            return Outcome::Pending;
        }
        if now >= self.deadline {
            self.stage = Stage::Closed;
            return Outcome::Rejected("Connection timed out".into());
        }
        for _ in 0..16 {
            match self.stage {
                Stage::Greeting | Stage::Reply => {
                    if self.written == self.output.len() {
                        if self.stage == Stage::Reply {
                            self.stage = Stage::Closed;
                            return Outcome::Pending;
                        }
                        self.stage = Stage::Reading;
                        continue;
                    }
                    match self.stream.write(&self.output[self.written..]) {
                        Ok(0) => return self.close("Connection closed while writing"),
                        Ok(count) => self.written += count,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            return Outcome::Pending;
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => return self.close("Connection write failed"),
                    }
                }
                Stage::Reading => {
                    if self.input.len() == self.expected {
                        if self.expected == 4 {
                            match v2::frame_length(&self.input) {
                                Ok(length) => self.expected = length,
                                Err(error) => return self.reject(error),
                            }
                        }
                        if self.input.len() == self.expected {
                            return match v2::decode(&self.input, token, &self.challenge) {
                                Ok(vote) => {
                                    self.reply(b"{\"status\":\"ok\"}\n".to_vec());
                                    Outcome::Accepted(vote)
                                }
                                Err(error) => self.reject(error),
                            };
                        }
                    }
                    let mut bytes = [0; 1024];
                    let count = bytes.len().min(self.expected - self.input.len());
                    match self.stream.read(&mut bytes[..count]) {
                        Ok(0) => return self.close("Connection closed before vote completed"),
                        Ok(count) => self.input.extend_from_slice(&bytes[..count]),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            return Outcome::Pending;
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => return self.close("Connection read failed"),
                    }
                }
                Stage::Closed => return Outcome::Pending,
            }
        }
        Outcome::Pending
    }

    fn reply(&mut self, bytes: Vec<u8>) {
        self.output = bytes;
        self.written = 0;
        self.stage = Stage::Reply;
        self.challenge.clear();
    }

    fn reject(&mut self, error: v2::DecodeError) -> Outcome {
        let reason = error.to_string();
        let response = serde_json::json!({
            "status": "error", "cause": "VoteRejected", "errorMessage": reason,
        });
        self.reply(format!("{response}\n").into_bytes());
        Outcome::Rejected(reason)
    }

    fn close(&mut self, reason: &str) -> Outcome {
        self.stage = Stage::Closed;
        Outcome::Rejected(reason.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Socket {
        input: VecDeque<u8>,
        output: Vec<u8>,
        pause: bool,
    }

    impl Read for Socket {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.pause = !self.pause;
            if self.pause || self.input.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = output.len().min(7).min(self.input.len());
            for byte in &mut output[..count] {
                *byte = self.input.pop_front().unwrap();
            }
            Ok(count)
        }
    }

    impl Write for Socket {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.pause = !self.pause;
            if self.pause {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = input.len().min(3);
            self.output.extend_from_slice(&input[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn packet() -> Vec<u8> {
        let payload = r#"{"serviceName":"ExampleList", "username":"Alex","address":"127.0.0.1","timestamp":1700000000123,"challenge":"test-challenge"}"#;
        let body = serde_json::json!({"payload": payload, "signature": "aGp4jrtM+NXJVestaP0lIYPwhzsTE5YTn78WRCxtZSQ="}).to_string();
        let mut frame = vec![0x73, 0x3a];
        frame.extend_from_slice(&(body.len() as u16).to_be_bytes());
        frame.extend_from_slice(body.as_bytes());
        frame
    }

    #[test]
    fn partial_io_accepts_once_and_finishes_response() {
        let mut input = packet();
        input.extend_from_slice(&packet());
        let socket = Socket {
            input: input.into(),
            ..Socket::default()
        };
        let now = Instant::now();
        let mut connection = Connection::new(socket, "test-challenge".into(), now);
        let mut accepted = 0;
        for _ in 0..1000 {
            match connection.poll("test-token", now) {
                Outcome::Accepted(vote) => {
                    assert_eq!(vote.username(), "Alex");
                    accepted += 1;
                }
                Outcome::Rejected(reason) => panic!("{reason}"),
                Outcome::Pending => {}
            }
            if connection.stage == Stage::Closed {
                break;
            }
        }
        assert!(connection.stage == Stage::Closed);
        assert_eq!(accepted, 1);
        assert_eq!(connection.stream.input.len(), packet().len());
        assert_eq!(
            String::from_utf8(connection.stream.output).unwrap(),
            "VOTIFIER 2 test-challenge\n{\"status\":\"ok\"}\n"
        );
    }

    #[test]
    fn invalid_votes_receive_error_and_oversized_frames_stop_at_header() {
        for (input, token) in [
            (packet(), "wrong-token"),
            (vec![0x73, 0x3a, 0xff, 0xff], "test-token"),
        ] {
            let now = Instant::now();
            let mut connection = Connection::new(
                Socket {
                    input: input.into(),
                    ..Socket::default()
                },
                "test-challenge".into(),
                now,
            );
            let mut rejected = 0;
            for _ in 0..1000 {
                match connection.poll(token, now) {
                    Outcome::Accepted(_) => panic!("invalid vote accepted"),
                    Outcome::Rejected(_) => rejected += 1,
                    Outcome::Pending => {}
                }
                if connection.stage == Stage::Closed {
                    break;
                }
            }
            assert!(connection.stage == Stage::Closed);
            assert_eq!(rejected, 1);
            let output = String::from_utf8(connection.stream.output).unwrap();
            let response: serde_json::Value =
                serde_json::from_str(output.lines().nth(1).unwrap()).unwrap();
            assert_eq!(response["status"], "error");
            assert!(!output.contains(token));
        }
    }

    #[test]
    fn deadline_closes_idle_connection() {
        let now = Instant::now();
        let mut connection = Connection::new(Socket::default(), "challenge".into(), now);
        assert!(matches!(connection.poll("token", now), Outcome::Pending));
        assert!(matches!(
            connection.poll("token", now + CONNECTION_TIMEOUT),
            Outcome::Rejected(_)
        ));
        assert!(connection.stage == Stage::Closed);
        assert!(matches!(
            connection.poll("token", now + CONNECTION_TIMEOUT),
            Outcome::Pending
        ));
    }
}
