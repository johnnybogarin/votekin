//! Protocol and vote handling for VoteKin.

pub mod v2;

use std::{fmt, time::SystemTime};

pub const MAX_SERVICE_NAME_BYTES: usize = 128;
pub const MAX_PLAYER_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceProtocol {
    VotifierV1,
    NuVotifierV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    service: String,
    username: String,
    source_protocol: SourceProtocol,
    voted_at: Option<SystemTime>,
    received_at: SystemTime,
}

impl Vote {
    pub fn new(
        service: &str,
        username: &str,
        source_protocol: SourceProtocol,
        voted_at: Option<SystemTime>,
    ) -> Result<Self, ValidationError> {
        validate_name(service, "service", MAX_SERVICE_NAME_BYTES)?;
        validate_name(username, "username", MAX_PLAYER_NAME_BYTES)?;

        Ok(Self {
            service: service.to_owned(),
            username: username.to_owned(),
            source_protocol,
            voted_at,
            received_at: SystemTime::now(),
        })
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn source_protocol(&self) -> SourceProtocol {
        self.source_protocol
    }

    pub fn voted_at(&self) -> Option<SystemTime> {
        self.voted_at
    }

    pub fn received_at(&self) -> SystemTime {
        self.received_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Empty {
        field: &'static str,
    },
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    ControlCharacter {
        field: &'static str,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(f, "{field} must not be empty or whitespace-only"),
            Self::TooLong { field, max_bytes } => {
                write!(f, "{field} must not exceed {max_bytes} bytes")
            }
            Self::ControlCharacter { field } => {
                write!(f, "{field} must not contain control characters")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

fn validate_name(
    value: &str,
    field: &'static str,
    max_bytes: usize,
) -> Result<(), ValidationError> {
    if value.len() > max_bytes {
        return Err(ValidationError::TooLong { field, max_bytes });
    }
    if value.trim().is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if value.chars().any(char::is_control) {
        return Err(ValidationError::ControlCharacter { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn preserves_fields_and_records_local_time() {
        let before = SystemTime::now();
        let claimed_time = before + Duration::from_secs(3600);
        let vote = Vote::new(
            "Example List",
            ".Bedrock Player",
            SourceProtocol::NuVotifierV2,
            Some(claimed_time),
        )
        .unwrap();

        assert_eq!(vote.service(), "Example List");
        assert_eq!(vote.username(), ".Bedrock Player");
        assert_eq!(vote.source_protocol(), SourceProtocol::NuVotifierV2);
        assert_eq!(vote.voted_at(), Some(claimed_time));
        assert!(vote.received_at() >= before);
        assert!(vote.received_at() <= SystemTime::now());
        assert_ne!(vote.received_at(), claimed_time);
    }

    #[test]
    fn enforces_byte_limits_for_both_names() {
        let service = "s".repeat(MAX_SERVICE_NAME_BYTES);
        let username = "é".repeat(MAX_PLAYER_NAME_BYTES / 2);
        let vote = Vote::new(&service, &username, SourceProtocol::VotifierV1, None).unwrap();
        assert_eq!(vote.voted_at(), None);

        assert_eq!(
            Vote::new(
                &(service + "s"),
                &username,
                SourceProtocol::VotifierV1,
                None
            ),
            Err(ValidationError::TooLong {
                field: "service",
                max_bytes: MAX_SERVICE_NAME_BYTES,
            })
        );
        assert_eq!(
            Vote::new("List", &(username + "x"), SourceProtocol::VotifierV1, None),
            Err(ValidationError::TooLong {
                field: "username",
                max_bytes: MAX_PLAYER_NAME_BYTES,
            })
        );
    }

    #[test]
    fn rejects_blank_names_and_control_characters() {
        for name in ["", "   ", "\u{2003}"] {
            for (service, username, field) in
                [(name, "Alex", "service"), ("List", name, "username")]
            {
                assert_eq!(
                    Vote::new(service, username, SourceProtocol::VotifierV1, None),
                    Err(ValidationError::Empty { field })
                );
            }
        }
        for name in [
            "name\nforged log",
            "name\r",
            "name\0",
            "name\t",
            "\u{1b}[31mname",
            "name\u{85}",
        ] {
            for (service, username, field) in
                [(name, "Alex", "service"), ("List", name, "username")]
            {
                let error =
                    Vote::new(service, username, SourceProtocol::VotifierV1, None).unwrap_err();
                assert_eq!(error, ValidationError::ControlCharacter { field });
                assert!(!error.to_string().contains(name));
            }
        }
    }
}
