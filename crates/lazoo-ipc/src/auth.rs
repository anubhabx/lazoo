use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::IpcError;

pub struct ProofFields<'a> {
    pub challenge: &'a [u8],
    pub installation_id: &'a str,
    pub client_name: &'a str,
    pub client_version: &'a str,
    pub offered_versions: &'a [u32],
}

pub fn compute_proof(key: &[u8], fields: &ProofFields<'_>) -> Result<Vec<u8>, IpcError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| IpcError::InvalidHmacKey)?;
    update_field(&mut mac, fields.challenge)?;
    update_field(&mut mac, fields.installation_id.as_bytes())?;
    update_field(&mut mac, fields.client_name.as_bytes())?;
    update_field(&mut mac, fields.client_version.as_bytes())?;
    let versions = fields
        .offered_versions
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    update_field(&mut mac, versions.as_bytes())?;
    Ok(mac.finalize().into_bytes().to_vec())
}

#[must_use]
pub fn verify_proof(key: &[u8], fields: &ProofFields<'_>, proof: &[u8]) -> bool {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return false;
    };
    if update_field(&mut mac, fields.challenge).is_err()
        || update_field(&mut mac, fields.installation_id.as_bytes()).is_err()
        || update_field(&mut mac, fields.client_name.as_bytes()).is_err()
        || update_field(&mut mac, fields.client_version.as_bytes()).is_err()
    {
        return false;
    }
    let versions = fields
        .offered_versions
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if update_field(&mut mac, versions.as_bytes()).is_err() {
        return false;
    }
    mac.verify_slice(proof).is_ok()
}

fn update_field(mac: &mut Hmac<Sha256>, value: &[u8]) -> Result<(), IpcError> {
    let length = u32::try_from(value.len()).map_err(|_| IpcError::FrameTooLarge {
        actual: value.len(),
        maximum: u32::MAX as usize,
    })?;
    mac.update(&length.to_le_bytes());
    mac.update(value);
    Ok(())
}
