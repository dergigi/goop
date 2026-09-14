//! NIP-17 kind-15 files. Only ciphertext is suitable for a media server.
use aes_gcm::{Aes256Gcm, Nonce, aead::{Aead, AeadCore, KeyInit, OsRng}};
use anyhow::{Result, anyhow, bail, ensure};
use nostr_sdk::prelude::*;
use sha2::{Digest, Sha256};

pub const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedFile {
    pub url: Url,
    pub mime: String,
    key: [u8; 32],
    nonce: [u8; 12],
    hash: String,
    original_hash: Option<String>,
    size: Option<usize>,
}

// Never include encryption keys in logs or debug output.
impl std::fmt::Debug for EncryptedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedFile").field("mime", &self.mime).finish_non_exhaustive()
    }
}

pub struct EncryptedUpload {
    pub ciphertext: Vec<u8>,
    pub file: EncryptedFile,
}

fn digest(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }

impl EncryptedFile {
    pub fn encrypt(plaintext: &[u8], mime: String) -> Result<EncryptedUpload> {
        ensure!(plaintext.len() <= MAX_FILE_BYTES, "Attachment exceeds the 100 MB limit");
        let key = Aes256Gcm::generate_key(&mut OsRng);
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = Aes256Gcm::new(&key).encrypt(&nonce, plaintext)
            .map_err(|_| anyhow!("Attachment encryption failed"))?;
        Ok(EncryptedUpload {
            file: Self {
                url: Url::parse("https://invalid.invalid/pending")?, mime,
                key: key.into(), nonce: nonce.into(),
                hash: digest(&ciphertext), original_hash: Some(digest(plaintext)),
                size: Some(plaintext.len()),
            }, ciphertext,
        })
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        ensure!(ciphertext.len() <= MAX_FILE_BYTES + 16, "Encrypted attachment is too large");
        ensure!(digest(ciphertext) == self.hash, "Attachment hash mismatch");
        let plaintext = Aes256Gcm::new_from_slice(&self.key).unwrap()
            .decrypt(Nonce::from_slice(&self.nonce), ciphertext)
            .map_err(|_| anyhow!("Attachment authentication failed"))?;
        // Dark Wisp sends the original size. Accept Goop's earlier ciphertext
        // size tags too, after authenticating the complete ciphertext.
        ensure!(self.size.is_none_or(|size| size == plaintext.len() || size == ciphertext.len()),
            "Attachment size mismatch");
        if let Some(hash) = &self.original_hash {
            ensure!(digest(&plaintext) == *hash, "Original attachment hash mismatch");
        }
        Ok(plaintext)
    }

    /// These tags belong only inside a sealed and gift-wrapped rumor.
    pub fn tags(&self) -> Vec<Tag> {
        let mut tags = vec![
            Tag::custom("file-type", [&self.mime]),
            Tag::custom("encryption-algorithm", ["aes-gcm"]),
            Tag::custom("decryption-key", [hex::encode(self.key)]),
            Tag::custom("decryption-nonce", [hex::encode(self.nonce)]),
            Tag::custom("x", [&self.hash]),
        ];
        if let Some(hash) = &self.original_hash { tags.push(Tag::custom("ox", [hash])); }
        if let Some(size) = self.size { tags.push(Tag::custom("size", [size.to_string()])); }
        tags
    }

