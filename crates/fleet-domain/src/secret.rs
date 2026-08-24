use std::fmt::{Debug, Display, Formatter};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRef(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRefError {
    Empty,
    UnsupportedScheme,
    InvalidCharacter,
    InvalidPath,
    ContainsInlineSecret,
}

impl SecretRef {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, SecretRefError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(SecretRefError::Empty);
        }
        let Some(path) = value.strip_prefix("secret://") else {
            return Err(SecretRefError::UnsupportedScheme);
        };
        if path.is_empty() {
            return Err(SecretRefError::InvalidPath);
        }
        if value.contains(['?', '#']) || value.chars().any(char::is_whitespace) {
            return Err(SecretRefError::InvalidCharacter);
        }
        if path.contains('=') || path.contains("token=") || path.contains("secret=") {
            return Err(SecretRefError::ContainsInlineSecret);
        }
        if path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            return Err(SecretRefError::InvalidPath);
        }
        if !path.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/')
        }) {
            return Err(SecretRefError::InvalidCharacter);
        }

        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn redacted_kind(&self) -> &'static str {
        "secret_ref"
    }
}

impl Debug for SecretRef {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretRef([REDACTED])")
    }
}

impl Display for SecretRef {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.redacted_kind())
    }
}

impl Display for SecretRefError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "secret reference cannot be empty"),
            Self::UnsupportedScheme => {
                write!(formatter, "secret reference must use secret:// scheme")
            }
            Self::InvalidCharacter => {
                write!(formatter, "secret reference contains invalid characters")
            }
            Self::InvalidPath => write!(formatter, "secret reference path is invalid"),
            Self::ContainsInlineSecret => {
                write!(
                    formatter,
                    "secret reference must not contain inline secret material"
                )
            }
        }
    }
}

impl std::error::Error for SecretRefError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_ref_accepts_supported_reference_format() {
        let reference = SecretRef::parse("secret://nginx/token").unwrap();

        assert_eq!(reference.as_str(), "secret://nginx/token");
        assert_eq!(reference.to_string(), "secret_ref");
        assert!(!format!("{reference:?}").contains("nginx/token"));
    }

    #[test]
    fn secret_ref_rejects_raw_empty_unsupported_and_unsafe_values() {
        for value in [
            "",
            "plain-secret-value",
            "env://TOKEN",
            "secret://",
            "secret://../token",
            "secret://app/./token",
            "secret://app/token?raw=1",
            "secret://app/token=value",
            "secret://app/token secret",
        ] {
            assert!(
                SecretRef::parse(value).is_err(),
                "secret ref should reject {value:?}"
            );
        }
    }
}
