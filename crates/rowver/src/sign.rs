//! A file's ed25519 signature: who pressed it, and that nothing in its header changed since.

use crate::json::canonical;
use crate::rvr::{digest, hex};
use crate::{invalid, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{json, Map, Value};

/// The SHA-256 a signature signs: of the canonical JSON of `rowver`, `data`, `made` and
/// `derived`, the parts every checksum covers.
pub(crate) fn signed_digest(header: &Value) -> Result<String> {
    let mut body = Map::new();
    for key in ["rowver", "data", "made", "derived"] {
        if let Some(value) = header.get(key) {
            body.insert(key.into(), value.clone());
        }
    }
    Ok(digest(canonical(&Value::Object(body))?.as_bytes()))
}

/// The public key of a secret key, as 64 hexadecimal characters.
pub fn public_key(secret: &[u8; 32]) -> String {
    hex(SigningKey::from_bytes(secret).verifying_key().as_bytes())
}

/// Sign a whole header, putting its `signature` last.
pub fn sign(header: &mut Value, secret: &[u8; 32]) -> Result<()> {
    let key = SigningKey::from_bytes(secret);
    let root = header
        .as_object_mut()
        .ok_or_else(|| invalid("Header must be an object"))?;
    root.shift_remove("signature");
    let sum = signed_digest(&Value::Object(root.clone()))?;
    let signature = key.sign(&bytes::<32>(&sum)?);
    root.insert(
        "signature".into(),
        json!({"ed25519": hex(&signature.to_bytes()), "key": hex(key.verifying_key().as_bytes()), "sha256": sum}),
    );
    Ok(())
}

/// The key that signed a header, or `None` when it is not signed. A signature that does not
/// hold, because the header changed after it was signed, is refused.
pub(crate) fn verify(header: &Value) -> Result<Option<String>> {
    let Some(signature) = header.get("signature") else {
        return Ok(None);
    };
    let field = |name: &str| {
        signature.get(name).and_then(Value::as_str).ok_or_else(|| {
            invalid(format!(
                "signature.{name} is missing, or not text: the signature cannot be checked (docs/FORMAT.md §9)"
            ))
        })
    };
    let fields = signature.as_object().map(|s| s.keys().map(String::as_str).collect::<Vec<_>>());
    if fields.is_none_or(|f| f.len() != 3 || !f.iter().all(|k| ["ed25519", "key", "sha256"].contains(k))) {
        return Err(invalid(
            "signature must hold exactly ed25519, key and sha256 (docs/FORMAT.md §9): press its study again",
        ));
    }
    let (key, sum) = (field("key")?, field("sha256")?);
    let hex = |name: &str, n: usize| invalid(format!("signature.{name} must be {n} lowercase hexadecimal characters (docs/FORMAT.md §9): press its study again"));
    let sum_bytes = bytes::<32>(sum).map_err(|_| hex("sha256", 64))?;
    if signed_digest(header)? != sum {
        return Err(invalid(format!("the header changed after it was signed (signature.sha256 does not match it): {}", crate::rvr::MEND)));
    }
    let public = VerifyingKey::from_bytes(&bytes::<32>(key).map_err(|_| hex("key", 64))?)
        .map_err(|_| invalid("signature.key is not an ed25519 public key (docs/FORMAT.md §9): press its study again"))?;
    let value = Signature::from_bytes(&bytes::<64>(field("ed25519")?).map_err(|_| hex("ed25519", 128))?);
    public.verify_strict(&sum_bytes, &value).map_err(|_| {
        invalid("signature.ed25519 does not hold for signature.key, checked strictly as RFC 8032 says, a key of small order refused (docs/FORMAT.md §9): the header changed after it was signed, or another key signed it; press its study again")
    })?;
    Ok(Some(key.to_owned()))
}

/// Bytes written as lowercase hexadecimal, exactly `N` of them.
pub fn bytes<const N: usize>(text: &str) -> Result<[u8; N]> {
    let wrong = || {
        invalid(format!(
            "Expected {} lowercase hexadecimal characters",
            2 * N
        ))
    };
    if !crate::study::hex(text, 2 * N) {
        return Err(wrong());
    }
    let mut out = [0; N];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * index..2 * index + 2], 16).map_err(|_| wrong())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // FORMAT 9, 11
    #[test]
    fn a_signature_holds_until_the_header_changes() {
        let mut header =
            json!({"rowver": 15, "data": {"rows": 1}, "made": {"study": "s"}, "tool": {}});
        assert_eq!(verify(&header).unwrap(), None);
        sign(&mut header, &[7; 32]).unwrap();
        assert_eq!(verify(&header).unwrap(), Some(public_key(&[7; 32])));
        header["tool"]["note"] = "tool keys are outside it".into();
        assert!(verify(&header).is_ok());
        header["made"]["study"] = "t".into();
        assert!(verify(&header).is_err());
        sign(&mut header, &[7; 32]).unwrap();
        let other = public_key(&[8; 32]);
        header["signature"]["key"] = other.into();
        assert!(verify(&header).is_err());
    }
}