    pub fn from_tags(url: &str, tags: &Tags) -> Result<Self> {
        let value = |name: &str| -> Result<String> {
            let values: Vec<_> = tags.iter().filter(|tag| tag.as_slice().first().is_some_and(|v| v == name)).collect();
            ensure!(values.len() == 1, "Missing or duplicate {name} tag");
            values[0].as_slice().get(1).cloned().ok_or_else(|| anyhow!("Invalid {name} tag"))
        };
        let optional = |name: &str| -> Result<Option<String>> {
            if tags.iter().any(|tag| tag.as_slice().first().is_some_and(|v| v == name)) {
                value(name).map(Some)
            } else { Ok(None) }
        };
        ensure!(value("encryption-algorithm")? == "aes-gcm", "Unsupported attachment encryption");
        let key: [u8; 32] = hex::decode(value("decryption-key")?).map_err(|_| anyhow!("Invalid file key"))?
            .try_into().map_err(|_| anyhow!("Invalid file key length"))?;
        let nonce: [u8; 12] = hex::decode(value("decryption-nonce")?).map_err(|_| anyhow!("Invalid file nonce"))?
            .try_into().map_err(|_| anyhow!("Invalid file nonce length"))?;
        let hash = value("x")?.to_lowercase();
        ensure!(hex::decode(&hash).is_ok_and(|bytes| bytes.len() == 32), "Invalid ciphertext hash");
        let original_hash = optional("ox")?.map(|hash| hash.to_lowercase());
        if let Some(hash) = &original_hash {
            ensure!(hex::decode(hash).is_ok_and(|bytes| bytes.len() == 32), "Invalid original hash");
        }
        let size = optional("size")?.map(|size| size.parse::<usize>()).transpose()?;
        ensure!(size.is_none_or(|size| size <= MAX_FILE_BYTES + 16), "Attachment is too large");
        let url = Url::parse(url)?;
        ensure!(url.scheme() == "https", "Encrypted attachments require HTTPS");
        ensure!(url.username().is_empty() && url.password().is_none(), "Invalid attachment URL");
        Ok(Self { url, mime: value("file-type")?, key, nonce, hash, original_hash, size })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn download(file: EncryptedFile, cx: &gpui::AsyncApp) -> Result<Vec<u8>> {
    gpui_tokio::Tokio::spawn(cx, async move {
        let client = reqwest::Client::builder()
            .https_only(true).timeout(std::time::Duration::from_secs(90)).build()?;
        let mut response = client.get(file.url.clone()).send().await?.error_for_status()?;
        ensure!(response.content_length().is_none_or(|n| n <= (MAX_FILE_BYTES + 16) as u64), "Attachment is too large");
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if data.len() + chunk.len() > MAX_FILE_BYTES + 16 { bail!("Attachment is too large"); }
            data.extend_from_slice(&chunk);
        }
        file.decrypt(&data)
    }).await.map_err(|_| anyhow!("Attachment download task failed"))?
}

#[cfg(target_arch = "wasm32")]
pub async fn download(_: EncryptedFile, _: &gpui::AsyncApp) -> Result<Vec<u8>> {
    bail!("Encrypted attachment downloads are not supported on web")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_randomness_tags_and_tamper_rejection() {
        let data = b"private screenshot bytes";
        let a = EncryptedFile::encrypt(data, "image/png".into()).unwrap();
        let b = EncryptedFile::encrypt(data, "image/png".into()).unwrap();
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.file.key, b.file.key);
        assert_ne!(a.file.nonce, b.file.nonce);
        assert!(!a.ciphertext.windows(data.len()).any(|window| window == data));
        let tags = Tags::from_list(a.file.tags());
        let parsed = EncryptedFile::from_tags("https://example.com/blob", &tags).unwrap();
        assert_eq!(parsed.decrypt(&a.ciphertext).unwrap(), data);
        let mut tampered = a.ciphertext.clone(); tampered[0] ^= 1;
        assert!(parsed.decrypt(&tampered).is_err());
        let mut wrong_key = parsed.clone(); wrong_key.key[0] ^= 1;
        assert!(wrong_key.decrypt(&a.ciphertext).is_err());
        assert!(parsed.decrypt(&a.ciphertext[..a.ciphertext.len()-1]).is_err());
        assert!(!format!("{:?}", parsed).contains(&hex::encode(parsed.key)));
    }
    #[test]
    fn invalid_metadata_is_rejected_before_download() {
        let file = EncryptedFile::encrypt(b"image", "image/png".into()).unwrap().file;
        let mut tags = file.tags();
        tags.push(Tag::custom("decryption-key", ["00"]));
        assert!(EncryptedFile::from_tags("https://example.com/blob", &Tags::from_list(tags)).is_err());
        let tags = Tags::from_list(file.tags());
        assert!(EncryptedFile::from_tags("http://example.com/blob", &tags).is_err());
        assert!(EncryptedFile::from_tags("file:///tmp/image", &tags).is_err());
        assert!(EncryptedFile::from_tags("https://example.com/blob", &Tags::new()).is_err());
        let tags = file.tags().into_iter().filter(|tag| tag.kind() != "encryption-algorithm")
            .chain([Tag::custom("encryption-algorithm", ["unknown"])]).collect();
        assert!(EncryptedFile::from_tags("https://example.com/blob", &Tags::from_list(tags)).is_err());
    }

    #[test]
    fn nist_aes256_gcm_vector_matches_dark_wisp_wire_layout() {
        // Ciphertext followed by the 128-bit authentication tag, no AAD.
        let ciphertext = hex::decode("cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919").unwrap();
        let file = EncryptedFile { url: Url::parse("https://example.com/blob").unwrap(),
            mime: "application/octet-stream".into(), key: [0;32], nonce: [0;12],
            hash: digest(&ciphertext), original_hash: None, size: None };
        assert_eq!(file.decrypt(&ciphertext).unwrap(), [0;16]);
    }

    #[test]
    fn dark_wisp_original_size_and_legacy_goop_size_are_supported() {
        // AES-256-GCM fixture with Dark Wisp's kind-15 tag layout and
        // original-byte size, independent of Goop's encrypt/tag builders.
        let ciphertext = hex::decode("cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919").unwrap();
        let tags = Tags::from_list(vec![
            Tag::custom("file-type", ["image/png"]),
            Tag::custom("encryption-algorithm", ["aes-gcm"]),
            Tag::custom("decryption-key", ["00".repeat(32)]),
            Tag::custom("decryption-nonce", ["00".repeat(12)]),
            Tag::custom("x", [digest(&ciphertext)]),
            Tag::custom("ox", [digest(&[0; 16])]),
            Tag::custom("size", ["16"]),
        ]);
        let mut file = EncryptedFile::from_tags("https://example.com/blob", &tags).unwrap();
        assert_eq!(file.decrypt(&ciphertext).unwrap(), [0; 16]);
        file.size = Some(32);
        assert_eq!(file.decrypt(&ciphertext).unwrap(), [0; 16]);
        file.size = Some(17);
        assert!(file.decrypt(&ciphertext).is_err());

        let upload = EncryptedFile::encrypt(&[0; 16], "image/png".into()).unwrap();
        assert!(upload.file.tags().iter().any(|tag| tag.as_slice() == ["size", "16"]));
    }
}
