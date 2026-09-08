use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::Path,
};

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bind_address: IpAddr,
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub enable_v1: bool,
}

impl Config {
    pub fn load(folder: &Path) -> Result<Self, String> {
        fs::create_dir_all(folder).map_err(|_| "Cannot create VoteKin data directory")?;
        let path = folder.join("config.json");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                let result = (|| {
                    let config = Self {
                        bind_address: IpAddr::from([0, 0, 0, 0]),
                        port: 8192,
                        token: random_secret()?,
                        enable_v1: false,
                    };
                    let data = serde_json::to_vec_pretty(&config)
                        .map_err(|_| "Cannot encode VoteKin configuration")?;
                    file.write_all(&data)
                        .map_err(|_| "Cannot write VoteKin configuration")?;
                    file.sync_all()
                        .map_err(|_| "Cannot save VoteKin configuration")?;
                    Ok(config)
                })();
                if result.is_err() {
                    drop(file);
                    let _ = fs::remove_file(&path);
                }
                result
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut data = Vec::new();
                File::open(&path)
                    .and_then(|file| file.take(16385).read_to_end(&mut data))
                    .map_err(|_| "Cannot read VoteKin config.json")?;
                if data.len() > 16384 {
                    return Err("VoteKin config.json exceeds 16 KiB".into());
                }
                Self::parse(&data)
            }
            Err(_) => Err("Cannot create VoteKin config.json".into()),
        }
    }

    fn parse(data: &[u8]) -> Result<Self, String> {
        let config: Self = serde_json::from_slice(data).map_err(
            |_| "Invalid VoteKin config.json: check bind_address, port, token, and enable_v1",
        )?;
        if config.port == 0 {
            return Err("VoteKin port must be between 1 and 65535".into());
        }
        if config.token.trim().is_empty()
            || config.token.len() > 1024
            || config.token.chars().any(char::is_control)
        {
            return Err("VoteKin token must be nonblank, at most 1024 bytes, and contain no control characters".into());
        }
        Ok(config)
    }

    pub fn address(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.port)
    }
}

pub fn random_secret() -> Result<String, String> {
    use std::fmt::Write;
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Secure randomness unavailable")?;
    let mut value = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_existing_token_and_rejects_invalid_config() {
        let folder = std::env::temp_dir().join(format!("votekin-{}", random_secret().unwrap()));
        let first = Config::load(&folder).unwrap();
        assert_eq!(first.token.len(), 64);
        assert!(!first.enable_v1);
        assert!(
            !Config::parse(br#"{"bind_address":"0.0.0.0","port":8192,"token":"secret"}"#)
                .unwrap()
                .enable_v1
        );
        assert_eq!(Config::load(&folder).unwrap().token, first.token);
        fs::write(folder.join("config.json"), b"{broken secret").unwrap();
        assert!(Config::load(&folder).is_err());
        assert_eq!(
            fs::read(folder.join("config.json")).unwrap(),
            b"{broken secret"
        );
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn validates_config_without_echoing_secrets() {
        for data in [
            r#"{"bind_address":"0.0.0.0","port":0,"token":"secret"}"#,
            r#"{"bind_address":"invalid","port":8192,"token":"secret"}"#,
            r#"{"bind_address":"0.0.0.0","port":8192,"token":" "}"#,
            r#"{"bind_address":"0.0.0.0","port":8192,"token":"secret","typo":true}"#,
        ] {
            let error = Config::parse(data.as_bytes()).err().unwrap();
            assert!(!error.contains("secret"));
        }
    }
}
