use std::{
    fmt,
    net::IpAddr,
    time::{Duration, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::{SourceProtocol, ValidationError, Vote};

pub const MAX_MESSAGE_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    IncompleteFrame,
    InvalidMagic,
    MessageTooLarge,
    TrailingBytes,
    InvalidJson,
    InvalidSignature,
    InvalidChallenge,
    EmptyToken,
    InvalidAddress,
    InvalidTimestamp,
    InvalidVote(ValidationError),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::IncompleteFrame => "incomplete vote frame",
            Self::InvalidMagic => "invalid NuVotifier v2 magic bytes",
            Self::MessageTooLarge => "vote message exceeds size limit",
            Self::TrailingBytes => "unexpected bytes after vote frame",
            Self::InvalidJson => "invalid vote JSON or field types",
            Self::InvalidSignature => "invalid vote signature",
            Self::InvalidChallenge => "vote challenge does not match",
            Self::EmptyToken => "vote authentication token is empty",
            Self::InvalidAddress => "invalid vote address",
            Self::InvalidTimestamp => "vote timestamp is out of range",
            Self::InvalidVote(error) => return error.fmt(f),
        };
        f.write_str(message)
    }
}

impl std::error::Error for DecodeError {}

#[derive(Deserialize)]
struct Envelope {
    payload: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    service_name: String,
    username: String,
    address: String,
    timestamp: i64,
    challenge: String,
}

pub fn frame_length(header: &[u8]) -> Result<usize, DecodeError> {
    if header.len() < 4 {
        return Err(DecodeError::IncompleteFrame);
    }
    if header[..2] != [0x73, 0x3a] {
        return Err(DecodeError::InvalidMagic);
    }
    let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if length > MAX_MESSAGE_BYTES {
        return Err(DecodeError::MessageTooLarge);
    }
    Ok(length + 4)
}

