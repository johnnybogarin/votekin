use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, rand_core::CryptoRngCore, traits::PublicKeyParts};

use crate::{SourceProtocol, Vote};

pub const PACKET_BYTES: usize = 256;

/// V1 encryption does not authenticate a voting service: anyone with the public
/// key can construct a packet. Never treat successful decryption as authentication.
pub fn decode(
    packet: &[u8],
    key: &RsaPrivateKey,
    rng: &mut impl CryptoRngCore,
) -> Result<Vote, &'static str> {
    if packet.len() != PACKET_BYTES || key.size() != PACKET_BYTES {
        return Err("Invalid legacy vote");
    }
    let plaintext = key
        .decrypt_blinded(rng, Pkcs1v15Encrypt, packet)
        .map_err(|_| "Invalid legacy vote")?;
    parse(&plaintext)
}

fn parse(plaintext: &[u8]) -> Result<Vote, &'static str> {
    let text = std::str::from_utf8(plaintext).map_err(|_| "Invalid legacy vote")?;
    let text = text.trim_end_matches('\0');
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut fields = text.split('\n');
    if fields.next() != Some("VOTE") {
        return Err("Invalid legacy vote");
    }
    let service = fields.next().ok_or("Invalid legacy vote")?;
    let username = fields.next().ok_or("Invalid legacy vote")?;
    let address = fields.next().ok_or("Invalid legacy vote")?;
    let timestamp = fields.next().ok_or("Invalid legacy vote")?;
    if fields.next().is_some()
        || address.len() > 256
        || timestamp.len() > 64
        || address.chars().any(char::is_control)
        || timestamp.chars().any(char::is_control)
    {
        return Err("Invalid legacy vote");
    }
    // V1 timestamps have no standard units or format
    Vote::new(service, username, SourceProtocol::VotifierV1, None)
        .map_err(|_| "Invalid legacy vote")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_fields_and_optional_padding() {
        for packet in [
            b"VOTE\nList\nAlex\n\n2026-09-08\n\0\0".as_slice(),
            b"VOTE\nList\nAlex\nunknown\n123456",
        ] {
            let vote = parse(packet).unwrap();
            assert_eq!(vote.service(), "List");
            assert_eq!(vote.username(), "Alex");
            assert_eq!(vote.source_protocol(), SourceProtocol::VotifierV1);
            assert_eq!(vote.voted_at(), None);
        }
    }

    #[test]
    fn rejects_bad_opcode_missing_fields_and_controls() {
        for packet in [
            b"OTHER\nList\nAlex\naddress\ntime".as_slice(),
            b"VOTE\nList\nAlex",
            b"VOTE\nList\n\naddress\ntime",
            b"VOTE\nList\nAlex\na\t\ntime",
            b"VOTE\nList\nAlex\naddress\ntime\nextra",
            b"\xff",
        ] {
            assert!(parse(packet).is_err());
        }
    }
}
