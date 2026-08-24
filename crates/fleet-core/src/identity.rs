use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentKeyPair {
    pub private_key_hex: String,
    pub public_key_hex: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    RandomFailed,
    InvalidHex,
    InvalidPrivateKey,
    InvalidPublicKey,
    InvalidSignature,
}

impl Display for IdentityError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RandomFailed => write!(formatter, "failed to generate identity entropy"),
            Self::InvalidHex => write!(formatter, "invalid hex encoded identity material"),
            Self::InvalidPrivateKey => write!(formatter, "invalid private key"),
            Self::InvalidPublicKey => write!(formatter, "invalid public key"),
            Self::InvalidSignature => write!(formatter, "invalid signature"),
        }
    }
}

impl std::error::Error for IdentityError {}

pub fn generate_agent_key_pair() -> Result<AgentKeyPair, IdentityError> {
    let mut seed = [0_u8; 32];
    getrandom::getrandom(&mut seed).map_err(|_| IdentityError::RandomFailed)?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key_hex = hex_encode(&signing_key.verifying_key().to_bytes());
    let fingerprint = fingerprint_public_key(&public_key_hex)?;
    Ok(AgentKeyPair {
        private_key_hex: hex_encode(&seed),
        public_key_hex,
        fingerprint,
    })
}

pub fn fingerprint_public_key(public_key_hex: &str) -> Result<String, IdentityError> {
    let public_key = hex_decode_exact::<32>(public_key_hex)?;
    let digest = Sha256::digest(public_key);
    Ok(hex_encode(&digest))
}

pub fn sign_challenge(private_key_hex: &str, nonce: &str) -> Result<String, IdentityError> {
    let private_key = hex_decode_exact::<32>(private_key_hex)?;
    let signing_key = SigningKey::from_bytes(&private_key);
    Ok(hex_encode(&signing_key.sign(nonce.as_bytes()).to_bytes()))
}

pub fn verify_challenge_signature(
    public_key_hex: &str,
    nonce: &str,
    signature_hex: &str,
) -> Result<bool, IdentityError> {
    let public_key = hex_decode_exact::<32>(public_key_hex)?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_key).map_err(|_| IdentityError::InvalidPublicKey)?;
    let signature_bytes = hex_decode_exact::<64>(signature_hex)?;
    let signature = Signature::from_bytes(&signature_bytes);
    Ok(verifying_key.verify(nonce.as_bytes(), &signature).is_ok())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningMaterialValidation {
    pub public_key_fingerprint: String,
}

pub fn validate_signing_material_pair(
    public_key_hex: &str,
    private_key_hex: &str,
    challenge: &str,
) -> Result<SigningMaterialValidation, IdentityError> {
    let signature = sign_challenge(private_key_hex, challenge)?;
    if !verify_challenge_signature(public_key_hex, challenge, &signature)? {
        return Err(IdentityError::InvalidSignature);
    }
    Ok(SigningMaterialValidation {
        public_key_fingerprint: fingerprint_public_key(public_key_hex)?,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn hex_decode_exact<const N: usize>(value: &str) -> Result<[u8; N], IdentityError> {
    let bytes = hex_decode(value)?;
    bytes.try_into().map_err(|_| IdentityError::InvalidHex)
}

fn hex_decode(value: &str) -> Result<Vec<u8>, IdentityError> {
    if !value.len().is_multiple_of(2) {
        return Err(IdentityError::InvalidHex);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let high = hex_nibble(chunk[0])?;
            let low = hex_nibble(chunk[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Result<u8, IdentityError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(IdentityError::InvalidHex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_pair_signs_and_verifies_challenge() {
        let key_pair = generate_agent_key_pair().unwrap();
        let signature = sign_challenge(&key_pair.private_key_hex, "nonce-1").unwrap();

        assert!(
            verify_challenge_signature(&key_pair.public_key_hex, "nonce-1", &signature).unwrap()
        );
    }

    #[test]
    fn wrong_challenge_rejects_signature() {
        let key_pair = generate_agent_key_pair().unwrap();
        let signature = sign_challenge(&key_pair.private_key_hex, "nonce-1").unwrap();

        assert!(
            !verify_challenge_signature(&key_pair.public_key_hex, "nonce-2", &signature).unwrap()
        );
    }

    #[test]
    fn fingerprint_is_derived_from_public_key() {
        let key_pair = generate_agent_key_pair().unwrap();

        assert_eq!(
            fingerprint_public_key(&key_pair.public_key_hex).unwrap(),
            key_pair.fingerprint
        );
    }

    #[test]
    fn invalid_public_key_is_rejected() {
        assert!(matches!(
            verify_challenge_signature("not-hex", "nonce-1", "sig"),
            Err(IdentityError::InvalidHex)
        ));
    }

    #[test]
    fn valid_signing_material_pair_returns_public_fingerprint() {
        let key_pair = generate_agent_key_pair().unwrap();

        let validation = validate_signing_material_pair(
            &key_pair.public_key_hex,
            &key_pair.private_key_hex,
            "controller-signing-rotation-validation",
        )
        .unwrap();

        assert_eq!(validation.public_key_fingerprint, key_pair.fingerprint);
    }

    #[test]
    fn mismatched_signing_material_pair_is_rejected() {
        let public_pair = generate_agent_key_pair().unwrap();
        let private_pair = generate_agent_key_pair().unwrap();

        assert!(matches!(
            validate_signing_material_pair(
                &public_pair.public_key_hex,
                &private_pair.private_key_hex,
                "controller-signing-rotation-validation"
            ),
            Err(IdentityError::InvalidSignature)
        ));
    }

    #[test]
    fn invalid_signing_material_error_does_not_echo_key_material() {
        let error =
            validate_signing_material_pair("not-hex-public-key", "PRIVATE KEY BODY", "challenge")
                .unwrap_err();

        let error_text = format!("{error}");
        assert!(!error_text.contains("PRIVATE KEY BODY"));
        assert!(!error_text.contains("not-hex-public-key"));
    }
}
