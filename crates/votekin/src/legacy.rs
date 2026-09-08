use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use rand_chacha::ChaCha20Rng;
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    rand_core::SeedableRng,
    traits::PublicKeyParts,
};

pub struct Legacy {
    pub key: RsaPrivateKey,
    pub rng: ChaCha20Rng,
}

impl Legacy {
    pub fn load(folder: &Path) -> Result<Self, String> {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).map_err(|_| "Secure randomness unavailable")?;
        let mut rng = ChaCha20Rng::from_seed(seed);
        let private_path = folder.join("private.pem");
        let public_path = folder.join("public.key");
        let mut key = match read_key(&private_path) {
            Ok(data) => RsaPrivateKey::from_pkcs8_pem(&data)
                .map_err(|_| "Invalid VoteKin private.pem; restore the original key")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if public_path.exists() {
                    return Err("VoteKin private.pem is missing but public.key exists; restore the private key".into());
                }
                tracing::info!("Generating VoteKin legacy RSA keys");
                let key = RsaPrivateKey::new(&mut rng, 2048)
                    .map_err(|_| "Cannot generate legacy RSA key")?;
                let pem = key
                    .to_pkcs8_pem(LineEnding::LF)
                    .map_err(|_| "Cannot encode legacy private key")?;
                write_new(&private_path, pem.as_bytes())?;
                key
            }
            Err(_) => return Err("Cannot read VoteKin private.pem".into()),
        };
        key.validate().map_err(|_| "Invalid legacy RSA key")?;
        if key.n().bits() != 2048 {
            return Err("Legacy RSA key must be 2048 bits".into());
        }
        key.precompute()
            .map_err(|_| "Cannot prepare legacy RSA key")?;
        let public = RsaPublicKey::from(&key)
            .to_public_key_der()
            .map_err(|_| "Cannot encode legacy public key")?;
        let public = STANDARD.encode(public.as_bytes());
        match read_key(&public_path) {
            Ok(existing) if existing.trim() == public => {}
            Ok(_) => return Err("VoteKin public.key does not match private.pem".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&public_path, public.as_bytes())?
            }
            Err(_) => return Err("Cannot read VoteKin public.key".into()),
        }
        Ok(Self { key, rng })
    }
}

fn read_key(path: &Path) -> std::io::Result<String> {
    let mut data = String::new();
    fs::File::open(path)?
        .take(16385)
        .read_to_string(&mut data)?;
    if data.len() > 16384 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(data)
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Cannot create legacy key file")?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|_| "Cannot save legacy key file".into())
}
