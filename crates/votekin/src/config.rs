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
        let path = folder.join("config.yaml");
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
                    let data = serde_yaml_ng::to_string(&config)
                        .map_err(|_| "Cannot encode VoteKin configuration")?;
                    file.write_all(data.as_bytes())
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
                    .map_err(|_| "Cannot read VoteKin config.yaml")?;
                if data.len() > 16384 {
                    return Err("VoteKin config.yaml exceeds 16 KiB".into());
                }
                Self::parse(&data)
            }
            Err(_) => Err("Cannot create VoteKin config.yaml".into()),
        }
    }

    fn parse(data: &[u8]) -> Result<Self, String> {
        let config: Self = serde_yaml_ng::from_slice(data).map_err(
            |_| "Invalid VoteKin config.yaml: check bind_address, port, token, and enable_v1",
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
            !Config::parse(b"bind_address: 0.0.0.0\nport: 8192\ntoken: secret\n")
                .unwrap()
                .enable_v1
        );
        assert_eq!(Config::load(&folder).unwrap().token, first.token);
        fs::write(folder.join("config.yaml"), b"token: [broken secret").unwrap();
        assert!(Config::load(&folder).is_err());
        assert_eq!(
            fs::read(folder.join("config.yaml")).unwrap(),
            b"token: [broken secret"
        );
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn validates_config_without_echoing_secrets() {
        for data in [
            "bind_address: 0.0.0.0\nport: 0\ntoken: secret\n",
            "bind_address: invalid\nport: 8192\ntoken: secret\n",
            "bind_address: 0.0.0.0\nport: 8192\ntoken: ' '\n",
            "bind_address: 0.0.0.0\nport: 8192\ntoken: secret\ntypo: true\n",
        ] {
            let error = Config::parse(data.as_bytes()).err().unwrap();
            assert!(!error.contains("secret"));
        }
    }
}
