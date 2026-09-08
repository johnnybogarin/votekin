use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};

use crate::legacy::Legacy;
use votekin_core::{Vote, v1, v2};

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
    legacy: Option<Legacy>,
    next_failure_log: Instant,
    suppressed: usize,
}

impl Listener {
    pub fn bind(config: Config, legacy: Option<Legacy>) -> Result<Self, String> {
        let socket = TcpListener::bind(config.address())
            .map_err(|error| format!("Cannot bind VoteKin to {}: {error}", config.address()))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| format!("Cannot configure VoteKin listener: {error}"))?;
        Ok(Self {
            socket,
            connections: Vec::new(),
            token: config.token,
            legacy,
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
        let mut legacy_budget = 1;
        for index in 0..self.connections.len() {
            match self.connections[index].poll(
                &self.token,
                now,
                self.legacy.as_mut(),
                &mut legacy_budget,
            ) {
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
    legacy_frame: bool,
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
            legacy_frame: false,
            output,
            written: 0,
        }
    }

    fn poll(
        &mut self,
        token: &str,
        now: Instant,
        legacy: Option<&mut Legacy>,
        legacy_budget: &mut usize,
    ) -> Outcome {
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
                            if self.input[..2] == [0x73, 0x3a] {
                                match v2::frame_length(&self.input) {
                                    Ok(length) => self.expected = length,
                                    Err(error) => return self.reject(error),
                                }
                            } else if legacy.is_some() {
                                self.expected = v1::PACKET_BYTES;
                                self.legacy_frame = true;
                            } else {
                                return self
                                    .close("Legacy Votifier v1 is disabled or invalid protocol");
                            }
                        }
                        if self.input.len() == self.expected {
                            if self.legacy_frame {
                                if *legacy_budget == 0 {
                                    return Outcome::Pending;
                                }
                                *legacy_budget -= 1;
                                self.stage = Stage::Closed;
                                let Some(legacy) = legacy else {
                                    return Outcome::Rejected(
                                        "Legacy Votifier v1 is disabled".into(),
                                    );
                                };
                                // Limit costly private-key operations to one per server tick.
                                return match v1::decode(&self.input, &legacy.key, &mut legacy.rng) {
                                    Ok(vote) => Outcome::Accepted(vote),
                                    Err(_) => Outcome::Rejected("Invalid legacy vote".into()),
                                };
                            }
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
            match connection.poll("test-token", now, None, &mut 1) {
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
                match connection.poll(token, now, None, &mut 1) {
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
    fn legacy_keys_persist_and_listener_handles_both_protocols() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs8::DecodePublicKey};
        let folder = std::env::temp_dir().join(format!("votekin-v1-{}", random_secret().unwrap()));
        std::fs::create_dir(&folder).unwrap();
        let mut legacy = Legacy::load(&folder).unwrap();
        let saved = std::fs::read(folder.join("private.pem")).unwrap();
        let restored = Legacy::load(&folder).unwrap();
        assert_eq!(legacy.key, restored.key);
        assert_eq!(std::fs::read(folder.join("private.pem")).unwrap(), saved);
        let public = STANDARD
            .decode(std::fs::read(folder.join("public.key")).unwrap())
            .unwrap();
        let public = RsaPublicKey::from_public_key_der(&public).unwrap();
        assert_eq!(public, RsaPublicKey::from(&legacy.key));
        let encrypted = public
            .encrypt(
                &mut legacy.rng,
                Pkcs1v15Encrypt,
                b"VOTE\nList\nAlex\nunknown\n123456\n",
            )
            .unwrap();
        assert_eq!(encrypted.len(), v1::PACKET_BYTES);
        let now = Instant::now();
        for (input, enabled, expected_protocol) in [
            (
                encrypted.clone(),
                true,
                Some(votekin_core::SourceProtocol::VotifierV1),
            ),
            (
                packet(),
                true,
                Some(votekin_core::SourceProtocol::NuVotifierV2),
            ),
            (encrypted, false, None),
            (vec![0; 256], true, None),
        ] {
            let mut connection = Connection::new(
                Socket {
                    input: input.into(),
                    ..Socket::default()
                },
                "test-challenge".into(),
                now,
            );
            let mut accepted = None;
            let mut rejected = 0;
            for _ in 0..1000 {
                let key = if enabled { Some(&mut legacy) } else { None };
                match connection.poll("test-token", now, key, &mut 1) {
                    Outcome::Accepted(vote) => {
                        assert!(accepted.is_none());
                        accepted = Some(vote.source_protocol());
                    }
                    Outcome::Rejected(_) => rejected += 1,
                    Outcome::Pending => {}
                }
                if connection.stage == Stage::Closed {
                    break;
                }
            }
            assert!(connection.stage == Stage::Closed);
            assert_eq!(accepted, expected_protocol);
            assert_eq!(rejected, usize::from(expected_protocol.is_none()));
            if expected_protocol != Some(votekin_core::SourceProtocol::NuVotifierV2) {
                assert_eq!(connection.stream.output, b"VOTIFIER 2 test-challenge\n");
            }
        }
        std::fs::write(folder.join("private.pem"), b"broken").unwrap();
        assert!(Legacy::load(&folder).is_err());
        assert_eq!(
            std::fs::read(folder.join("private.pem")).unwrap(),
            b"broken"
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn deadline_closes_idle_connection() {
        let now = Instant::now();
        let mut connection = Connection::new(Socket::default(), "challenge".into(), now);
        assert!(matches!(
            connection.poll("token", now, None, &mut 1),
            Outcome::Pending
        ));
        assert!(matches!(
            connection.poll("token", now + CONNECTION_TIMEOUT, None, &mut 1),
            Outcome::Rejected(_)
        ));
        assert!(connection.stage == Stage::Closed);
        assert!(matches!(
            connection.poll("token", now + CONNECTION_TIMEOUT, None, &mut 1),
            Outcome::Pending
        ));
    }
}