pub fn decode(frame: &[u8], token: &str, expected_challenge: &str) -> Result<Vote, DecodeError> {
    let length = frame_length(frame)?;
    if frame.len() < length {
        return Err(DecodeError::IncompleteFrame);
    }
    if frame.len() > length {
        return Err(DecodeError::TrailingBytes);
    }
    if token.is_empty() {
        return Err(DecodeError::EmptyToken);
    }
    if expected_challenge.is_empty() {
        return Err(DecodeError::InvalidChallenge);
    }
    let envelope: Envelope =
        serde_json::from_slice(&frame[4..]).map_err(|_| DecodeError::InvalidJson)?;
    let signature = STANDARD
        .decode(&envelope.signature)
        .map_err(|_| DecodeError::InvalidSignature)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes())
        .map_err(|_| DecodeError::InvalidSignature)?;

    mac.update(envelope.payload.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| DecodeError::InvalidSignature)?;

    let payload: Payload =
        serde_json::from_str(&envelope.payload).map_err(|_| DecodeError::InvalidJson)?;
    if payload.challenge != expected_challenge {
        return Err(DecodeError::InvalidChallenge);
    }
    payload
        .address
        .parse::<IpAddr>()
        .map_err(|_| DecodeError::InvalidAddress)?;
    let offset = Duration::from_millis(payload.timestamp.unsigned_abs());
    let voted_at = if payload.timestamp < 0 {
        UNIX_EPOCH.checked_sub(offset)
    } else {
        UNIX_EPOCH.checked_add(offset)
    }
    .ok_or(DecodeError::InvalidTimestamp)?;

    Vote::new(
        &payload.service_name,
        &payload.username,
        SourceProtocol::NuVotifierV2,
        Some(voted_at),
    )
    .map_err(DecodeError::InvalidVote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PAYLOAD: &str = r#"{"serviceName":"ExampleList", "username":"Alex","address":"127.0.0.1","timestamp":1700000000123,"challenge":"test-challenge"}"#;

    const SIGNATURE: &str = "aGp4jrtM+NXJVestaP0lIYPwhzsTE5YTn78WRCxtZSQ=";

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x73, 0x3a];
        bytes.extend_from_slice(&u16::try_from(body.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    fn envelope(payload: &str, signature: &str) -> Vec<u8> {
        frame(&serde_json::to_vec(&json!({"payload": payload, "signature": signature})).unwrap())
    }

    fn signed(payload: &str) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"test-token").unwrap();
        mac.update(payload.as_bytes());
        envelope(payload, &STANDARD.encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn decodes_independently_signed_vote() {
        let vote = decode(
            &envelope(PAYLOAD, SIGNATURE),
            "test-token",
            "test-challenge",
        )
        .unwrap();
        assert_eq!(vote.service(), "ExampleList");
        assert_eq!(vote.username(), "Alex");
        assert_eq!(vote.source_protocol(), SourceProtocol::NuVotifierV2);
        assert_eq!(
            vote.voted_at(),
            Some(UNIX_EPOCH + Duration::from_millis(1700000000123))
        );
    }

    #[test]
    fn rejects_wrong_token_and_tampered_payload() {
        assert_eq!(
            decode(&envelope(PAYLOAD, SIGNATURE), "wrong", "test-challenge"),
            Err(DecodeError::InvalidSignature)
        );
        assert_eq!(
            decode(
                &envelope(&PAYLOAD.replace("Alex", "Other"), SIGNATURE),
                "test-token",
                "test-challenge"
            ),
            Err(DecodeError::InvalidSignature)
        );
        for signature in ["!", "", "YWJj"] {
            assert_eq!(
                decode(
                    &envelope(PAYLOAD, signature),
                    "test-token",
                    "test-challenge"
                ),
                Err(DecodeError::InvalidSignature)
            );
        }
    }

    #[test]
    fn rejects_wrong_challenge_and_empty_credentials() {
        let packet = envelope(PAYLOAD, SIGNATURE);
        for challenge in ["other-connection", ""] {
            assert_eq!(
                decode(&packet, "test-token", challenge),
                Err(DecodeError::InvalidChallenge)
            );
        }
        assert_eq!(
            decode(&packet, "", "test-challenge"),
            Err(DecodeError::EmptyToken)
        );
    }

    #[test]
    fn checks_framing_before_parsing() {
        let packet = envelope(PAYLOAD, SIGNATURE);
        for end in 0..packet.len() {
            assert_eq!(
                decode(&packet[..end], "test-token", "test-challenge"),
                Err(DecodeError::IncompleteFrame)
            );
        }
        let mut extra = packet.clone();
        extra.push(0);
        assert_eq!(
            decode(&extra, "test-token", "test-challenge"),
            Err(DecodeError::TrailingBytes)
        );
        let mut bad_magic = packet;
        bad_magic[0] = 0;
        assert_eq!(
            decode(&bad_magic, "test-token", "test-challenge"),
            Err(DecodeError::InvalidMagic)
        );
        assert_eq!(
            frame_length(&[0x73, 0x3a, 0x20, 0]),
            Ok(MAX_MESSAGE_BYTES + 4)
        );
        assert_eq!(
            frame_length(&[0x73, 0x3a, 0x20, 1]),
            Err(DecodeError::MessageTooLarge)
        );
    }

    #[test]
    fn rejects_malformed_json_and_missing_fields() {
        for packet in [
            frame(b"{"),
            frame(b"{}"),
            frame(b"\xff"),
            signed("{"),
            signed("{}"),
        ] {
            assert_eq!(
                decode(&packet, "test-token", "test-challenge"),
                Err(DecodeError::InvalidJson)
            );
        }
        let wrong_type = PAYLOAD.replace("1700000000123", "\"1700000000123\"");
        assert_eq!(
            decode(&signed(&wrong_type), "test-token", "test-challenge"),
            Err(DecodeError::InvalidJson)
        );
    }

    #[test]
    fn validates_authenticated_fields() {
        for (from, to, error) in [
            ("127.0.0.1", "invalid-address", DecodeError::InvalidAddress),
            (
                "Alex",
                "",
                DecodeError::InvalidVote(ValidationError::Empty { field: "username" }),
            ),
            (
                "Alex",
                "Alex\\nforged log",
                DecodeError::InvalidVote(ValidationError::ControlCharacter { field: "username" }),
            ),
        ] {
            assert_eq!(
                decode(
                    &signed(&PAYLOAD.replace(from, to)),
                    "test-token",
                    "test-challenge"
                ),
                Err(error)
            );
        }
        let ipv6 = PAYLOAD.replace("127.0.0.1", "::1");
        assert!(decode(&signed(&ipv6), "test-token", "test-challenge").is_ok());
        for milliseconds in [0_i64, -1] {
            let payload = PAYLOAD.replace("1700000000123", &milliseconds.to_string());
            let vote = decode(&signed(&payload), "test-token", "test-challenge").unwrap();
            assert_eq!(
                vote.voted_at(),
                UNIX_EPOCH.checked_sub(Duration::from_millis(milliseconds.unsigned_abs()))
            );
        }
    }
}
