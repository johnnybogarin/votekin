use std::{
    collections::BTreeSet,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use votekin_core::{SourceProtocol, Vote};

#[derive(Default)]
pub struct Subscriptions {
    consumers: Mutex<BTreeSet<String>>,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Request {
    #[serde(rename = "votekin:subscribe/v1")]
    Subscribe {},
    #[serde(rename = "votekin:unsubscribe/v1")]
    Unsubscribe {},
}

#[derive(Serialize)]
struct VoteMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    service: &'a str,
    username: &'a str,
    source_protocol: &'static str,
    voted_at: Option<i64>,
    received_at: i64,
}

impl Subscriptions {
    pub fn handle(&self, sender: &str, message: &[u8]) -> Result<Vec<u8>, String> {
        if sender.is_empty() || sender == "votekin" {
            return Err("Invalid subscriber".into());
        }
        if message.len() > 256 {
            return Err("Subscription request exceeds 256 bytes".into());
        }
        let request: Request =
            serde_json::from_slice(message).map_err(|_| "Unsupported VoteKin IPC request")?;
        let mut consumers = self
            .consumers
            .lock()
            .map_err(|_| "VoteKin subscriptions unavailable")?;
        match request {
            Request::Subscribe {} => {
                consumers.insert(sender.to_owned());
            }
            Request::Unsubscribe {} => {
                consumers.remove(sender);
            }
        }
        Ok(b"{\"status\":\"ok\"}".to_vec())
    }

    pub fn clear(&self) -> Result<(), String> {
        self.consumers
            .lock()
            .map_err(|_| "VoteKin subscriptions unavailable")?
            .clear();
        Ok(())
    }

    pub fn publish(
        &self,
        vote: &Vote,
        mut send: impl FnMut(&str, &[u8]) -> Result<(), ()>,
    ) -> Result<Vec<String>, String> {
        let consumers: Vec<String> = self
            .consumers
            .lock()
            .map_err(|_| "VoteKin subscriptions unavailable")?
            .iter()
            .cloned()
            .collect();
        if consumers.is_empty() {
            return Ok(Vec::new());
        }
        let message = encode_vote(vote)?;
        let mut failed = Vec::new();
        for consumer in consumers {
            if send(&consumer, &message).is_err() {
                failed.push(consumer);
            }
        }
        Ok(failed)
    }
}

fn encode_vote(vote: &Vote) -> Result<Vec<u8>, String> {
    let message = VoteMessage {
        kind: "votekin:vote.accepted/v1",
        service: vote.service(),
        username: vote.username(),
        source_protocol: match vote.source_protocol() {
            SourceProtocol::VotifierV1 => "votifier_v1",
            SourceProtocol::NuVotifierV2 => "nuvotifier_v2",
        },
        voted_at: vote.voted_at().map(unix_millis).transpose()?,
        received_at: unix_millis(vote.received_at())?,
    };
    serde_json::to_vec(&message).map_err(|_| "Cannot encode VoteKin IPC vote".into())
}

fn unix_millis(time: SystemTime) -> Result<i64, String> {
    let millis = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as i128,
        Err(error) => -(error.duration().as_millis() as i128),
    };
    i64::try_from(millis).map_err(|_| "VoteKin IPC timestamp is out of range".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    const SUBSCRIBE: &[u8] = br#"{"type":"votekin:subscribe/v1"}"#;
    const UNSUBSCRIBE: &[u8] = br#"{"type":"votekin:unsubscribe/v1"}"#;

    #[test]
    fn subscriptions_use_sender_and_are_idempotent() {
        let subscriptions = Subscriptions::default();
        assert_eq!(
            subscriptions.handle("rewards", SUBSCRIBE).unwrap(),
            br#"{"status":"ok"}"#
        );
        subscriptions.handle("rewards", SUBSCRIBE).unwrap();
        assert_eq!(subscriptions.consumers.lock().unwrap().len(), 1);
        assert!(
            subscriptions
                .handle(
                    "other",
                    br#"{"type":"votekin:subscribe/v1","recipient":"rewards"}"#
                )
                .is_err()
        );
        for message in [
            b"{}".as_slice(),
            b"not json",
            br#"{"type":"votekin:subscribe/v2"}"#,
            &[b' '; 257],
        ] {
            assert!(subscriptions.handle("other", message).is_err());
        }
        subscriptions.handle("other", UNSUBSCRIBE).unwrap();
        assert_eq!(subscriptions.consumers.lock().unwrap().len(), 1);
        subscriptions.handle("rewards", UNSUBSCRIBE).unwrap();
        subscriptions.handle("rewards", UNSUBSCRIBE).unwrap();
        assert!(subscriptions.consumers.lock().unwrap().is_empty());
        subscriptions.handle("rewards", SUBSCRIBE).unwrap();
        subscriptions.clear().unwrap();
        assert!(subscriptions.consumers.lock().unwrap().is_empty());
    }

    #[test]
    fn vote_schema_preserves_protocol_and_timestamp_meaning() {
        let vote = Vote::new(
            "List",
            "Alex",
            SourceProtocol::NuVotifierV2,
            Some(UNIX_EPOCH - Duration::from_millis(1)),
        )
        .unwrap();
        let message: serde_json::Value =
            serde_json::from_slice(&encode_vote(&vote).unwrap()).unwrap();
        assert_eq!(
            message,
            json!({
                "type": "votekin:vote.accepted/v1", "service": "List", "username": "Alex",
                "source_protocol": "nuvotifier_v2", "voted_at": -1,
                "received_at": unix_millis(vote.received_at()).unwrap(),
            })
        );
        let legacy = Vote::new("List", "Alex", SourceProtocol::VotifierV1, None).unwrap();
        let message: serde_json::Value =
            serde_json::from_slice(&encode_vote(&legacy).unwrap()).unwrap();
        assert_eq!(message["source_protocol"], "votifier_v1");
        assert!(message["voted_at"].is_null());
    }

    #[test]
    fn delivery_continues_after_failure_and_allows_reentry() {
        let subscriptions = Subscriptions::default();
        for name in ["a", "b"] {
            subscriptions.handle(name, SUBSCRIBE).unwrap();
        }
        let vote = Vote::new("List", "Alex", SourceProtocol::NuVotifierV2, None).unwrap();
        let mut sent = Vec::new();
        let failed = subscriptions
            .publish(&vote, |name, _| {
                sent.push(name.to_owned());
                subscriptions.handle(name, UNSUBSCRIBE).unwrap();
                if name == "a" { Err(()) } else { Ok(()) }
            })
            .unwrap();
        assert_eq!(sent, ["a", "b"]);
        assert_eq!(failed, ["a"]);
        subscriptions
            .publish(&vote, |_, _| panic!("unsubscribed consumer called"))
            .unwrap();
    }
}
